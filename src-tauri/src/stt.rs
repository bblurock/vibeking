//! Speech-to-text cloud provider registry.
//!
//! Provider pattern adapted from EpicenterHQ/epicenter (MIT) —
//! `apps/whispering/src/lib/services/transcription/registry.ts`.

use anyhow::{anyhow, Result};
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    Deepgram,
    Groq,
    Openai,
    Elevenlabs,
    Local,
    FluidAudio,
    /// Local Qwen3-ASR (CoreML/ANE). User-selectable CJK + multilingual engine
    /// with live streaming. Serialized as "qwen3".
    Qwen3,
    /// Local Gemma 4 audio via an app-managed MLX sidecar (Python server).
    /// Clip-based (no streaming). Serialized as "gemma".
    Gemma,
}

#[derive(Debug, Clone)]
pub struct TranscribeInput<'a> {
    pub wav: &'a [u8],
    pub api_key: &'a str,
    pub language: Option<&'a str>,
    pub hotwords: &'a [String],
    /// Proper-noun candidates extracted from the focused window at recording start.
    /// Merged with `hotwords` for cloud providers' biasing parameter (Deepgram
    /// `keyterm`, Groq/OpenAI `prompt`), and passed to local Whisper as
    /// `initial_prompt` bias.
    pub context_candidates: &'a [String],
    pub model: Option<&'a str>,
}

/// De-duplicate (case-insensitive) and cap a merged hotword + screen-context
/// list. Hotwords come first so they survive truncation when the merged list
/// exceeds `max_total`.
pub fn merge_bias_terms(hotwords: &[String], context: &[String], max_total: usize) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(max_total.min(hotwords.len() + context.len()));
    for w in hotwords.iter().chain(context.iter()) {
        let trimmed = w.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = trimmed.to_lowercase();
        if seen.insert(key) {
            out.push(trimmed.to_string());
            if out.len() >= max_total {
                break;
            }
        }
    }
    out
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscribeOutput {
    pub text: String,
    pub provider: Provider,
    pub model: String,
}

pub async fn transcribe(
    provider: Provider,
    input: TranscribeInput<'_>,
) -> Result<TranscribeOutput> {
    match provider {
        Provider::Deepgram => deepgram::transcribe(input).await,
        Provider::Groq => groq::transcribe(input).await,
        Provider::Openai => openai::transcribe(input).await,
        Provider::Elevenlabs => elevenlabs::transcribe(input).await,
        Provider::Local => local::transcribe(input).await,
        Provider::FluidAudio => fluid::transcribe(input).await,
        Provider::Qwen3 => qwen3::transcribe(input).await,
        Provider::Gemma => gemma::transcribe(input).await,
    }
}

/// Streaming transcribe for the Gemma sidecar: POSTs with SSE enabled and calls
/// `on_partial` with the cumulative transcript after each token delta, so the
/// live preview can paint tokens as they decode. Gemma-specific (the only HTTP
/// engine that exposes token streaming); Qwen3 streams via its own native API.
pub async fn transcribe_gemma_streaming(
    input: TranscribeInput<'_>,
    max_tokens: u32,
    on_partial: impl Fn(&str),
) -> Result<TranscribeOutput> {
    gemma::transcribe_streaming(input, max_tokens, on_partial).await
}

/// EXPERIMENTAL streaming session API (Settings.gemma_streaming). Open a session
/// for a recording; returns (port, session_id).
pub async fn gemma_session_start(
    model: Option<&str>,
    language: Option<&str>,
) -> Result<(u16, String)> {
    gemma::session_start(model, language).await
}

/// Feed the NEW 16 kHz mono f32 chunk; returns the current partial transcript.
pub async fn gemma_session_feed(port: u16, id: &str, pcm16k: &[f32]) -> Result<String> {
    gemma::session_feed(port, id, pcm16k).await
}

/// Final authoritative transcript; drops the session.
pub async fn gemma_session_finish(port: u16, id: &str) -> Result<String> {
    gemma::session_finish(port, id).await
}

/// Drop a session without finishing it.
pub async fn gemma_session_cancel(port: u16, id: &str) {
    gemma::session_cancel(port, id).await
}

