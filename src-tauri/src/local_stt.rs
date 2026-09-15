//! Local STT via whisper.cpp (whisper-rs).
//!
//! macOS-only path. Uses Metal acceleration. Models are downloaded from
//! HuggingFace on first use and cached in the app data dir.

#[cfg(not(target_os = "macos"))]
pub use noop::*;

#[cfg(target_os = "macos")]
pub use platform::*;

#[cfg(not(target_os = "macos"))]
mod noop {
    use anyhow::{anyhow, Result};

    pub const DEFAULT_MODEL_ID: &str = "ggml-large-v3-turbo-q5_0";

    pub fn model_exists(_model_id: &str) -> bool {
        false
    }
    pub fn model_path(_id: &str) -> Result<std::path::PathBuf> {
        Err(anyhow!("local STT only available on macOS"))
    }
    pub async fn download_model(_id: &str) -> Result<std::path::PathBuf> {
        Err(anyhow!("local STT only available on macOS"))
    }
    pub async fn transcribe(
        _wav: &[u8],
        _lang: Option<&str>,
        _model_id: &str,
        _hotwords: &[String],
        _context_candidates: &[String],
    ) -> Result<String> {
        Err(anyhow!("local STT only available on macOS"))
    }
    pub async fn warm(_model_id: &str) -> Result<()> {
        Ok(())
    }
    pub async fn transcribe_with_prompt_budget(
        _wav: &[u8],
        _lang: Option<&str>,
        _model_id: &str,
        _hotwords: &[String],
        _context_candidates: &[String],
        _max_prompt_tokens: usize,
    ) -> Result<String> {
        Err(anyhow!("local STT only available on macOS"))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use anyhow::{anyhow, Context, Result};
    use futures_util::StreamExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use tokio::io::AsyncWriteExt;
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    use crate::audio_io::decode_wav_to_mono_f32;

    pub const DEFAULT_MODEL_ID: &str = "ggml-large-v3-turbo-q5_0";

    /// Cache of the loaded Whisper model, keyed by model id. The `WhisperContext`
    /// holds the heavy, read-only model weights (hundreds of MB) + its Metal
    /// buffers; only the per-decode `state` is cheap. Without this cache every
    /// `transcribe` call reloaded the entire model from disk — fine for the one
    /// batch decode on stop, but the live-preview loop fires a transcribe every
    /// ~700 ms DURING recording, so it was reloading large-v3-turbo from scratch
    /// on every preview pass. That's what froze the live card after a few words
    /// (and re-allocated Metal buffers each time — the same hazard as the Gemma
    /// Metal-cache leak). Reusing one warm context turns each pass into just a
    /// decode. Keyed by id so switching models transparently reloads.
    static WHISPER_CTX: Mutex<Option<(String, Arc<WhisperContext>)>> = Mutex::new(None);

    /// Return a shared, warm `WhisperContext` for `model_id`, loading it once and
    /// caching it for reuse. The lock is held across the (one-time) model load so
    /// concurrent first calls don't double-load; subsequent calls just clone the
    /// `Arc`. Called from inside `spawn_blocking`, so blocking on the load is fine.
    fn whisper_context(model_id: &str, path: &Path) -> Result<Arc<WhisperContext>> {
        let mut guard = WHISPER_CTX
            .lock()
            .map_err(|_| anyhow!("whisper context cache poisoned"))?;
        if let Some((id, ctx)) = guard.as_ref() {
            if id == model_id {
                return Ok(Arc::clone(ctx));
            }
        }
        log::info!("[vibeking local_stt] loading whisper model {model_id} into cache (reused across decodes)");
        let ctx = Arc::new(WhisperContext::new_with_params(
            path.to_str().ok_or_else(|| anyhow!("model path not UTF-8"))?,
            WhisperContextParameters::default(),
        )?);
        *guard = Some((model_id.to_string(), Arc::clone(&ctx)));
        Ok(ctx)
    }

    fn model_url(model_id: &str) -> String {
        format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{model_id}.bin?download=true"
        )
    }

