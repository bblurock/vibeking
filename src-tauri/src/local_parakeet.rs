//! Local STT via FluidAudio (Parakeet TDT v3) on the Apple Neural Engine.
//!
//! macOS-14+ / Apple Silicon path. First initialization downloads ~500 MB
//! of CoreML weights from HuggingFace and JIT-compiles them for the ANE
//! (~20-30 s wall time). Subsequent loads of the same compiled package
//! take ~1 s, so we cache the warm `FluidAudio` instance behind a
//! `parking_lot::Mutex` for the lifetime of the process.
//!
//! ## Biasing
//!
//! Per the Phase 0 spike on the FluidAudio biasing API: `fluidaudio-rs`
//! 0.14.1 exposes no `initial_prompt` / vocabulary / forced-token primitive
//! on its batch ASR path. Hotword + screen-context biasing here is
//! therefore **post-hoc substitution, not acoustic** — see [`apply_bias`].
//! Swap that one function out if/when the upstream Swift package's
//! `configureVocabularyBoosting` lands on the Rust bridge.

#[cfg(not(target_os = "macos"))]
pub use noop::*;

#[cfg(target_os = "macos")]
pub use platform::*;

/// Identifier used by the model-management UI / Tauri commands.
pub const ENGINE_ID: &str = "parakeet-tdt-v3";

/// Short hint surfaced in settings UI explaining how biasing differs from
/// the Whisper engine. Kept here so the frontend can read it via a Tauri
/// command later if we want; for now Phase 2 may hardcode it in TS.
#[allow(dead_code)] // consumed by Phase 2 frontend work
pub const BIAS_MODE_HINT: &str = "post-hoc substitution, not acoustic";