/// True for local engines whose model + sidecar must be loaded into memory
/// before the first dictation feels instant. Cloud providers (always reachable)
/// and Whisper (loaded fresh per transcribe, so warming wouldn't persist) return
/// false — the warming HUD never appears for them.
pub fn provider_needs_warm(provider: Provider) -> bool {
    matches!(
        provider,
        Provider::Qwen3 | Provider::FluidAudio | Provider::Gemma | Provider::Local
    )
}

/// Eagerly load a local engine's model + sidecar into memory so the next
/// recording transcribes without cold-start latency. Mirrors [`transcribe`]'s
/// provider dispatch. Each underlying call is idempotent (early-returns when the
/// model is already resident), so repeated warms are cheap. Cloud providers and
/// Whisper are no-ops (see [`provider_needs_warm`]).
pub async fn warm_engine(
    provider: Provider,
    model: Option<&str>,
    gemma_streaming: bool,
) -> Result<()> {
    match provider {
        Provider::Qwen3 => qwen3::warm().await,
        Provider::FluidAudio => fluid::warm().await,
        Provider::Gemma => gemma::warm(model, gemma_streaming).await,
        // Whisper now caches its loaded context (see local_stt), so warming
        // pre-loads the model once at app-open / engine-switch — keeping the
        // first recording's live preview from cold-loading mid-recording.
        Provider::Local => local::warm(model).await,
        // Cloud providers need no local load.
        Provider::Deepgram | Provider::Groq | Provider::Openai | Provider::Elevenlabs => Ok(()),
    }
}

mod gemma {
    use super::*;
    use crate::gemma_server;
    use base64::Engine as _;

    #[derive(Deserialize)]
    struct ChatResponse {
        choices: Vec<Choice>,
    }
    #[derive(Deserialize)]
    struct Choice {
        message: ChoiceMessage,
    }
    #[derive(Deserialize)]
    struct ChoiceMessage {
        content: String,
    }

    // SSE streaming chunk shape: an OpenAI `chat.completion.chunk` carrying an
    // incremental `choices[0].delta.content`. The first chunk is role-only
    // (no content) and the terminal marker is a bare `[DONE]` line.
    #[derive(Deserialize)]
    struct StreamChunk {
        #[serde(default)]
        choices: Vec<StreamChoice>,
    }
    #[derive(Deserialize)]
    struct StreamChoice {
        delta: StreamDelta,
    }
    #[derive(Deserialize)]
    struct StreamDelta {
        #[serde(default)]
        content: Option<String>,
    }

