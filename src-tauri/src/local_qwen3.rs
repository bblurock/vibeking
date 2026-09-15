//! Local STT via FluidAudio (Qwen3-ASR 0.6B) on Apple Silicon.
//!
//! This is the **Chinese / CJK specialist** in the local-engine family.
//! Parakeet TDT v3 is European-only (it cannot transcribe Mandarin), and
//! whisper.cpp cannot stream cleanly — so when a local-engine user dictates
//! Chinese we transparently route here instead. Qwen3 supports Chinese (plus
//! Cantonese + dialects, Japanese, Korean, …) and, crucially, exposes real
//! streaming partials (`qwen3_streaming_feed`) for the live dictation bar.
//!
//! macOS-15+ / Apple Silicon. First initialization downloads the Qwen3 CoreML
//! weights (~4 GB, f32) into the shared FluidAudio cache and JIT-compiles them
//! for the ANE. The warm `FluidAudio` instance is cached for the process
//! lifetime behind a `parking_lot::Mutex`, mirroring [`crate::local_parakeet`].
//!
//! Streaming uses a one-second re-decode cadence; see [`STREAM_CHUNK_SECONDS`].

#[cfg(not(target_os = "macos"))]
pub use noop::*;

#[cfg(target_os = "macos")]
pub use platform::*;

/// Identifier used by the model-management UI / Tauri commands.
pub const ENGINE_ID: &str = "qwen3-asr-0.6b";

/// Streaming re-transcribe cadence, in seconds — the `chunk_seconds` handed to
/// FluidAudio. Picked from the spike: 1.0 s gave zero-flicker prefix-stable
/// partials; 0.5 s updated faster but introduced rewrites, 2.0 s was too
/// coarse. This governs ONLY the steady-state re-decode rhythm — the time to
/// the *first* partial is set separately by [`STREAM_MIN_AUDIO_SECONDS`].
pub const STREAM_CHUNK_SECONDS: f64 = 1.0;

/// Audio that must accumulate before the FIRST streaming partial is emitted —
/// the `min_audio_seconds` handed to FluidAudio. Lower than the re-decode
/// cadence so the live card lights up sooner; subsequent ticks still run on
/// [`STREAM_CHUNK_SECONDS`], keeping their prefix-stable, no-rewrite behaviour.
/// (The spike's rewrite flicker came from a 0.5 s *chunk* cadence; a 0.5 s
/// *first-partial* threshold is a different axis and doesn't reintroduce it.)
pub const STREAM_MIN_AUDIO_SECONDS: f64 = 0.5;

/// Per-session `max_audio_seconds` handed to FluidAudio. Kept generously above
/// our own rolling-window cap (`WINDOW_HARD_SAMPLES`, 30 s) so the underlying
/// session never reaches its own buffer limit before we roll it — see the
/// windowing note on [`StreamWindow`]. The Qwen3 CoreML decoder has a hard
/// 512-token KV-cache (≈41 s of audio); a single continuous session overflows
/// past that with `generationFailed("Prompt length N exceeds cache capacity
/// 512")`, so long dictation is handled by stitching ≤30 s windows, NOT by one
/// long session. The streaming final is still the **authoritative transcript
/// for Qwen3** — the batch path overflows identically on long audio.
pub const STREAM_MAX_SECONDS: f64 = 600.0;

#[cfg(not(target_os = "macos"))]
mod noop {
    use anyhow::{anyhow, Result};

    pub fn engine_ready() -> bool {
        false
    }

    pub fn is_preparing() -> bool {
        false
    }

    pub async fn prepare_engine(
        _on_progress: impl Fn(u64, Option<u64>) + Send + Sync + 'static,
    ) -> Result<()> {
        Err(anyhow!("Qwen3 engine only available on macOS"))
    }

    pub async fn transcribe(
        _wav: &[u8],
        _lang: Option<&str>,
        _hotwords: &[String],
        _context_candidates: &[String],
    ) -> Result<String> {
        Err(anyhow!("Qwen3 engine only available on macOS"))
    }

    pub async fn warm_engine() -> Result<()> {
        Err(anyhow!("Qwen3 engine only available on macOS"))
    }

    pub async fn streaming_start(_lang: Option<&str>) -> Result<()> {
        Err(anyhow!("Qwen3 engine only available on macOS"))
    }

