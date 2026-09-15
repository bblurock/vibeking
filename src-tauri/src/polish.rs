//! LLM polish + translation pass.
//!
//! Routes to one of:
//!   - Anthropic Messages API (Claude Haiku 4.5 default)
//!   - OpenAI-compatible /v1/chat/completions (Ollama, LM Studio, mlx-lm.server, custom)
//!
//! Local options keep transcripts on-device when paired with the Local STT path.

use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

const ANTHROPIC_ENDPOINT: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const ANTHROPIC_DEFAULT_MODEL: &str = "claude-haiku-4-5";

/// Connect timeout for refinement HTTP calls. A stopped or unreachable local
/// server (Ollama / MLX / LM Studio) must fail fast so the transcript paste
/// falls back to the raw text instead of hanging.
const POLISH_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Overall request timeout. Caps a slow/hung server so refinement can never
/// stall the recording pipeline indefinitely; on timeout `run()` returns an
/// `Err`, which the caller already falls back from to the raw transcript.
const POLISH_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// Shared HTTP client with timeouts for all refinement calls (polish, translate,
/// and model listing). Falls back to the default client if the builder fails,
/// which cannot happen with this static config but keeps the call infallible.
fn http_client() -> Client {
    Client::builder()
        .connect_timeout(POLISH_CONNECT_TIMEOUT)
        .timeout(POLISH_REQUEST_TIMEOUT)
        .build()
        .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PolishProvider {
    Anthropic,
    Ollama,
    LmStudio,
    MlxLm,
    Custom,
}

impl Default for PolishProvider {
    fn default() -> Self {
        PolishProvider::Anthropic
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Mode<'a> {
    Polish,
    Translate { target_language: &'a str },
}

#[derive(Debug, Clone)]
pub struct PolishConfig<'a> {
    pub provider: PolishProvider,
    pub anthropic_key: &'a str,
    pub base_url: &'a str,
    pub model: &'a str,
    pub api_key: &'a str,
    /// System prompt sourced from the active refinement mode. For
    /// `Mode::Translate` the caller has already substituted `{target}`.
    pub prompt: &'a str,
}

/// Static definition for one built-in refinement mode. Kept as a flat tuple
/// of `&'static str`s so the array can sit in const context; the runtime Vec
/// is built in `default_refinement_modes()`.
pub struct RefinementModeDefault {
    pub id: &'static str,
    pub name: &'static str,
    pub emoji: &'static str,
    pub prompt: &'static str,
}

/// Built-in refinement modes shipped with the app. The order here is the
/// initial cycle order (Polish first; Off is appended implicitly by the
/// cycle logic). Users can reorder or hide via the Quick switch setting.
pub const DEFAULT_REFINEMENT_MODES: &[RefinementModeDefault] = &[
    RefinementModeDefault {
        id: "polish",
        name: "Polish",
        emoji: "✨",
        prompt: "You polish raw voice-typed transcripts. Fix obvious disfluencies, \
                 repetition, casing, and punctuation. Preserve the speaker's voice and meaning. \
                 Output ONLY the polished text, no preamble.",
    },
    RefinementModeDefault {
        id: "email",
        name: "Email",
        emoji: "✉️",
        prompt: "You rewrite raw voice dictation into the body of a business email. \
                 \n\nRewrite rules: \
                 \n- Tone: professional and warm. Formalize casual fillers (\"wanna\" → \"would like to\", \"gonna\" → \"will\", \"hey\" → \"hi\", drop \"just\", \"basically\", \"like\"). Use complete, clear sentences. \
                 \n- Structure: insert blank-line paragraph breaks between distinct ideas. Even a 2-sentence dictation gets a line break between the greeting (if any) and the body. Do NOT produce a single wall of text. \
                 \n- Enumerations: when the speaker enumerates items (\"first... second... third...\", \"there are three things: A, B, C\", \"a few points: ...\"), reformat them as a markdown list — one item per line under a short intro sentence ending in a colon. Use numbered items (1. 2. 3.) when the speaker signaled order (\"first/second/third\"); use dash bullets (- ) otherwise. \
                 \n- Greeting: KEEP any greeting (\"Hi Benson\") the speaker actually said, verbatim on its own line. Do NOT invent or change the greeting if none was dictated. \
                 \n- Sign-off: ALWAYS end with a sign-off paragraph separated by a blank line. If the speaker dictated one (\"Best,\", \"Thanks,\", \"Cheers,\"), preserve it verbatim. Otherwise append exactly this closing on its own two lines: \"Best regards,\" then \"[Your name]\". Always use the literal placeholder \"[Your name]\" when inventing — never guess a sender name. \
                 \n- Fidelity: preserve all facts, names, numbers, dates, and the speaker's intent exactly. Never add commitments or content the speaker didn't dictate. \
                 \n\nOutput the email body only. No preamble, no surrounding quotes, no explanation.",
    },
    RefinementModeDefault {
        id: "code",
        name: "Code prompt",
        emoji: "💻",
        prompt: "You rewrite voice-typed dictation as a structured coding prompt suitable for an AI coding assistant. \
                 Organize the content into clear sections: Goal, Context, Constraints, and any acceptance criteria the \
                 speaker mentioned. Preserve technical terms, identifiers, file names, and library names exactly as \
                 spoken. Use code-fence markdown for any literal code or commands. Output ONLY the structured prompt, \
                 no preamble.",
    },
    RefinementModeDefault {
        id: "notes",
        name: "Notes",
        emoji: "📝",
        prompt: "You rewrite voice-typed dictation as well-formatted markdown notes. Add a short title as an H2 header \
                 if the speaker introduced a topic. Use bullet points for lists, **bold** for key terms the speaker \
                 emphasized, and paragraph breaks for natural shifts in topic. Preserve facts, numbers, and names exactly. \
                 Output ONLY the markdown notes, no preamble.",
    },
    RefinementModeDefault {
        id: "translate",
        name: "Translate",
        emoji: "🌐",
        prompt: "You translate voice-typed transcripts to {target}. \
                 Produce a natural, fluent translation. \
                 Preserve names, numbers, and technical terms. \
                 Output ONLY the translated text, no preamble.",
    },
];

pub fn default_refinement_modes() -> Vec<crate::state::RefinementMode> {
    DEFAULT_REFINEMENT_MODES
        .iter()
        .map(|d| crate::state::RefinementMode {
            id: d.id.to_string(),
            name: d.name.to_string(),
            emoji: d.emoji.to_string(),
            prompt: d.prompt.to_string(),
            builtin: true,
        })
        .collect()
}

pub async fn run(
    text: &str,
    hotwords: &[String],
    correction_dict: &[crate::state::CorrectionEntry],
    config: PolishConfig<'_>,
    mode: Mode<'_>,
) -> Result<String> {
    if text.trim().is_empty() {
        return Ok(String::new());
    }

    let (system, user) = build_prompt(text, hotwords, correction_dict, &config, mode);

    match config.provider {
        PolishProvider::Anthropic => anthropic(&system, &user, config.anthropic_key).await,
        _ => {
            openai_compat(
                &system,
                &user,
                config.base_url,
                config.model,
                config.api_key,
            )
            .await
        }
    }
}

async fn anthropic(system: &str, user: &str, api_key: &str) -> Result<String> {
    if api_key.is_empty() {
        return Err(anyhow!("missing Anthropic API key"));
    }

    #[derive(Serialize)]
    struct Req<'a> {
        model: &'a str,
        max_tokens: u32,
        system: &'a str,
        messages: Vec<Msg<'a>>,
    }
    #[derive(Serialize)]
    struct Msg<'a> {
        role: &'a str,
        content: &'a str,
    }
    #[derive(Deserialize)]
    struct Resp {
        content: Vec<ContentBlock>,
    }
    #[derive(Deserialize)]
    struct ContentBlock {
        text: Option<String>,
    }

    let req = Req {
        model: ANTHROPIC_DEFAULT_MODEL,
        max_tokens: 1024,
        system,
        messages: vec![Msg {
            role: "user",
            content: user,
        }],
    };

    let res = http_client()
        .post(ANTHROPIC_ENDPOINT)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&req)
        .send()
        .await?;
    if !res.status().is_success() {
        return Err(anyhow!(
            "anthropic: HTTP {}: {}",
            res.status(),
            res.text().await.unwrap_or_default()
        ));
    }
    let body: Resp = res.json().await?;
    let text = body
        .content
        .into_iter()
        .find_map(|b| b.text)
        .ok_or_else(|| anyhow!("anthropic: empty response"))?;
    Ok(strip_thinking(&text))
}