    /// Shared keep-alive client for the loopback MLX sidecar. The windowed live
    /// preview fires many short requests per dictation; a per-call
    /// `Client::new()` re-handshakes each time, so reuse one connection pool.
    fn client() -> &'static reqwest::Client {
        static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
        CLIENT.get_or_init(reqwest::Client::new)
    }

    /// Parse one SSE line into its token delta, if any. Pure + synchronous so
    /// it's unit-testable. Returns `None` for blank lines, SSE comments/events,
    /// the `[DONE]` terminator, role-only deltas, and garbled JSON.
    fn parse_sse_delta(line: &str) -> Option<String> {
        let data = line.trim().strip_prefix("data:")?.trim();
        if data.is_empty() || data == "[DONE]" {
            return None;
        }
        let parsed = serde_json::from_str::<StreamChunk>(data).ok()?;
        let content = parsed.choices.into_iter().next()?.delta.content?;
        (!content.is_empty()).then_some(content)
    }

    /// Map an STT language code to its English name for the prompt. Gemma is an
    /// instruction-tuned LLM, so "Chinese" biases far more reliably than the
    /// bare ISO code "zh" (and codes like "yue"/"uk"/"nl" may not register at
    /// all). Covers Gemma's audio language set (GEMMA_LANGS in stt-languages.ts)
    /// plus a couple of neighbours; unknown codes fall through to the code.
    fn language_name(code: &str) -> &str {
        match code {
            "en" => "English",
            "zh" => "Chinese",
            "yue" => "Cantonese",
            "ja" => "Japanese",
            "ko" => "Korean",
            "de" => "German",
            "fr" => "French",
            "es" => "Spanish",
            "pt" => "Portuguese",
            "it" => "Italian",
            "nl" => "Dutch",
            "ru" => "Russian",
            "ar" => "Arabic",
            "hi" => "Hindi",
            "id" => "Indonesian",
            "vi" => "Vietnamese",
            "th" => "Thai",
            "tr" => "Turkish",
            "pl" => "Polish",
            "uk" => "Ukrainian",
            "cs" => "Czech",
            "el" => "Greek",
            "hu" => "Hungarian",
            "ro" => "Romanian",
            "sv" => "Swedish",
            "da" => "Danish",
            "fi" => "Finnish",
            "he" => "Hebrew",
            "fa" => "Persian",
            "ta" => "Tamil",
            "te" => "Telugu",
            "bn" => "Bengali",
            "ml" => "Malayalam",
            "sw" => "Swahili",
            other => other,
        }
    }

    /// Build the transcription instruction from the selected language. Shared by
    /// the whole-clip chat path and the streaming session path.
    fn instruction_for(language: Option<&str>) -> String {
        match language.filter(|l| *l != "auto") {
            Some(code) => {
                // Use the language NAME, not the bare ISO code — Gemma is an LLM.
                let name = language_name(code);
                format!(
                    "Transcribe the following speech in {name}. Output only the verbatim {name} transcription — no preamble, no translation, no commentary.",
                )
            }
            // Auto-detect: Google's official ASR pattern uses "its original
            // language" (not a named language) — hardened here against the
            // documented failure modes (translating to English, switching
            // script) for an ambiguous/short prompt.
            None => "Transcribe the following speech segment in its original language into text in that same language. Do not translate. Output only the verbatim transcription in the exact language and script that is spoken, with no preamble and no newlines.".to_string(),
        }
    }

    // --- EXPERIMENTAL streaming session client (Settings.gemma_streaming) ---
    // Drives gemma_stream_server.py: one session per recording, holding a
    // persistent KV cache. `feed` sends the NEW 16 kHz mono f32 chunk and returns
    // the current full transcript; `finish` returns the authoritative final.

    #[derive(Deserialize)]
    struct SessionStartResp {
        session_id: String,
    }
    #[derive(Deserialize)]
    struct SessionTextResp {
        #[serde(default)]
        text: String,
    }

    /// Open a streaming session; returns (port, session_id). Ensures the
    /// streaming sidecar is hot for `model`.
    pub async fn session_start(
        model: Option<&str>,
        language: Option<&str>,
    ) -> Result<(u16, String)> {
        let model = model
            .filter(|m| !m.is_empty())
            .unwrap_or(gemma_server::DEFAULT_MODEL)
            .to_string();
        let port = gemma_server::ensure_streaming(&model).await?;
        let body = serde_json::json!({ "instruction": instruction_for(language) });
        let url = format!("http://127.0.0.1:{port}/session/start");
        let res = client().post(&url).json(&body).send().await?;
        if !res.status().is_success() {
            return Err(anyhow!("gemma session start: HTTP {}", res.status()));
        }
        let parsed: SessionStartResp = res.json().await?;
        Ok((port, parsed.session_id))
    }

    /// Feed the NEW audio chunk (16 kHz mono f32) and get the current partial.
    pub async fn session_feed(port: u16, id: &str, pcm16k: &[f32]) -> Result<String> {
        let mut bytes = Vec::with_capacity(pcm16k.len() * 4);
        for s in pcm16k {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let url = format!("http://127.0.0.1:{port}/session/{id}/feed");
        let res = client()
            .post(&url)
            .header("Content-Type", "application/octet-stream")
            .body(bytes)
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(anyhow!("gemma session feed: HTTP {}", res.status()));
        }
        let parsed: SessionTextResp = res.json().await?;
        Ok(parsed.text.trim().to_string())
    }

    /// Final authoritative transcript; drops the session.
    pub async fn session_finish(port: u16, id: &str) -> Result<String> {
        let url = format!("http://127.0.0.1:{port}/session/{id}/finish");
        let res = client().post(&url).send().await?;
        if !res.status().is_success() {
            return Err(anyhow!("gemma session finish: HTTP {}", res.status()));
        }
        let parsed: SessionTextResp = res.json().await?;
        Ok(parsed.text.trim().to_string())
    }

    /// Drop a session without finishing (e.g. a superseded recording).
    pub async fn session_cancel(port: u16, id: &str) {
        let url = format!("http://127.0.0.1:{port}/session/{id}/cancel");
        let _ = client().post(&url).send().await;
    }

    /// Resolve the model, ensure the sidecar is hot, and build the OpenAI-style
    /// chat `messages` array (language instruction + the `input_audio` part —
    /// the shape `mlx_vlm.server` accepts; there's no dedicated /transcribe
    /// endpoint). Shared by the blocking and streaming transcribe paths.
    async fn prepare(input: &TranscribeInput<'_>) -> Result<(String, u16, serde_json::Value)> {
        let model = input
            .model
            .filter(|m| !m.is_empty())
            .unwrap_or(gemma_server::DEFAULT_MODEL)
            .to_string();

        // Spawn / reuse the MLX sidecar and wait for it to be healthy.
        let port = gemma_server::ensure_running(&model).await?;

        let instruction = instruction_for(input.language);
        let audio_b64 = base64::engine::general_purpose::STANDARD.encode(input.wav);
        let messages = serde_json::json!([{
            "role": "user",
            "content": [
                { "type": "text", "text": instruction },
                { "type": "input_audio",
                  "input_audio": { "data": audio_b64, "format": "wav" } }
            ]
        }]);
        Ok((model, port, messages))
    }

    pub async fn transcribe(input: TranscribeInput<'_>) -> Result<TranscribeOutput> {
        let (model, port, messages) = prepare(&input).await?;
        let body = serde_json::json!({
            "model": model,
            "max_tokens": 1024,
            "temperature": 0.0,
            "messages": messages,
        });

        let url = format!("http://127.0.0.1:{port}/v1/chat/completions");
        let res = client().post(&url).json(&body).send().await?;
        if !res.status().is_success() {
            return Err(anyhow!(
                "gemma: HTTP {}: {}",
                res.status(),
                res.text().await.unwrap_or_default()
            ));
        }
        let parsed: ChatResponse = res.json().await?;
        let text = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| anyhow!("gemma: empty response"))?;
        Ok(TranscribeOutput {
            text: text.trim().to_string(),
            provider: Provider::Gemma,
            model,
        })
    }

    /// Streaming sibling of [`transcribe`]: POSTs with `"stream": true` and
    /// consumes the OpenAI-style SSE response, calling `on_partial` with the
    /// cumulative transcript after every token delta so the live preview can
    /// paint tokens as they decode instead of waiting for the whole window.
    /// Returns the full transcript as the window's final.
    ///
    /// Bytes are buffered and split on `\n` so a multi-byte UTF-8 char (CJK)
    /// straddling two network chunks is never decoded mid-codepoint.
    pub async fn transcribe_streaming(
        input: TranscribeInput<'_>,
        max_tokens: u32,
        on_partial: impl Fn(&str),
    ) -> Result<TranscribeOutput> {
        let (model, port, messages) = prepare(&input).await?;
        let body = serde_json::json!({
            "model": model,
            "max_tokens": max_tokens,
            "temperature": 0.0,
            "stream": true,
            "messages": messages,
        });

        let url = format!("http://127.0.0.1:{port}/v1/chat/completions");
        let mut res = client().post(&url).json(&body).send().await?;
        if !res.status().is_success() {
            return Err(anyhow!(
                "gemma stream: HTTP {}: {}",
                res.status(),
                res.text().await.unwrap_or_default()
            ));
        }

        let mut buf: Vec<u8> = Vec::new();
        let mut cumulative = String::new();
        while let Some(chunk) = res.chunk().await? {
            buf.extend_from_slice(&chunk);
            while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=pos).collect();
                let line = std::str::from_utf8(&line[..line.len() - 1]).unwrap_or("");
                if let Some(delta) = parse_sse_delta(line) {
                    cumulative.push_str(&delta);
                    on_partial(cumulative.trim());
                }
            }
        }

        Ok(TranscribeOutput {
            text: cumulative.trim().to_string(),
            provider: Provider::Gemma,
            model,
        })
    }

    /// Spawn / reuse the MLX sidecar and wait until the model is healthy, so the
    /// next dictation skips the multi-GB cold start. Idempotent — `ensure_running`
    /// returns immediately when a server for this model is already hot.
    pub async fn warm(model: Option<&str>, streaming: bool) -> Result<()> {
        let model = model
            .filter(|m| !m.is_empty())
            .unwrap_or(gemma_server::DEFAULT_MODEL);
        if streaming {
            gemma_server::ensure_streaming(model).await.map(|_| ())
        } else {
            gemma_server::ensure_running(model).await.map(|_| ())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::parse_sse_delta;

        #[test]
        fn extracts_content_delta() {
            let line = r#"data: {"choices":[{"delta":{"content":"你好"}}]}"#;
            assert_eq!(parse_sse_delta(line).as_deref(), Some("你好"));
        }

        #[test]
        fn ignores_done_blank_and_role_only() {
            assert_eq!(parse_sse_delta("data: [DONE]"), None);
            assert_eq!(parse_sse_delta(""), None);
            assert_eq!(parse_sse_delta(": keep-alive comment"), None);
            // First chunk is role-only with no content field.
            assert_eq!(
                parse_sse_delta(r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#),
                None
            );
        }

        #[test]
        fn skips_garbled_and_empty_content() {
            assert_eq!(parse_sse_delta("data: not json"), None);
            assert_eq!(
                parse_sse_delta(r#"data: {"choices":[{"delta":{"content":""}}]}"#),
                None
            );
        }
    }
}

mod qwen3 {
    use super::*;
    use crate::local_qwen3;

    pub async fn transcribe(input: TranscribeInput<'_>) -> Result<TranscribeOutput> {
        // Pass the selected language straight through (Qwen3 accepts ISO codes
        // like "zh", "yue", "ja", "ko", "en"); "auto" → None for auto-detect.
        let lang = input.language.filter(|l| *l != "auto");
        let text =
            local_qwen3::transcribe(input.wav, lang, input.hotwords, input.context_candidates)
                .await?;
        Ok(TranscribeOutput {
            text,
            provider: Provider::Qwen3,
            model: local_qwen3::ENGINE_ID.to_string(),
        })
    }

    /// Initialize the CoreML/ANE engine ahead of the first dictation. Idempotent
    /// — returns immediately once the engine is cached.
    pub async fn warm() -> Result<()> {
        local_qwen3::warm_engine().await
    }
}

mod local {
    use super::*;
    use crate::local_stt;

    pub async fn transcribe(input: TranscribeInput<'_>) -> Result<TranscribeOutput> {
        let model_id: String = input
            .model
            .filter(|m| !m.is_empty())
            .map(|m| m.to_string())
            .unwrap_or_else(|| default_model().to_string());
        let text = local_stt::transcribe(
            input.wav,
            input.language,
            &model_id,
            input.hotwords,
            input.context_candidates,
        )
        .await?;
        Ok(TranscribeOutput {
            text,
            provider: Provider::Local,
            model: model_id,
        })
    }

    pub async fn warm(model: Option<&str>) -> Result<()> {
        let model_id: String = model
            .filter(|m| !m.is_empty())
            .map(|m| m.to_string())
            .unwrap_or_else(|| default_model().to_string());
        local_stt::warm(&model_id).await
    }

    #[cfg(target_os = "macos")]
    fn default_model() -> &'static str {
        local_stt::DEFAULT_MODEL_ID
    }

    #[cfg(not(target_os = "macos"))]
    fn default_model() -> &'static str {
        "unavailable"
    }
}