    pub fn streaming_feed(_samples: &[f32]) -> Result<Option<String>> {
        Err(anyhow!("Qwen3 engine only available on macOS"))
    }

    pub fn streaming_finish() -> Result<String> {
        Err(anyhow!("Qwen3 engine only available on macOS"))
    }

    pub fn clear_engine_cache() -> Result<()> {
        Err(anyhow!("Qwen3 engine only available on macOS"))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use anyhow::{anyhow, Result};
    use fluidaudio_rs::FluidAudio;
    use parking_lot::Mutex;

    use crate::audio_io::decode_wav_to_mono_f32;

    static ENGINE: Mutex<Option<Arc<FluidAudio>>> = Mutex::new(None);

    const STREAM_SR: usize = 16_000;
    /// Once a session has accumulated this much audio, roll it at the next
    /// near-silent frame (a natural pause) so we cut between words.
    const WINDOW_TARGET_SAMPLES: usize = STREAM_SR * 25; // 25 s
    /// Hard ceiling: roll regardless of silence by here. Well under the ~41 s /
    /// 512-token decoder-cache cap, leaving headroom for prefix/suffix tokens.
    const WINDOW_HARD_SAMPLES: usize = STREAM_SR * 30; // 30 s
    /// A feed frame quieter than this RMS counts as a pause — a safe cut point.
    /// Mirrors lib.rs `PREVIEW_MIN_RMS`.
    const WINDOW_SILENCE_RMS: f32 = 0.006;

    /// Streaming session window state. The Qwen3 CoreML decoder has a hard
    /// 512-token KV-cache (≈41 s of audio); a single continuous session
    /// overflows past that with `generationFailed("Prompt length N exceeds
    /// cache capacity 512")` — which is why long dictation previously failed on
    /// BOTH the batch and streaming paths. We keep each underlying session under
    /// ~30 s (rolling `finish()`→`start()` at a silence boundary) and stitch
    /// their transcripts in `committed`, so total dictation length is unbounded
    /// while live partials keep flowing. Reset on every `streaming_start`.
    struct StreamWindow {
        engine: Arc<FluidAudio>,
        /// Stitched transcript of all already-finished windows.
        committed: String,
        /// 16 kHz samples fed to the CURRENT (open) session.
        samples_in_window: usize,
        /// Language hint, re-applied to every rolled session.
        lang: Option<String>,
    }
    static WINDOW: Mutex<Option<StreamWindow>> = Mutex::new(None);

    /// RMS of a feed frame (used to detect a pause to cut a window on).
    fn frame_rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
        (sum_sq / samples.len() as f32).sqrt()
    }

    fn is_cjk(c: char) -> bool {
        ('\u{3000}'..='\u{9fff}').contains(&c)       // CJK punct/symbols + Unified (incl. Ext-A)
            || ('\u{ac00}'..='\u{d7af}').contains(&c) // Hangul syllables (Korean)
            || ('\u{f900}'..='\u{faff}').contains(&c) // CJK Compatibility Ideographs
            || ('\u{ff00}'..='\u{ffef}').contains(&c) // Fullwidth/halfwidth forms
    }

    /// Append a window's transcript onto the stitched accumulator, inserting a
    /// space only at a Latin/Latin boundary — CJK text has no inter-word spaces,
    /// so a space between two Chinese windows would read wrong.
    fn append_segment(acc: &mut String, seg: &str) {
        let seg = seg.trim();
        if seg.is_empty() {
            return;
        }
        if let (Some(prev), Some(next)) = (acc.chars().last(), seg.chars().next()) {
            if !is_cjk(prev) && !is_cjk(next) {
                acc.push(' ');
            }
        }
        acc.push_str(seg);
    }

    /// Committed windows + the current session's live partial, for the preview.
    fn compose(committed: &str, partial: &str) -> String {
        let mut s = committed.to_string();
        append_segment(&mut s, partial);
        s
    }

    /// True while `prepare_engine` is mid-download/compile. Survives frontend
    /// reloads (Rust process keeps running) so the model-management row can
    /// show "Preparing…" after a Cmd+R. Mirrors `local_parakeet`.
    static PREPARING: AtomicBool = AtomicBool::new(false);