async fn openai_compat(
    system: &str,
    user: &str,
    base_url: &str,
    model: &str,
    api_key: &str,
) -> Result<String> {
    if base_url.is_empty() {
        return Err(anyhow!("missing base URL"));
    }
    if model.is_empty() {
        return Err(anyhow!("missing model name"));
    }

    #[derive(Deserialize)]
    struct Resp {
        choices: Vec<Choice>,
    }
    #[derive(Deserialize)]
    struct Choice {
        message: ChoiceMessage,
    }
    #[derive(Deserialize)]
    struct ChoiceMessage {
        content: Option<String>,
    }

    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    // chat_template_kwargs.enable_thinking=false is the canonical way to skip
    // reasoning on Qwen3 / DeepSeek R1 via mlx-lm.server, vLLM, and most local
    // OpenAI-compatible servers. The OpenAI API ignores unknown fields, so this
    // is safe to send unconditionally.
    let body = json!({
        "model": model,
        "max_tokens": 1024,
        "temperature": 0.3,
        "stream": false,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
        "chat_template_kwargs": { "enable_thinking": false },
    });

    let body_preview = serde_json::to_string(&body).unwrap_or_default();
    log::info!(
        "[vibeking polish] POST {url}  model={model}  auth={}",
        if api_key.is_empty() { "none" } else { "bearer" },
    );
    log::info!("[vibeking polish] request body: {body_preview}");

    let started = std::time::Instant::now();

    let mut req = http_client()
        .post(&url)
        .header("content-type", "application/json")
        .json(&body);
    if !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }

    let res = req.send().await.map_err(|e| {
        log::error!(
            "[vibeking polish] connect error after {:?}: {e}",
            started.elapsed()
        );
        anyhow!("polish: failed to reach {url} ({e}). Is the server running?")
    })?;
    let status = res.status();
    let elapsed = started.elapsed();
    let bytes = res.bytes().await.unwrap_or_default();
    let preview: String = String::from_utf8_lossy(&bytes).chars().take(800).collect();
    log::info!(
        "[vibeking polish] response status={status}  elapsed={:?}  bytes={}",
        elapsed,
        bytes.len()
    );
    log::info!("[vibeking polish] response body: {preview}");

    if !status.is_success() {
        return Err(anyhow!("polish: HTTP {}: {}", status, preview));
    }
    let parsed: Resp = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow!("polish: response parse failed ({e}): {preview}"))?;
    let text = parsed
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.message.content)
        .ok_or_else(|| anyhow!("polish: empty response"))?;
    Ok(strip_thinking(&text))
}