mod fluid {
    use super::*;
    use crate::local_parakeet;

    pub async fn transcribe(input: TranscribeInput<'_>) -> Result<TranscribeOutput> {
        let text = local_parakeet::transcribe(
            input.wav,
            input.language,
            input.hotwords,
            input.context_candidates,
        )
        .await?;
        Ok(TranscribeOutput {
            text,
            provider: Provider::FluidAudio,
            model: local_parakeet::ENGINE_ID.to_string(),
        })
    }

    /// Initialize the Parakeet (FluidAudio) engine ahead of the first dictation.
    /// Idempotent — returns immediately once the engine is cached.
    pub async fn warm() -> Result<()> {
        local_parakeet::warm_engine().await
    }
}

fn missing_key(provider: Provider) -> anyhow::Error {
    anyhow!("missing API key for provider {:?}", provider)
}

mod deepgram {
    use super::*;

    const ENDPOINT: &str = "https://api.deepgram.com/v1/listen";
    pub const DEFAULT_MODEL: &str = "nova-3";

    #[derive(Debug, Deserialize)]
    struct Response {
        results: Results,
    }
    #[derive(Debug, Deserialize)]
    struct Results {
        channels: Vec<Channel>,
    }
    #[derive(Debug, Deserialize)]
    struct Channel {
        alternatives: Vec<Alt>,
    }
    #[derive(Debug, Deserialize)]
    struct Alt {
        transcript: String,
    }