    /// Approximate on-disk size of the Qwen3-ASR 0.6B CoreML bundle (f32:
    /// .mlpackage source + compiled .mlmodelc for encoder + decoder).
    /// Denominator for the synthetic download progress bar (fluidaudio-rs
    /// exposes no real progress stream — we poll the cache dir).
    const EXPECTED_MODEL_SIZE_BYTES: u64 = 4 * 1024 * 1024 * 1024;

    /// Minimum on-disk bytes treated as "model fully downloaded". Loose enough
    /// to survive size drift between releases, tight enough to reject a
    /// partial download.
    const COMPLETED_CACHE_THRESHOLD_BYTES: u64 = 3 * 1024 * 1024 * 1024;

    /// The Qwen3 model's own subdirectory inside the shared FluidAudio cache.
    /// Scoped to this model so readiness + cache-clear don't touch Parakeet's
    /// weights, which live in a sibling `parakeet-tdt-0.6b-v3/` dir.
    fn model_dirs() -> Vec<std::path::PathBuf> {
        let Some(home) = dirs::home_dir() else {
            return Vec::new();
        };
        // FluidAudio has historically used a couple of cache roots; the model
        // subdir name is stable.
        vec![
            home.join("Library/Application Support/FluidAudio/Models")
                .join(super::ENGINE_ID),
            home.join("Library/Application Support/FluidInference/Models")
                .join(super::ENGINE_ID),
        ]
    }