/// Parses an OpenAI-compatible `GET /v1/models` response body into a sorted,
/// de-duplicated list of model ids. Ollama, LM Studio, and mlx-lm.server all
/// return `{ "data": [ { "id": "..." }, ... ] }`.
fn parse_model_ids(bytes: &[u8]) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct ModelsResp {
        data: Vec<ModelEntry>,
    }
    #[derive(Deserialize)]
    struct ModelEntry {
        id: String,
    }
    let parsed: ModelsResp = serde_json::from_slice(bytes)
        .map_err(|e| anyhow!("list_polish_models: parse failed ({e})"))?;
    let mut ids: Vec<String> = parsed
        .data
        .into_iter()
        .map(|m| m.id)
        .filter(|id| !id.trim().is_empty())
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// Lists the models installed on a local OpenAI-compatible server so the
/// Settings UI can offer a dropdown instead of a free-text model field.
///
/// Returns `Err` (as a `String`, for the Tauri boundary) when the server is
/// unreachable or replies non-2xx — the frontend treats that as "couldn't
/// reach the server" and falls back to manual model entry.
#[tauri::command]
pub async fn list_polish_models(base_url: String, api_key: String) -> Result<Vec<String>, String> {
    if base_url.trim().is_empty() {
        return Err("missing base URL".to_string());
    }
    let url = format!("{}/models", base_url.trim_end_matches('/'));

    let mut req = http_client().get(&url);
    if !api_key.is_empty() {
        req = req.bearer_auth(&api_key);
    }
    let res = req
        .send()
        .await
        .map_err(|e| format!("failed to reach {url} ({e}). Is the server running?"))?;
    let status = res.status();
    let bytes = res.bytes().await.unwrap_or_default();
    if !status.is_success() {
        let preview: String = String::from_utf8_lossy(&bytes).chars().take(200).collect();
        return Err(format!("HTTP {status}: {preview}"));
    }
    parse_model_ids(&bytes).map_err(|e| e.to_string())
}