/// One decoded token with its `[start, end]` time in seconds, relative to the
/// start of the audio slice it was decoded from. Returned by
/// [`preview_transcribe_timed`] for the live-preview frozen-head/live-tail
/// stitcher. The `token` string is the raw SentencePiece piece (a leading
/// `▁` marks a word boundary); join pieces and replace `▁` with a space to
/// reconstruct text.
#[derive(Debug, Clone)]
pub struct PreviewToken {
    pub token: String,
    pub start: f32,
    pub end: f32,
}

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
        Err(anyhow!("Parakeet engine only available on macOS"))
    }

    pub async fn transcribe(
        _wav: &[u8],
        _lang: Option<&str>,
        _hotwords: &[String],
        _context_candidates: &[String],
    ) -> Result<String> {
        Err(anyhow!("Parakeet engine only available on macOS"))
    }

    pub async fn warm_engine() -> Result<()> {
        Err(anyhow!("Parakeet engine only available on macOS"))
    }

    pub async fn streaming_start() -> Result<()> {
        Err(anyhow!("Parakeet engine only available on macOS"))
    }

    pub fn streaming_feed(_samples: &[f32]) -> Result<Option<String>> {
        Err(anyhow!("Parakeet engine only available on macOS"))
    }

    pub fn streaming_finish() -> Result<String> {
        Err(anyhow!("Parakeet engine only available on macOS"))
    }

    pub async fn preview_transcribe_timed(
        _samples_16k: Vec<f32>,
    ) -> Result<Vec<super::PreviewToken>> {
        Err(anyhow!("Parakeet engine only available on macOS"))
    }

    pub fn clear_engine_cache() -> Result<()> {
        Err(anyhow!("Parakeet engine only available on macOS"))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use anyhow::{anyhow, Context, Result};
    use fluidaudio_rs::FluidAudio;
    use parking_lot::Mutex;

    use super::PreviewToken;
    use crate::audio_io::decode_wav_to_mono_f32;
    use crate::stt::merge_bias_terms;

    /// Cap on combined hotword + context candidates considered for post-hoc
    /// substitution. Generous on purpose — fuzzy matching is O(tokens *
    /// terms), and the token count of a 5-30 s dictation clip is typically
    /// <200, so even 200 bias terms stays sub-millisecond.
    const BIAS_TERM_CAP: usize = 200;

    /// Jaro-Winkler similarity threshold above which a Parakeet output
    /// token will be substituted with its matching bias term. Tight on
    /// purpose: false substitutions for common words ("can" → "Cann") are
    /// far worse than missing a rare proper noun.
    const SIMILARITY_THRESHOLD: f64 = 0.85;

    /// Minimum token length before we consider it for substitution. Very
    /// short tokens (1-3 chars) produce too many false-positive matches
    /// against short bias terms ("Al" → "Ali" type collisions).
    const MIN_TOKEN_LEN_FOR_FUZZY: usize = 4;

    static ENGINE: Mutex<Option<Arc<FluidAudio>>> = Mutex::new(None);

    /// True while `prepare_engine` is mid-download/compile. Survives
    /// frontend page reloads (in-memory tracking on the JS side is wiped
    /// by Cmd+R; this flag isn't, because the Rust process keeps running).
    /// Consumed by `model_status({ engine: "parakeet" })` so the
    /// `ParakeetEngineRow` can show "Preparing…" even after a refresh.
    static PREPARING: AtomicBool = AtomicBool::new(false);

    /// "Ready" from the user's perspective means "no 500 MB download
    /// stands between me and dictating". Two cases satisfy that:
    ///   1. The in-process engine handle is already cached (transcribe
    ///      will return in milliseconds).
    ///   2. The model files are present on disk, even if the handle
    ///      hasn't been warm-loaded yet. The first transcribe will pay
    ///      a ~1 s warm-load, but no download is needed.
    /// Reporting case 2 as ready avoids the "Prepare engine" button
    /// flashing on a fresh app launch with a populated cache.
    pub fn engine_ready() -> bool {
        if ENGINE.lock().is_some() {
            return true;
        }
        current_cache_size_bytes() >= COMPLETED_CACHE_THRESHOLD_BYTES
    }

    /// Minimum on-disk bytes treated as "model fully downloaded". Tight
    /// enough to reject partial downloads (a few MB of metadata that
    /// FluidAudio writes early) but loose enough to be robust against
    /// minor size variation between releases.
    const COMPLETED_CACHE_THRESHOLD_BYTES: u64 = 400 * 1024 * 1024;

    pub fn is_preparing() -> bool {
        PREPARING.load(Ordering::Relaxed)
    }

    /// RAII guard so the PREPARING flag is cleared even if `prepare_engine`
    /// panics or returns an error mid-flight.
    struct PreparingGuard;
    impl Drop for PreparingGuard {
        fn drop(&mut self) {
            PREPARING.store(false, Ordering::Relaxed);
        }
    }

    /// Approximate total on-disk size of Parakeet TDT v3 after download
    /// completes (~500 MB across the compiled CoreML bundles + tokenizer).
    /// Used as the denominator for the synthetic progress bar we derive
    /// from polling the cache directory while `fluidaudio-rs` downloads
    /// the model (the upstream API gives us no real progress stream).
    const EXPECTED_MODEL_SIZE_BYTES: u64 = 500 * 1024 * 1024;

    /// Candidate roots that the upstream Swift package has historically
    /// written models into. Same list `clear_engine_cache` walks.
    fn model_cache_roots() -> Vec<std::path::PathBuf> {
        let Some(home) = dirs::home_dir() else {
            return Vec::new();
        };
        vec![
            home.join("Library/Application Support/FluidAudio"),
            home.join("Library/Application Support/FluidInference"),
            home.join("Library/Caches/FluidAudio"),
            home.join("Library/Caches/com.fluidinference.FluidAudio"),
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
        model_cache_roots().iter().map(|p| dir_size(p)).sum()
    }

    /// Download + ANE-compile the Parakeet TDT v3 models and cache the
    /// resulting warm instance. Idempotent: a second call returns early.
    ///
    /// `on_progress` is invoked periodically with `(downloaded_bytes,
    /// Some(EXPECTED_MODEL_SIZE_BYTES))`. The numbers come from polling
    /// the on-disk cache directory every 500 ms — `fluidaudio-rs` 0.14.1
    /// exposes no real progress callback, but we get a reasonable bar
    /// out of watching the filesystem. After the bytes plateau we're in
    /// the ANE-compile phase; the final tick clamps to 100 % once
    /// `ensure_engine` returns.
    pub async fn prepare_engine(
        on_progress: impl Fn(u64, Option<u64>) + Send + Sync + 'static,
    ) -> Result<()> {
        use std::sync::atomic::AtomicBool;

        PREPARING.store(true, Ordering::Relaxed);
        let _guard = PreparingGuard;

        let cb: Arc<dyn Fn(u64, Option<u64>) + Send + Sync> = Arc::new(on_progress);
        let total = Some(EXPECTED_MODEL_SIZE_BYTES);

        // Emit the current on-disk size immediately so the bar reflects
        // any partial cache from a previous prep attempt rather than
        // flashing 0% then jumping forward.
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
                // Stay under 100% while ensure_engine is still running —
                // we save the final tick for the success path so the UI
                // doesn't flash "done" while ANE compile is still in
                // flight.
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

    /// Transcribe a WAV blob using the cached Parakeet instance.
    ///
    /// `lang` is accepted for signature parity with `local_stt::transcribe`
    /// but is **not** forwarded — Parakeet TDT v3 auto-detects across its
    /// 25-language model and `fluidaudio-rs`'s batch `transcribe_samples`
    /// takes no language argument. (Qwen3 path does, but it requires
    /// macOS 15+ and is a separate model.)
    /// Length of silence (in samples at 16 kHz) prepended to every PCM
    /// buffer before it hits Parakeet. The TDT v3 model has built-in VAD
    /// that needs a moment of silence to calibrate before locking onto
    /// speech — without this pad the first 200-400 ms of dictation is
    /// classified as noise and trimmed off the transcript. 500 ms is a
    /// comfortable margin while keeping the extra cost negligible
    /// (8000 × f32 ≈ 32 KB, processed in <1 ms on ANE).
    const PARAKEET_LEADING_SILENCE_SAMPLES: usize = 8000;

    pub async fn transcribe(
        wav: &[u8],
        lang: Option<&str>,
        hotwords: &[String],
        context_candidates: &[String],
    ) -> Result<String> {
        let _ = lang; // see doc comment above
        let raw_pcm = decode_wav_to_mono_f32(wav)?;
        // Prepend silence so Parakeet's VAD has a clean calibration
        // window before the user's speech starts. See the constant's
        // doc comment for why this is needed.
        let mut pcm = Vec::with_capacity(PARAKEET_LEADING_SILENCE_SAMPLES + raw_pcm.len());
        pcm.resize(PARAKEET_LEADING_SILENCE_SAMPLES, 0.0);
        pcm.extend_from_slice(&raw_pcm);
        let bias_terms = merge_bias_terms(hotwords, context_candidates, BIAS_TERM_CAP);

        if bias_terms.is_empty() {
            log::info!("[vibeking local_parakeet] bias_terms=<none>");
        } else {
            let preview: Vec<&str> = bias_terms.iter().take(10).map(String::as_str).collect();
            let more = bias_terms.len().saturating_sub(preview.len());
            log::info!(
                "[vibeking local_parakeet] bias_terms ({}) [{}{}]",
                bias_terms.len(),
                preview.join(", "),
                if more > 0 {
                    format!(", +{} more", more)
                } else {
                    String::new()
                }
            );
        }

        let engine = ensure_engine().await?;

        let raw = tauri::async_runtime::spawn_blocking(move || -> Result<String> {
            let result = engine
                .transcribe_samples(&pcm)
                .map_err(|e| anyhow!("parakeet transcribe: {e}"))?;
            Ok(result.text)
        })
        .await
        .map_err(|e| anyhow!("parakeet join: {e}"))??;

        Ok(apply_bias(&sanitize_output(&raw), &bias_terms))
    }

    /// Trim Parakeet's idiosyncratic edge artifacts. The TDT v3 model
    /// often emits a leading sentence-ending punctuation mark (period,
    /// 句号, comma) when the audio opens with a brief silence, yielding
    /// outputs like `". hello world"` or `"。你好"`. whisper.cpp gets
    /// trimmed via its segment iterator naturally; Parakeet does not.
    ///
    /// Strip leading whitespace, then peel off at most one leading
    /// punctuation char (and any whitespace after it). Conservative: we
    /// only touch the *first* character so legitimate "...interesting"
    /// or "...so anyway" can't survive without the model also producing
    /// further leading punctuation (which it doesn't). Trailing
    /// whitespace also trimmed, leading parity with whisper.cpp.
    fn sanitize_output(s: &str) -> String {
        let trimmed = s.trim();
        let mut chars = trimmed.chars();
        let Some(first) = chars.next() else {
            return String::new();
        };
        // Sentence-final punctuation that Parakeet may stutter at the
        // very start. ASCII + CJK forms covered.
        const STRIP: &[char] = &[
            '.', ',', '!', '?', ';', ':', '。', '，', '、', '！', '？', '；', '：',
        ];
        if STRIP.contains(&first) {
            chars.as_str().trim_start().to_string()
        } else {
            trimmed.to_string()
        }
    }

    /// Drop the in-process engine handle and attempt to remove FluidAudio's
    /// on-disk model cache so the next `prepare_engine` does a fresh
    /// download. Path coverage is best-effort across the locations the
    /// upstream Swift package has historically used — extra paths are no-op
    /// if absent.
    pub fn clear_engine_cache() -> Result<()> {
        *ENGINE.lock() = None;

        let home = dirs::home_dir().ok_or_else(|| anyhow!("could not resolve home dir"))?;
        let candidates = [
            home.join("Library/Application Support/FluidAudio"),
            home.join("Library/Application Support/FluidInference"),
            home.join("Library/Caches/FluidAudio"),
            home.join("Library/Caches/com.fluidinference.FluidAudio"),
        ];

        let mut removed_any = false;
        for path in candidates.iter() {
            if path.exists() {
                std::fs::remove_dir_all(path)
                    .with_context(|| format!("remove {}", path.display()))?;
                removed_any = true;
            }
        }
        if !removed_any {
            log::info!(
                "[vibeking local_parakeet] clear_engine_cache: no on-disk FluidAudio cache found"
            );
        }
        Ok(())
    }

    /// Load the Parakeet engine into memory ahead of the first dictation so the
    /// initial transcribe doesn't pay the ANE-compile cost. Idempotent —
    /// `ensure_engine` returns the cached instance if already warm.
    pub async fn warm_engine() -> Result<()> {
        ensure_engine().await.map(|_| ())
    }

    // ===================== Streaming (live dictation preview) =====================
    //
    // FluidAudio's `SlidingWindowAsrManager` maintains its OWN confirmed-prefix +
    // volatile-tail window internally, so — unlike Qwen3 — we don't roll windows
    // here; just start / feed / finish. Recordings are serialized (one hotkey
    // session at a time), so the cached engine's single streaming session is
    // uncontended. Callers feed 16 kHz mono f32 frames during recording.

    /// Begin a streaming session, lazily initializing the streaming models.
    /// Resets any dangling session left by a rapid-fire previous recording.
    pub async fn streaming_start() -> Result<()> {
        let engine = ensure_engine().await?;
        tauri::async_runtime::spawn_blocking(move || -> Result<()> {
            if !engine.is_streaming_asr_available() {
                engine
                    .init_streaming_asr()
                    .map_err(|e| anyhow!("init_streaming_asr: {e}"))?;
            }
            // The API requires finish() between sessions; ignore "nothing to finish".
            let _ = engine.streaming_asr_finish();
            engine
                .streaming_asr_start()
                .map_err(|e| anyhow!("streaming_asr_start: {e}"))
        })
        .await
        .map_err(|e| anyhow!("parakeet stream start join: {e}"))?
    }

    /// Feed a frame of 16 kHz mono f32 samples. Returns the running transcript
    /// (confirmed prefix + live volatile tail) when there's an update, else None.
    pub fn streaming_feed(samples: &[f32]) -> Result<Option<String>> {
        let engine = ENGINE
            .lock()
            .clone()
            .ok_or_else(|| anyhow!("parakeet streaming not started"))?;
        engine
            .streaming_asr_feed(samples)
            .map_err(|e| anyhow!("streaming_asr_feed: {e}"))
    }

    /// Finish the streaming session (cleanly closing it). The authoritative final
    /// is the batch decode with bias, so this result is only used to tidy up.
    pub fn streaming_finish() -> Result<String> {
        let engine = ENGINE
            .lock()
            .clone()
            .ok_or_else(|| anyhow!("parakeet streaming not started"))?;
        engine
            .streaming_asr_finish()
            .map_err(|e| anyhow!("streaming_asr_finish: {e}"))
    }

    /// Live-preview decode-on-demand. Re-transcribes the whole accumulated
    /// 16 kHz mono buffer with the fast batch decoder (RTF ~0.02) and returns
    /// the running transcript.
    ///
    /// Why not the streaming `SlidingWindowAsrManager`? That API is built for
    /// long-form streaming: it emits nothing until a full chunk+right-context
    /// window (seconds) has decoded, so short dictation — the common case —
    /// showed no live text at all. Parakeet's batch decode, by contrast, is
    /// cheap enough to re-run every tick and produces text from the first ~0.5 s
    /// for any clip length (it's the same call that produces the authoritative
    /// final). Preview only — no hotword/context bias (applied to the final).
    ///
    /// Each token carries its `[start, end]` time in seconds, RELATIVE to the
    /// start of `samples_16k` — the live-preview stitcher uses these to freeze a
    /// head and keep only the most recent ~second as a self-correcting tail. We
    /// deliberately do NOT prepend the VAD-calibration silence here (the final
    /// path does), because that would shift every timing and break the mapping.
    pub async fn preview_transcribe_timed(mut samples_16k: Vec<f32>) -> Result<Vec<PreviewToken>> {
        let engine = ENGINE
            .lock()
            .clone()
            .ok_or_else(|| anyhow!("parakeet engine not loaded"))?;

        // Lift a quiet-but-real capture to a workable level, mirroring the final
        // path's `normalize_capture_gain` (target −12 dBFS). No voice gate: this
        // is a non-authoritative preview, so amplifying a little noise is fine.
        let peak = samples_16k.iter().fold(0.0_f32, |a, b| a.max(b.abs()));
        if peak > 0.0 && peak < 0.08 {
            let gain = (0.25 / peak).min(64.0);
            if gain > 1.0 {
                for s in samples_16k.iter_mut() {
                    *s = (*s * gain).clamp(-1.0, 1.0);
                }
            }
        }

        let json = tauri::async_runtime::spawn_blocking(move || -> Result<String> {
            let (_text, json) = engine
                .transcribe_samples_timed(&samples_16k)
                .map_err(|e| anyhow!("parakeet timed transcribe: {e}"))?;
            Ok(json)
        })
        .await
        .map_err(|e| anyhow!("parakeet preview join: {e}"))??;

        #[derive(serde::Deserialize)]
        struct TimedTokenJson {
            t: String,
            s: f64,
            e: f64,
        }
        let parsed: Vec<TimedTokenJson> = serde_json::from_str(&json).unwrap_or_default();
        Ok(parsed
            .into_iter()
            .map(|j| PreviewToken {
                token: j.t,
                start: j.s as f32,
                end: j.e as f32,
            })
            .collect())
    }

    async fn ensure_engine() -> Result<Arc<FluidAudio>> {
        if let Some(existing) = ENGINE.lock().clone() {
            return Ok(existing);
        }
        // Initialization is sync + blocking (model download, ANE compile).
        // Run on the blocking pool so we don't stall the tokio runtime.
        let initialized = tauri::async_runtime::spawn_blocking(|| -> Result<FluidAudio> {
            let audio = FluidAudio::new().map_err(|e| anyhow!("FluidAudio::new failed: {e}"))?;
            audio
                .init_asr()
                .map_err(|e| anyhow!("FluidAudio::init_asr failed: {e}"))?;
            Ok(audio)
        })
        .await
        .map_err(|e| anyhow!("parakeet init join: {e}"))??;

        let arc = Arc::new(initialized);
        // Two threads racing `ensure_engine` could each compile the model;
        // keep whichever wrote first.
        let mut slot = ENGINE.lock();
        if let Some(existing) = slot.clone() {
            Ok(existing)
        } else {
            *slot = Some(arc.clone());
            Ok(arc)
        }
    }

    /// Post-hoc fuzzy substitution. See module-level doc.
    ///
    /// Algorithm:
    /// 1. Tokenize the Parakeet output on whitespace, preserving the
    ///    original separators so we can rebuild the string verbatim.
    /// 2. For each token long enough to be a serious candidate (>=4
    ///    chars), strip its leading/trailing punctuation, lowercase it,
    ///    and Jaro-Winkler-score it against every bias term.
    /// 3. Substitute only when the **single** best score clears the
    ///    threshold and no other bias term is within 0.03 of it (to
    ///    avoid ambiguous swaps).
    /// 4. Preserve the bias term's case; re-attach the original
    ///    punctuation.
    ///
    /// Pulled out as a free function so a future `vocabulary_rescore`
    /// bridge can swap in with no caller changes.
    pub fn apply_bias(text: &str, bias_terms: &[String]) -> String {
        if bias_terms.is_empty() || text.is_empty() {
            return text.to_string();
        }

        // Pre-lowercase bias terms once.
        let lowered: Vec<(String, &str)> = bias_terms
            .iter()
            .map(|t| (t.to_lowercase(), t.as_str()))
            .collect();

        let mut out = String::with_capacity(text.len());
        for token in split_keep_delimiters(text) {
            if is_whitespace_only(&token) {
                out.push_str(&token);
                continue;
            }
            let (lead, core, trail) = strip_edge_punct(&token);
            if core.chars().count() < MIN_TOKEN_LEN_FOR_FUZZY {
                out.push_str(&token);
                continue;
            }

            // Don't touch tokens that are already an exact case-insensitive
            // match for some bias term — the model already nailed it.
            let core_lower = core.to_lowercase();
            if lowered.iter().any(|(lo, _)| lo == &core_lower) {
                out.push_str(lead);
                // Replace with canonical casing of the bias term.
                let canonical = lowered
                    .iter()
                    .find(|(lo, _)| lo == &core_lower)
                    .map(|(_, orig)| *orig)
                    .unwrap_or(core);
                out.push_str(canonical);
                out.push_str(trail);
                continue;
            }

            // Fuzzy match.
            let mut best_score = 0.0_f64;
            let mut second_best = 0.0_f64;
            let mut best_term: Option<&str> = None;
            for (lo, orig) in lowered.iter() {
                let score = strsim::jaro_winkler(&core_lower, lo);
                if score > best_score {
                    second_best = best_score;
                    best_score = score;
                    best_term = Some(*orig);
                } else if score > second_best {
                    second_best = score;
                }
            }

            // Require clear winner + above absolute threshold.
            if best_score >= SIMILARITY_THRESHOLD && (best_score - second_best) >= 0.03 {
                if let Some(term) = best_term {
                    out.push_str(lead);
                    out.push_str(term);
                    out.push_str(trail);
                    continue;
                }
            }
            out.push_str(&token);
        }
        out
    }

    /// Whitespace-aware tokenizer that yields both word tokens and the
    /// whitespace runs between them, so the caller can reassemble the
    /// original string by concatenation.
    fn split_keep_delimiters(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut buf = String::new();
        let mut in_ws: Option<bool> = None;
        for c in s.chars() {
            let ws = c.is_whitespace();
            match in_ws {
                None => {
                    in_ws = Some(ws);
                    buf.push(c);
                }
                Some(prev) if prev == ws => {
                    buf.push(c);
                }
                Some(_) => {
                    out.push(std::mem::take(&mut buf));
                    in_ws = Some(ws);
                    buf.push(c);
                }
            }
        }
        if !buf.is_empty() {
            out.push(buf);
        }
        out
    }

    fn is_whitespace_only(s: &str) -> bool {
        !s.is_empty() && s.chars().all(char::is_whitespace)
    }

    /// Split a token into (leading punctuation, core, trailing punctuation)
    /// using a generous "anything not alphanumeric" rule. Returns string
    /// slices into the input so we can reattach cheaply.
    fn strip_edge_punct(token: &str) -> (&str, &str, &str) {
        let bytes_len = token.len();
        let mut start = 0;
        for (i, c) in token.char_indices() {
            if c.is_alphanumeric() {
                start = i;
                break;
            }
            start = i + c.len_utf8();
        }
        if start == bytes_len {
            // All punctuation; nothing to do.
            return (token, "", "");
        }
        let mut end = bytes_len;
        for (i, c) in token.char_indices().rev() {
            if c.is_alphanumeric() {
                end = i + c.len_utf8();
                break;
            }
            end = i;
        }
        (&token[..start], &token[start..end], &token[end..])
    }

    #[cfg(test)]
    mod bias_tests {
        use super::*;

        fn s(strs: &[&str]) -> Vec<String> {
            strs.iter().map(|s| s.to_string()).collect()
        }

        #[test]
        fn no_bias_terms_passes_text_through() {
            assert_eq!(apply_bias("hello world", &[]), "hello world");
        }

        #[test]
        fn empty_text_passes_through() {
            assert_eq!(apply_bias("", &s(&["Karpathy"])), "");
        }

        #[test]
        fn exact_case_insensitive_match_normalizes_case() {
            let out = apply_bias("i love karpathy", &s(&["Karpathy"]));
            assert_eq!(out, "i love Karpathy");
        }

        #[test]
        fn near_match_above_threshold_substitutes() {
            // "carpathy" → "Karpathy" should clear the JW threshold.
            let out = apply_bias("i love carpathy", &s(&["Karpathy"]));
            assert!(out.contains("Karpathy"), "got: {out}");
        }

        #[test]
        fn dissimilar_token_is_left_alone() {
            // "hello" vs "Karpathy" should not match.
            let out = apply_bias("hello world", &s(&["Karpathy"]));
            assert_eq!(out, "hello world");
        }

        #[test]
        fn punctuation_is_preserved() {
            let out = apply_bias("(carpathy),", &s(&["Karpathy"]));
            assert_eq!(out, "(Karpathy),");
        }

        #[test]
        fn short_tokens_are_not_fuzzy_matched() {
            // "al" is below MIN_TOKEN_LEN_FOR_FUZZY; should not become "Ali".
            let out = apply_bias("al said hi", &s(&["Ali"]));
            assert_eq!(out, "al said hi");
        }

        #[test]
        fn ambiguous_matches_are_skipped() {
            // Two bias terms equally close to the token → don't substitute.
            let out = apply_bias("xarpathy", &s(&["Karpathy", "Marpathy"]));
            assert_eq!(out, "xarpathy");
        }
    }

    #[cfg(test)]
    mod sanitize_tests {
        use super::*;

        #[test]
        fn passes_clean_text_through() {
            assert_eq!(sanitize_output("hello world"), "hello world");
            assert_eq!(sanitize_output("你好世界"), "你好世界");
        }

        #[test]
        fn strips_leading_ascii_punctuation() {
            assert_eq!(sanitize_output(". hello world"), "hello world");
            assert_eq!(sanitize_output(", hello"), "hello");
            assert_eq!(sanitize_output("! hello"), "hello");
        }

        #[test]
        fn strips_leading_cjk_punctuation() {
            assert_eq!(sanitize_output("。你好"), "你好");
            assert_eq!(sanitize_output("，世界"), "世界");
            assert_eq!(sanitize_output("、 测试"), "测试");
        }

        #[test]
        fn strips_only_one_leading_punctuation() {
            // Conservative: only the first char is stripped, so legitimate
            // "..." or "?!" mid-content survives if it isn't the first artifact.
            assert_eq!(sanitize_output("..hello"), ".hello");
        }

        #[test]
        fn trims_outer_whitespace() {
            assert_eq!(sanitize_output("   hello   "), "hello");
            assert_eq!(sanitize_output("\n\t hello \t\n"), "hello");
        }

        #[test]
        fn preserves_internal_punctuation() {
            assert_eq!(
                sanitize_output("hello, world. how are you?"),
                "hello, world. how are you?"
            );
        }

        #[test]
        fn empty_and_whitespace_only() {
            assert_eq!(sanitize_output(""), "");
            assert_eq!(sanitize_output("   "), "");
            assert_eq!(sanitize_output("."), "");
        }
    }
}