    fn dir_size(path: &std::path::Path) -> u64 {
        let Ok(rd) = std::fs::read_dir(path) else {
            return 0;
        };
        let mut total = 0u64;
        for entry in rd.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            total += if meta.is_dir() {
                dir_size(&entry.path())
            } else {
                meta.len()
            };
        }
        total
    }

    fn current_cache_size_bytes() -> u64 {
        model_dirs().iter().map(|p| dir_size(p)).sum()
    }

    /// "Ready" = no multi-GB download stands between the user and dictating:
    /// either the warm handle is cached, or the model files are on disk (the
    /// first transcribe pays a ~1 s warm load but no download).
    pub fn engine_ready() -> bool {
        if ENGINE.lock().is_some() {
            return true;
        }
        current_cache_size_bytes() >= COMPLETED_CACHE_THRESHOLD_BYTES
    }

    pub fn is_preparing() -> bool {
        PREPARING.load(Ordering::Relaxed)
    }

    /// RAII guard so PREPARING clears even if prepare_engine errors/panics.
    struct PreparingGuard;
    impl Drop for PreparingGuard {
        fn drop(&mut self) {
            PREPARING.store(false, Ordering::Relaxed);
        }
    }

    /// Download + ANE-compile the Qwen3 models and cache the warm instance.
    /// Idempotent. `on_progress(downloaded, Some(total))` is driven by polling
    /// the model cache dir every 500 ms (no upstream progress callback).
    pub async fn prepare_engine(
        on_progress: impl Fn(u64, Option<u64>) + Send + Sync + 'static,
    ) -> Result<()> {
        PREPARING.store(true, Ordering::Relaxed);
        let _guard = PreparingGuard;

        let cb: Arc<dyn Fn(u64, Option<u64>) + Send + Sync> = Arc::new(on_progress);
        let total = Some(EXPECTED_MODEL_SIZE_BYTES);
        cb(
            current_cache_size_bytes().min(EXPECTED_MODEL_SIZE_BYTES),
            total,
        );

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_poll = cancel.clone();
        let cb_poll = cb.clone();
        let poll_handle = tauri::async_runtime::spawn(async move {
            while !cancel_poll.load(Ordering::Relaxed) {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let size = current_cache_size_bytes().min(EXPECTED_MODEL_SIZE_BYTES);
                // Stay under 100% until ensure_engine returns (ANE compile is
                // still in flight after bytes plateau).
                let clamped = size.min(EXPECTED_MODEL_SIZE_BYTES.saturating_sub(1));
                cb_poll(clamped, total);
            }
        });

        let result = ensure_engine().await.map(|_| ());
        cancel.store(true, Ordering::Relaxed);
        let _ = poll_handle.await;

        if result.is_ok() {
            cb(EXPECTED_MODEL_SIZE_BYTES, total);
        }
        result
    }

    /// Batch transcribe a WAV blob with Qwen3, passing the language hint.
    /// `hotwords` / `context_candidates` are accepted for signature parity
    /// with the other local engines but not used acoustically — the upstream
    /// learned-correction dictionary substitution (applied by the caller in
    /// lib.rs) covers post-hoc fixes, and Qwen3's batch API has no vocabulary
    /// primitive. Chinese has no whitespace word boundaries, so the Parakeet
    /// fuzzy-substitution approach doesn't transfer here.
    pub async fn transcribe(
        wav: &[u8],
        lang: Option<&str>,
        _hotwords: &[String],
        _context_candidates: &[String],
    ) -> Result<String> {
        let pcm = decode_wav_to_mono_f32(wav)?;
        let engine = ensure_engine().await?;
        let lang_owned = lang.map(str::to_string);
        let text = tauri::async_runtime::spawn_blocking(move || -> Result<String> {
            let res = engine
                .qwen3_transcribe_samples(&pcm, lang_owned.as_deref())
                .map_err(|e| anyhow!("qwen3 transcribe: {e}"))?;
            Ok(res.text)
        })
        .await
        .map_err(|e| anyhow!("qwen3 join: {e}"))??;
        Ok(text.trim().to_string())
    }

    // ===================== Streaming (live dictation) =====================
    //
    // Recordings are serialized (one hotkey session at a time), so a single
    // process-wide streaming session on the cached engine is safe. The caller
    // feeds 16 kHz mono f32 frames during recording and finishes on stop.

    /// Initialize the streaming engine (downloading the model if needed) and
    /// open the first window for `lang`. Call once at recording start. Resets
    /// the rolling-window accumulator.
    pub async fn streaming_start(lang: Option<&str>) -> Result<()> {
        let engine = ensure_engine().await?;
        let lang_owned = lang.map(str::to_string);
        let engine_for_session = engine.clone();
        let lang_for_session = lang_owned.clone();
        tauri::async_runtime::spawn_blocking(move || -> Result<()> {
            if !engine_for_session.is_qwen3_streaming_available() {
                engine_for_session
                    .init_qwen3_streaming()
                    .map_err(|e| anyhow!("init_qwen3_streaming: {e}"))?;
            }
            // Reset any dangling session from a previous recording (rapid-fire
            // hotkey) — the API requires finish() between sessions. Ignore the
            // error when there was nothing to finish.
            let _ = engine_for_session.qwen3_streaming_finish();
            engine_for_session
                .qwen3_streaming_start(
                    lang_for_session.as_deref(),
                    super::STREAM_MIN_AUDIO_SECONDS,
                    super::STREAM_CHUNK_SECONDS,
                    super::STREAM_MAX_SECONDS,
                )
                .map_err(|e| anyhow!("qwen3_streaming_start: {e}"))
        })
        .await
        .map_err(|e| anyhow!("qwen3 stream start join: {e}"))??;

        *WINDOW.lock() = Some(StreamWindow {
            engine,
            committed: String::new(),
            samples_in_window: 0,
            lang: lang_owned,
        });
        Ok(())
    }

    /// Feed a frame of 16 kHz mono f32 samples. Returns `Some(full_transcript)`
    /// — committed windows + the current session's live partial — when there's
    /// an update to show, else `None`. Synchronous; safe to call from the
    /// audio-consumer task (recordings are serialized, so the single window is
    /// uncontended). When the open session nears the decoder-cache ceiling it
    /// rolls here: finish the session, commit its text, and open a fresh one —
    /// a brief (~1 s) seam while that final decode runs; audio keeps buffering.
    pub fn streaming_feed(samples: &[f32]) -> Result<Option<String>> {
        let mut guard = WINDOW.lock();
        let win = guard
            .as_mut()
            .ok_or_else(|| anyhow!("qwen3 streaming not started"))?;

        let partial = win
            .engine
            .qwen3_streaming_feed(samples)
            .map_err(|e| anyhow!("qwen3_streaming_feed: {e}"))?;
        win.samples_in_window += samples.len();

        let at_pause = frame_rms(samples) < WINDOW_SILENCE_RMS;
        let should_roll = win.samples_in_window >= WINDOW_HARD_SAMPLES
            || (win.samples_in_window >= WINDOW_TARGET_SAMPLES && at_pause);

        if should_roll {
            // Close this ~25-30 s window, commit its transcript, open a fresh
            // session. Each window decodes independently, so the 512-token
            // cache never overflows no matter how long the user dictates.
            let text = win
                .engine
                .qwen3_streaming_finish()
                .map_err(|e| anyhow!("qwen3_streaming_finish (roll): {e}"))?;
            append_segment(&mut win.committed, &text);
            win.engine
                .qwen3_streaming_start(
                    win.lang.as_deref(),
                    super::STREAM_MIN_AUDIO_SECONDS,
                    super::STREAM_CHUNK_SECONDS,
                    super::STREAM_MAX_SECONDS,
                )
                .map_err(|e| anyhow!("qwen3_streaming_start (roll): {e}"))?;
            win.samples_in_window = 0;
            return Ok(Some(win.committed.clone()));
        }

        match partial {
            Some(p) => Ok(Some(compose(&win.committed, &p))),
            // No new partial this tick — committed unchanged, nothing to emit.
            None => Ok(None),
        }
    }

    /// Finish the current window and return the authoritative stitched final
    /// transcript (all committed windows + the last session's tail). Clears the
    /// window state; call `streaming_start` again for a new recording.
    pub fn streaming_finish() -> Result<String> {
        let mut win = WINDOW
            .lock()
            .take()
            .ok_or_else(|| anyhow!("qwen3 streaming not started"))?;
        // Finish the open session and append its tail. If the final session
        // errors — e.g. a roll just opened a fresh (possibly empty) session, or
        // a roll's start() failed leaving a closed session — DON'T discard the
        // windows already stitched into `committed`; return them. Propagating
        // the error here would drop the entire long-audio transcript.
        match win.engine.qwen3_streaming_finish() {
            Ok(tail) => append_segment(&mut win.committed, tail.trim()),
            Err(e) => log::error!("[vibeking local_qwen3] streaming_finish tail dropped: {e}"),
        }
        Ok(win.committed.trim().to_string())
    }

    /// Drop the in-process handle and remove **only** the Qwen3 model dir from
    /// the FluidAudio cache (leaving Parakeet's weights intact).
    pub fn clear_engine_cache() -> Result<()> {
        *ENGINE.lock() = None;
        let mut removed_any = false;
        for path in model_dirs() {
            if path.exists() {
                std::fs::remove_dir_all(&path)
                    .map_err(|e| anyhow!("remove {}: {e}", path.display()))?;
                removed_any = true;
            }
        }
        if !removed_any {
            log::info!("[vibeking local_qwen3] clear_engine_cache: no on-disk Qwen3 cache found");
        }
        Ok(())
    }

    /// Load the Qwen3 CoreML engine into memory ahead of the first dictation so
    /// the initial transcribe doesn't pay the JIT-compile + ANE-load cost.
    /// Idempotent — `ensure_engine` returns the cached instance if already warm.
    ///
    /// Also primes the **streaming** graph (`init_qwen3_streaming`). That graph
    /// is otherwise compiled lazily inside the first `streaming_start`, putting
    /// its one-time cost on the user's first hotkey press; doing it here (off
    /// the audio thread, guarded so it's idempotent) means the first live
    /// dictation hits an already-compiled graph.
    pub async fn warm_engine() -> Result<()> {
        let engine = ensure_engine().await?;
        tauri::async_runtime::spawn_blocking(move || -> Result<()> {
            if !engine.is_qwen3_streaming_available() {
                engine
                    .init_qwen3_streaming()
                    .map_err(|e| anyhow!("init_qwen3_streaming (warm): {e}"))?;
            }
            Ok(())
        })
        .await
        .map_err(|e| anyhow!("qwen3 streaming warm join: {e}"))?
    }

    async fn ensure_engine() -> Result<Arc<FluidAudio>> {
        if let Some(existing) = ENGINE.lock().clone() {
            return Ok(existing);
        }
        let initialized = tauri::async_runtime::spawn_blocking(|| -> Result<FluidAudio> {
            let audio = FluidAudio::new().map_err(|e| anyhow!("FluidAudio::new failed: {e}"))?;
            audio
                .init_qwen3_asr()
                .map_err(|e| anyhow!("FluidAudio::init_qwen3_asr failed: {e}"))?;
            Ok(audio)
        })
        .await
        .map_err(|e| anyhow!("qwen3 init join: {e}"))??;

        let arc = Arc::new(initialized);
        let mut slot = ENGINE.lock();
        if let Some(existing) = slot.clone() {
            Ok(existing)
        } else {
            *slot = Some(arc.clone());
            Ok(arc)
        }
    }
}