    pub async fn transcribe(input: TranscribeInput<'_>) -> Result<TranscribeOutput> {
        if input.api_key.is_empty() {
            return Err(missing_key(Provider::Deepgram));
        }
        let model = input.model.unwrap_or(DEFAULT_MODEL).to_string();
        let mut params: Vec<(&str, String)> = vec![
            ("model", model.clone()),
            ("smart_format", "true".into()),
            ("punctuate", "true".into()),
        ];
        match input.language {
            Some(lang) if lang != "auto" => {
                params.push(("language", lang.into()));
            }
            _ => {
                // For Nova-3, "multi" enables multilingual auto-detection.
                if model.contains("nova-3") {
                    params.push(("language", "multi".into()));
                }
            }
        }
        // Deepgram Nova-3 caps `keyterm` at 100 per request. Stay well under.
        let merged = super::merge_bias_terms(input.hotwords, input.context_candidates, 50);
        if !merged.is_empty() {
            let joined = merged.join(",");
            let key = if model.contains("nova-3") {
                "keyterm"
            } else {
                "keywords"
            };
            params.push((key, joined));
        }

        let client = reqwest::Client::new();
        let res = client
            .post(ENDPOINT)
            .query(&params)
            .header("Authorization", format!("Token {}", input.api_key))
            .header("Content-Type", "audio/wav")
            .body(input.wav.to_vec())
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(anyhow!(
                "deepgram: HTTP {}: {}",
                res.status(),
                res.text().await.unwrap_or_default()
            ));
        }
        let body: Response = res.json().await?;
        let text = body
            .results
            .channels
            .into_iter()
            .next()
            .and_then(|c| c.alternatives.into_iter().next())
            .map(|a| a.transcript)
            .ok_or_else(|| anyhow!("deepgram: no transcript in response"))?;
        Ok(TranscribeOutput {
            text,
            provider: Provider::Deepgram,
            model,
        })
    }
}