    pub fn model_dir() -> Result<PathBuf> {
        let base = dirs::data_dir().ok_or_else(|| anyhow!("could not resolve user data dir"))?;
        let dir = base.join("Vibeking").join("models");
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create_dir_all {}", dir.display()))?;
        Ok(dir)
    }

    pub fn model_path(model_id: &str) -> Result<PathBuf> {
        Ok(model_dir()?.join(format!("{model_id}.bin")))
    }

    pub fn model_exists(model_id: &str) -> bool {
        model_path(model_id)
            .map(|p| p.exists() && std::fs::metadata(&p).map(|m| m.len() > 0).unwrap_or(false))
            .unwrap_or(false)
    }

    pub async fn download_model(
        model_id: &str,
        on_progress: impl Fn(u64, Option<u64>) + Send + 'static,
    ) -> Result<PathBuf> {
        let dest = model_path(model_id)?;
        if model_exists(model_id) {
            return Ok(dest);
        }
        let url = model_url(model_id);
        let res = reqwest::Client::new()
            .get(&url)
            .send()
            .await
            .with_context(|| format!("download {url}"))?;
        if !res.status().is_success() {
            return Err(anyhow!("download {url}: HTTP {}", res.status()));
        }
        let total = res.content_length();
        let tmp = dest.with_extension("download");
        let mut file = tokio::fs::File::create(&tmp).await?;
        let mut downloaded: u64 = 0;
        let mut stream = res.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            downloaded += chunk.len() as u64;
            file.write_all(&chunk).await?;
            on_progress(downloaded, total);
        }
        file.flush().await?;
        drop(file);
        tokio::fs::rename(&tmp, &dest).await?;
        Ok(dest)
    }

    /// Default cap on `initial_prompt` length, in approximate BPE tokens.
    /// whisper.cpp also caps internally at ~220 tokens, so this is conservative.
    pub const DEFAULT_PROMPT_TOKEN_BUDGET: usize = 150;

    pub async fn transcribe(
        wav: &[u8],
        lang: Option<&str>,
        model_id: &str,
        hotwords: &[String],
        context_candidates: &[String],
    ) -> Result<String> {
        transcribe_with_prompt_budget(
            wav,
            lang,
            model_id,
            hotwords,
            context_candidates,
            DEFAULT_PROMPT_TOKEN_BUDGET,
        )
        .await
    }

    /// Eagerly load the model into the shared context cache so the first
    /// recording — and its live-preview loop — reuses a warm context instead of
    /// cold-loading large-v3-turbo mid-recording. Idempotent: a no-op once the
    /// context for `model_id` is already cached.
    pub async fn warm(model_id: &str) -> Result<()> {
        let path = model_path(model_id)?;
        if !path.exists() {
            return Err(anyhow!("local model not downloaded: {}", path.display()));
        }
        let model_id = model_id.to_string();
        tauri::async_runtime::spawn_blocking(move || -> Result<()> {
            whisper_context(&model_id, &path).map(|_| ())
        })
        .await
        .map_err(|e| anyhow!("whisper warm join: {e}"))?
    }

    /// Like [`transcribe`] but lets the caller override the prompt-token budget.
    ///
    /// Used by the evaluation harness to sweep `initial_prompt` size and measure
    /// its effect on proper-noun WER. Production callers should use [`transcribe`].
    pub async fn transcribe_with_prompt_budget(
        wav: &[u8],
        lang: Option<&str>,
        model_id: &str,
        hotwords: &[String],
        context_candidates: &[String],
        max_prompt_tokens: usize,
    ) -> Result<String> {
        let path = model_path(model_id)?;
        if !path.exists() {
            return Err(anyhow!("local model not downloaded: {}", path.display()));
        }

        let mut pcm = decode_wav_to_mono_f32(wav)?;
        // whisper.cpp's mel-spectrogram path bails with
        // "input is too short — N ms < 1000 ms" when the PCM is under
        // a second at 16 kHz. The audio engine's leading-silence trim
        // (introduced for Bluetooth cold-start) can leave us just
        // below that threshold on short utterances. Pad with trailing
        // silence to a small margin above 1000 ms so whisper accepts
        // the input — silence at the tail doesn't influence the
        // transcript and the model returns to its idle state cleanly.
        const WHISPER_MIN_SAMPLES_16K: usize = 16_000; // 1000 ms
        const WHISPER_PAD_TARGET_16K: usize = 17_600; // 1100 ms (100 ms margin)
        if pcm.len() < WHISPER_MIN_SAMPLES_16K {
            let needed = WHISPER_PAD_TARGET_16K.saturating_sub(pcm.len());
            log::info!(
                "[vibeking local_stt] padding {} samples ({} ms) of trailing silence to meet whisper's 1000 ms minimum",
                needed,
                (needed * 1000) / 16_000,
            );
            pcm.resize(WHISPER_PAD_TARGET_16K, 0.0);
        }
        let lang = lang.unwrap_or("auto").to_string();
        let path = path.clone();
        let model_id = model_id.to_string();
        let initial_prompt =
            format_initial_prompt_with_budget(hotwords, context_candidates, max_prompt_tokens);

        if initial_prompt.is_empty() {
            log::info!("[vibeking local_stt] initial_prompt=<none>");
        } else {
            log::info!(
                "[vibeking local_stt] initial_prompt ({} chars): {}",
                initial_prompt.len(),
                initial_prompt
            );
        }

        tauri::async_runtime::spawn_blocking(move || -> Result<String> {
            let ctx = whisper_context(&model_id, &path)?;
            let mut state = ctx.create_state()?;
            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            params.set_n_threads(4);
            params.set_translate(false);
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            // We dictate utterances and only ever read the segment TEXT (never
            // timestamps). Leaving timestamps on triggers whisper.cpp's "single
            // timestamp ending - skip entire chunk" path: when greedy decoding ends
            // on a lone timestamp token at the very end of the audio (speech running
            // right up to the clip end, no trailing silence), it treats that
            // timestamp as a re-seek point, advances `seek` past the whole clip, and
            // emits ZERO segments — so a perfectly-decoded transcript ("okay this is
            // actually…") is silently dropped and the user sees nothing. Disabling
            // timestamp generation removes that code path entirely (whisper just
            // returns the decoded text) while still looping over multiple 30 s
            // windows, so long dictations aren't truncated.
            params.set_no_timestamps(true);
            if lang != "auto" {
                params.set_language(Some(&lang));
            }
            if !initial_prompt.is_empty() {
                params.set_initial_prompt(&initial_prompt);
            }
            state.full(params, &pcm)?;
            let n_segments = state.full_n_segments()?;
            let mut out = String::new();
            for i in 0..n_segments {
                if let Ok(seg) = state.full_get_segment_text(i) {
                    out.push_str(&seg);
                }
            }
            Ok(out.trim().to_string())
        })
        .await
        .map_err(|e| anyhow!("whisper join: {e}"))?
    }

    /// Build a Whisper `initial_prompt` from user hotwords + screen-derived candidates.
    ///
    /// - Sentence-style framing ("The following names may appear: X, Y, Z.") because
    ///   list-style prompts make Whisper over-commafy outputs.
    /// - Hotwords have priority over candidates on truncation.
    /// - Capped at the default ~150 BPE tokens (estimated via char-count).
    ///   whisper.cpp also caps internally at ~220 tokens, so this is conservative.
    #[cfg(test)]
    fn format_initial_prompt(hotwords: &[String], candidates: &[String]) -> String {
        format_initial_prompt_with_budget(hotwords, candidates, DEFAULT_PROMPT_TOKEN_BUDGET)
    }

    /// Budget-parameterized variant of [`format_initial_prompt`]. The harness
    /// uses this to sweep `max_prompt_tokens` ∈ {0, 50, 100, 150}.
    ///
    /// `max_prompt_tokens == 0` always returns an empty prompt (i.e. unbiased).
    fn format_initial_prompt_with_budget(
        hotwords: &[String],
        candidates: &[String],
        max_prompt_tokens: usize,
    ) -> String {
        if max_prompt_tokens == 0 {
            return String::new();
        }
        if hotwords.is_empty() && candidates.is_empty() {
            return String::new();
        }

        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut ordered: Vec<&str> = Vec::with_capacity(hotwords.len() + candidates.len());
        for h in hotwords {
            let key = h.to_lowercase();
            if seen.insert(key) {
                ordered.push(h.as_str());
            }
        }
        for c in candidates {
            let key = c.to_lowercase();
            if seen.insert(key) {
                ordered.push(c.as_str());
            }
        }

        const PREFIX: &str = "The following names may appear in the transcript: ";
        const SUFFIX: &str = ".";

        let mut out = String::from(PREFIX);
        let mut approx_tokens = approx_token_count(PREFIX) + approx_token_count(SUFFIX);
        let mut added = false;

        for tok in ordered {
            let segment_len = if added { tok.len() + 2 } else { tok.len() };
            let segment_tokens = (segment_len as f32 / 3.5).ceil() as usize;
            if approx_tokens + segment_tokens > max_prompt_tokens {
                break;
            }
            if added {
                out.push_str(", ");
            }
            out.push_str(tok);
            approx_tokens += segment_tokens;
            added = true;
        }

        if !added {
            return String::new();
        }
        out.push_str(SUFFIX);
        out
    }

    fn approx_token_count(s: &str) -> usize {
        // Whisper's BPE tokenizer averages ~3.5 chars/token for English prose.
        // Slightly overestimates for proper nouns (which split into more subwords),
        // which is the safe direction.
        (s.len() as f32 / 3.5).ceil() as usize
    }

    #[cfg(test)]
    mod prompt_tests {
        use super::*;

        #[test]
        fn empty_inputs_return_empty_prompt() {
            assert_eq!(format_initial_prompt(&[], &[]), "");
        }

        #[test]
        fn hotwords_only_form_a_sentence() {
            let p = format_initial_prompt(&["Katherine".into(), "Karpathy".into()], &[]);
            assert!(p.starts_with("The following names may appear in the transcript: "));
            assert!(p.contains("Katherine"));
            assert!(p.contains("Karpathy"));
            assert!(p.ends_with('.'));
        }

        #[test]
        fn hotwords_take_priority_over_candidates_on_dedupe() {
            let p = format_initial_prompt(
                &["Katherine".into()],
                &["katherine".into(), "Karpathy".into()],
            );
            // Only one "katherine" appears, preserving hotword casing.
            let lower_count = p.matches("atherine").count();
            assert_eq!(lower_count, 1);
            assert!(p.contains("Katherine"));
            assert!(p.contains("Karpathy"));
        }

        #[test]
        fn truncates_below_token_budget() {
            // Generate enough tokens to exceed budget.
            let many: Vec<String> = (0..200).map(|i| format!("Token{i:03}")).collect();
            let p = format_initial_prompt(&[], &many);
            assert!(p.len() < 1200, "prompt should be capped: was {}", p.len());
            assert!(p.ends_with('.'));
        }

        #[test]
        fn zero_budget_returns_empty_prompt() {
            // Budget of 0 always returns empty — used by the eval harness for
            // the "no biasing" baseline run in the token sweep.
            let p =
                format_initial_prompt_with_budget(&["Katherine".into()], &["Karpathy".into()], 0);
            assert_eq!(p, "");
        }

        #[test]
        fn smaller_budget_yields_shorter_or_equal_prompt() {
            // Monotonicity: 150-token budget produces a prompt at least as
            // long as a 50-token budget for the same inputs.
            let many: Vec<String> = (0..30).map(|i| format!("Person{i:02}")).collect();
            let small = format_initial_prompt_with_budget(&[], &many, 50);
            let large = format_initial_prompt_with_budget(&[], &many, 150);
            assert!(
                large.len() >= small.len(),
                "large={large:?} small={small:?}"
            );
            assert!(small.ends_with('.'));
            assert!(large.ends_with('.'));
        }
    }
}