/// Removes `<think>...</think>` blocks emitted by reasoning models
/// (Qwen3, DeepSeek R1, etc.). Also handles an unclosed leading `<think>`
/// (some models stream the closing tag late).
fn strip_thinking(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        match rest.find("<think>") {
            None => {
                out.push_str(rest);
                break;
            }
            Some(start) => {
                out.push_str(&rest[..start]);
                let after_open = &rest[start + "<think>".len()..];
                match after_open.find("</think>") {
                    Some(end) => {
                        rest = &after_open[end + "</think>".len()..];
                    }
                    None => break, // unclosed; drop the rest
                }
            }
        }
    }
    out.trim().to_string()
}

fn build_prompt(
    text: &str,
    hotwords: &[String],
    correction_dict: &[crate::state::CorrectionEntry],
    config: &PolishConfig<'_>,
    mode: Mode<'_>,
) -> (String, String) {
    let hotword_clause = if hotwords.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nProper-noun / hot-word corrections to apply when applicable: {}",
            hotwords.join(", ")
        )
    };

    // Learned-correction clause: complements the pre-polish substitution pass
    // in `corrections::apply_dictionary`. The substitution catches exact
    // word-boundary matches; this clause gives the LLM the same mappings as
    // context for cases the substitution didn't catch (different casing,
    // punctuation-embedded forms, plurals).
    let correction_clause = if correction_dict.is_empty() {
        String::new()
    } else {
        let pairs = correction_dict
            .iter()
            .map(|e| format!("\"{}\" should be \"{}\"", e.from, e.to))
            .collect::<Vec<_>>()
            .join("; ");
        format!("\n\nLearned corrections: {pairs}.")
    };

    let base = match mode {
        Mode::Polish => config.prompt.to_string(),
        Mode::Translate { target_language } => config.prompt.replace("{target}", target_language),
    };

    let system = format!("{base}{hotword_clause}{correction_clause}");
    (system, format!("Transcript:\n{text}"))
}

#[cfg(test)]
mod model_list_tests {
    use super::parse_model_ids;

    #[test]
    fn parses_sorts_and_dedups_ids() {
        // Shape returned by Ollama / LM Studio / mlx-lm GET /v1/models.
        let body = br#"{
            "object": "list",
            "data": [
                { "id": "qwen2.5:3b-instruct", "object": "model" },
                { "id": "llama3.2:latest", "object": "model" },
                { "id": "qwen2.5:3b-instruct", "object": "model" }
            ]
        }"#;
        let ids = parse_model_ids(body).expect("should parse");
        assert_eq!(ids, vec!["llama3.2:latest", "qwen2.5:3b-instruct"]);
    }

    #[test]
    fn empty_data_yields_empty_list() {
        let ids = parse_model_ids(br#"{ "object": "list", "data": [] }"#).expect("should parse");
        assert!(ids.is_empty());
    }

    #[test]
    fn malformed_body_is_error() {
        assert!(parse_model_ids(b"not json").is_err());
    }
}