mod groq {
    use super::*;

    const ENDPOINT: &str = "https://api.groq.com/openai/v1/audio/transcriptions";
    pub const DEFAULT_MODEL: &str = "whisper-large-v3-turbo";

    #[derive(Debug, Deserialize)]
    struct Response {
        text: String,
    }

    pub async fn transcribe(input: TranscribeInput<'_>) -> Result<TranscribeOutput> {
        if input.api_key.is_empty() {
            return Err(missing_key(Provider::Groq));
        }
        let model = input.model.unwrap_or(DEFAULT_MODEL).to_string();
        let mut form = Form::new()
            .text("model", model.clone())
            .part(
                "file",
                Part::bytes(input.wav.to_vec())
                    .file_name("audio.wav")
                    .mime_str("audio/wav")?,
            )
            .text("response_format", "json");
        if let Some(lang) = input.language.filter(|l| *l != "auto") {
            form = form.text("language", lang.to_string());
        }
        // Whisper's `prompt` field has a ~224-token ceiling. 30 terms is a
        // safe upper bound (~3-4 tokens each on average).
        let merged = super::merge_bias_terms(input.hotwords, input.context_candidates, 30);
        if !merged.is_empty() {
            form = form.text(
                "prompt",
                format!(
                    "Hotwords / proper nouns that may appear: {}",
                    merged.join(", ")
                ),
            );
        }

        let client = reqwest::Client::new();
        let res = client
            .post(ENDPOINT)
            .bearer_auth(input.api_key)
            .multipart(form)
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(anyhow!(
                "groq: HTTP {}: {}",
                res.status(),
                res.text().await.unwrap_or_default()
            ));
        }
        let body: Response = res.json().await?;
        Ok(TranscribeOutput {
            text: body.text,
            provider: Provider::Groq,
            model,
        })
    }
}

mod openai {
    use super::*;

    const ENDPOINT: &str = "https://api.openai.com/v1/audio/transcriptions";
    pub const DEFAULT_MODEL: &str = "gpt-4o-transcribe";

    #[derive(Debug, Deserialize)]
    struct Response {
        text: String,
    }

    pub async fn transcribe(input: TranscribeInput<'_>) -> Result<TranscribeOutput> {
        if input.api_key.is_empty() {
            return Err(missing_key(Provider::Openai));
        }
        let model = input.model.unwrap_or(DEFAULT_MODEL).to_string();
        let mut form = Form::new()
            .text("model", model.clone())
            .part(
                "file",
                Part::bytes(input.wav.to_vec())
                    .file_name("audio.wav")
                    .mime_str("audio/wav")?,
            )
            .text("response_format", "json");
        if let Some(lang) = input.language.filter(|l| *l != "auto") {
            form = form.text("language", lang.to_string());
        }
        // OpenAI's `prompt` field has the same ~224-token ceiling as Groq.
        let merged = super::merge_bias_terms(input.hotwords, input.context_candidates, 30);
        if !merged.is_empty() {
            form = form.text(
                "prompt",
                format!(
                    "Hotwords / proper nouns that may appear: {}",
                    merged.join(", ")
                ),
            );
        }

        let client = reqwest::Client::new();
        let res = client
            .post(ENDPOINT)
            .bearer_auth(input.api_key)
            .multipart(form)
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(anyhow!(
                "openai: HTTP {}: {}",
                res.status(),
                res.text().await.unwrap_or_default()
            ));
        }
        let body: Response = res.json().await?;
        Ok(TranscribeOutput {
            text: body.text,
            provider: Provider::Openai,
            model,
        })
    }
}

mod elevenlabs {
    use super::*;

    const ENDPOINT: &str = "https://api.elevenlabs.io/v1/speech-to-text";
    pub const DEFAULT_MODEL: &str = "scribe_v2";

    #[derive(Debug, Deserialize)]
    struct Response {
        text: String,
    }

    pub async fn transcribe(input: TranscribeInput<'_>) -> Result<TranscribeOutput> {
        if input.api_key.is_empty() {
            return Err(missing_key(Provider::Elevenlabs));
        }
        let model = input.model.unwrap_or(DEFAULT_MODEL).to_string();
        let mut form = Form::new().text("model_id", model.clone()).part(
            "file",
            Part::bytes(input.wav.to_vec())
                .file_name("audio.wav")
                .mime_str("audio/wav")?,
        );
        if let Some(lang) = input.language.filter(|l| *l != "auto") {
            form = form.text("language_code", lang.to_string());
        }

        let client = reqwest::Client::new();
        let res = client
            .post(ENDPOINT)
            .header("xi-api-key", input.api_key)
            .multipart(form)
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(anyhow!(
                "elevenlabs: HTTP {}: {}",
                res.status(),
                res.text().await.unwrap_or_default()
            ));
        }
        let body: Response = res.json().await?;
        Ok(TranscribeOutput {
            text: body.text,
            provider: Provider::Elevenlabs,
            model,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn merge_empty_both_returns_empty() {
        let out = merge_bias_terms(&[], &[], 50);
        assert!(out.is_empty());
    }

    #[test]
    fn merge_hotwords_first_then_context() {
        let out = merge_bias_terms(&s(&["Claude", "Karpathy"]), &s(&["BeeBob"]), 50);
        assert_eq!(out, vec!["Claude", "Karpathy", "BeeBob"]);
    }

    #[test]
    fn merge_dedupes_case_insensitively_keeping_first() {
        let out = merge_bias_terms(&s(&["Claude"]), &s(&["claude", "BeeBob"]), 50);
        assert_eq!(out, vec!["Claude", "BeeBob"]);
    }

    #[test]
    fn merge_truncates_to_cap_preserving_hotword_priority() {
        let out = merge_bias_terms(&s(&["H1", "H2", "H3"]), &s(&["C1", "C2"]), 4);
        // Hotwords come first, then context — cap kicks in after 4.
        assert_eq!(out, vec!["H1", "H2", "H3", "C1"]);
    }

    #[test]
    fn merge_skips_empty_and_whitespace_only_entries() {
        let out = merge_bias_terms(&s(&["", "  Claude  "]), &s(&["\t", "Anthropic"]), 50);
        assert_eq!(out, vec!["Claude", "Anthropic"]);
    }
}
