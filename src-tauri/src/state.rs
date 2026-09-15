//! Per-app runtime state: active audio capture + settings cache.

use crate::audio::AudioEngine;
use crate::polish::PolishProvider;
use crate::stt::Provider;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tauri::Emitter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub provider: Provider,
    pub deepgram_key: String,
    pub groq_key: String,
    pub openai_key: String,
    pub elevenlabs_key: String,
    pub anthropic_key: String,
    pub language: String,
    #[serde(default = "default_stt_language")]
    pub stt_language: String,
    pub model: Option<String>,
    #[serde(default)]
    pub polish_provider: PolishProvider,
    #[serde(default = "default_polish_base_url")]
    pub polish_base_url: String,
    #[serde(default = "default_polish_model")]
    pub polish_model: String,
    #[serde(default)]
    pub polish_api_key: String,
    /// Per-provider memory of the base URL + model the user last configured,
    /// keyed by provider id ("ollama", "mlx-lm", "lm-studio", "custom"). Lets
    /// switching providers in Settings restore the user's customizations
    /// instead of clobbering them with the preset each time. The flat
    /// `polish_base_url`/`polish_model` above remain the *active* config the
    /// refinement call uses; this map is purely the UI's remembered state.
    /// The backend never reads it — it only round-trips for persistence.
    #[serde(default)]
    pub polish_provider_configs: HashMap<String, PolishProviderConfig>,
    /// User-editable list of refinement modes. Each mode pairs a name+emoji
    /// for the chip-bar badge with a system prompt the LLM runs against the
    /// raw transcript. Built-in modes (`builtin: true`) can be reset to
    /// defaults but not deleted. The mode with id `translate` uses
    /// `translate_target` to substitute `{target}` in its prompt.
    #[serde(default = "default_refinement_modes")]
    pub refinement_modes: Vec<RefinementMode>,
    /// Currently-active mode id, or `None` for "no refinement" (raw transcript
    /// is inserted as-is). Cycled by Shift+Tab during recording.
    #[serde(default = "default_active_refinement_mode_id")]
    pub active_refinement_mode_id: Option<String>,
    /// Subset of `refinement_modes[].id` that the Shift+Tab cycle walks
    /// through, in order. The "Off" sentinel is implicit and always sits at
    /// the end of the cycle.
    #[serde(default = "default_quick_switch_mode_ids")]
    pub quick_switch_mode_ids: Vec<String>,
    pub sound_fx: bool,
    pub hotwords: Vec<String>,
    pub translate_target: String,
    #[serde(default = "default_record_hotkey")]
    pub record_hotkey: String,
    #[serde(default)]
    pub mic_device: String,
    #[serde(default)]
    pub screen_context_mode: ScreenContextMode,
    #[serde(default)]
    pub correction_learning_mode: CorrectionLearningMode,
    #[serde(default)]
    pub correction_dictionary: Vec<CorrectionEntry>,
    /// Languages the user has favorited for the menubar quick-switch.
    /// "auto" is implicit — always shown first, never stored. The tray
    /// renders [auto, ...favorites] intersected with the active
    /// provider's supported set. See `tray::languages_for_provider`.
    #[serde(default = "default_favorite_stt_languages")]
    pub favorite_stt_languages: Vec<String>,
    /// When true, the chip bar stays visible as a compact "idle bar" the user
    /// can click to start dictation (instead of only the global hotkey). The
    /// window shrinks to hug the bar while idle so it doesn't block clicks to
    /// apps behind it; it expands to the full chip on recording. See
    /// `windows::reconcile_persistent_bar`.
    #[serde(default)]
    pub persistent_bar: bool,
    /// Route the Gemma live preview through the stateful streaming sidecar
    /// (KV-cache reuse) instead of the windowed whole-window re-decode. Now the
    /// DEFAULT (graduated from experimental); the Gemma branch spawns
    /// `gemma_stream_server.py` and uses the session API, and the authoritative
    /// final comes from `session/finish`. The non-streaming windowed path remains
    /// as a latent fallback. See resources/gemma_stream_server.py.
    #[serde(default = "default_true")]
    pub gemma_streaming: bool,
    /// Keep the Gemma MLX sidecar resident even when idle (skip the idle
    /// unload). Trades ~4-6 GB of RAM for an instant first recording + live
    /// preview. Only Gemma has an idle-unload; the other local engines stay
    /// resident once loaded.
    #[serde(default)]
    pub gemma_keep_loaded: bool,
    /// Minutes of idle before the Gemma sidecar is unloaded to free RAM, when
    /// `gemma_keep_loaded` is false. Default 15.
    #[serde(default = "default_gemma_idle_timeout_min")]
    pub gemma_idle_timeout_min: u64,
}

/// How Vibeking gathers the focused window's text for STT biasing.
///
/// AX and OCR are *alternatives*, not a fallback chain — the user picks
/// based on the trade-off. AX is fast (~5-50ms) and free (no extra
/// permission) but coverage varies by app. OCR works on anything visible
/// but is slower (~80-300ms on Apple Silicon, more on Intel) and requires
/// the Screen Recording permission.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ScreenContextMode {
    Off,
    #[default]
    Ax,
    Ocr,
}

/// User's preference for how Vibeking handles detected corrections.
///
/// Ask  — chip toast prompts Keep/Edit/Discard before storing
/// Auto — silently stores + shows a 2s success toast
/// Off  — detector not running; no dictionary mutation
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum CorrectionLearningMode {
    Off,
    #[default]
    Ask,
    Auto,
}

/// Remembered base URL + model for one refinement provider. Stored per
/// provider id in `Settings::polish_provider_configs` so switching providers
/// in Settings doesn't lose what the user previously set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolishProviderConfig {
    pub base_url: String,
    pub model: String,
}

/// A single learned (from → to) mapping in the user's dictionary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CorrectionEntry {
    pub from: String,
    pub to: String,
    /// Unix epoch milliseconds when this entry was learned.
    pub learned_at_ms: u64,
}

/// A named refinement preset the user can pick at recording time. The id
/// `translate` is special-cased by the polish pipeline to receive the
/// `translate_target` substitution; all other ids run as plain Polish-mode
/// requests with their `prompt` as the system message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefinementMode {
    pub id: String,
    pub name: String,
    pub emoji: String,
    pub prompt: String,
    pub builtin: bool,
}

fn default_record_hotkey() -> String {
    "right-control".to_string()
}

fn default_true() -> bool {
    true
}

fn default_stt_language() -> String {
    "auto".to_string()
}

/// Curated top-10 codes for the menubar quick-switch. "auto" is
/// implicit — always shown first by the tray, never stored. Mirrors
/// the legacy tray.rs `LANGUAGES` constant so existing users see no
/// behavioral change after the favorites refactor.
fn default_favorite_stt_languages() -> Vec<String> {
    ["zh", "en", "ja", "ko", "es", "fr", "de", "pt", "ru"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn default_polish_base_url() -> String {
    "http://localhost:11434/v1".to_string()
}

fn default_polish_model() -> String {
    "qwen2.5:3b-instruct".to_string()
}

fn default_refinement_modes() -> Vec<RefinementMode> {
    crate::polish::default_refinement_modes()
}

fn default_gemma_idle_timeout_min() -> u64 {
    15
}

fn default_active_refinement_mode_id() -> Option<String> {
    // Off by default. A freshly onboarded user may have no local LLM (Ollama /
    // MLX) installed and no Anthropic key, so refinement would be silently
    // non-functional. Refinement is opt-in via Settings; existing users keep
    // whatever they had persisted (this default only applies to a brand-new
    // profile with no stored value).
    None
}

fn default_quick_switch_mode_ids() -> Vec<String> {
    crate::polish::DEFAULT_REFINEMENT_MODES
        .iter()
        .map(|d| d.id.to_string())
        .collect()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: Provider::Deepgram,
            deepgram_key: String::new(),
            groq_key: String::new(),
            openai_key: String::new(),
            elevenlabs_key: String::new(),
            anthropic_key: String::new(),
            language: "zh".to_string(),
            stt_language: "auto".to_string(),
            model: None,
            polish_provider: PolishProvider::Anthropic,
            polish_base_url: default_polish_base_url(),
            polish_model: default_polish_model(),
            polish_api_key: String::new(),
            polish_provider_configs: HashMap::new(),
            refinement_modes: default_refinement_modes(),
            active_refinement_mode_id: default_active_refinement_mode_id(),
            quick_switch_mode_ids: default_quick_switch_mode_ids(),
            sound_fx: true,
            hotwords: Vec::new(),
            translate_target: "English".to_string(),
            record_hotkey: default_record_hotkey(),
            mic_device: String::new(),
            screen_context_mode: ScreenContextMode::default(),
            correction_learning_mode: CorrectionLearningMode::default(),
            correction_dictionary: Vec::new(),
            favorite_stt_languages: default_favorite_stt_languages(),
            persistent_bar: false,
            gemma_streaming: true,
            gemma_keep_loaded: false,
            gemma_idle_timeout_min: default_gemma_idle_timeout_min(),
        }
    }
}

/// Recording state machine, shared between the global-hotkey event tap
/// (`hotkey.rs`) and the click-to-record command (`ui_toggle_recording`) so a
/// recording started by one input method can be stopped/cancelled by the
/// other. Previously this lived privately inside the hotkey thread, which left
/// a UI click and the hotkey with diverging notions of "are we recording".
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum RecMode {
    #[default]
    Idle,
    /// Modifier held; not yet classified as push-to-talk vs quick-tap toggle.
    Pressed {
        since: std::time::Instant,
    },
    PushToTalk,
    Toggle,
}

impl Settings {
    pub fn api_key_for(&self, provider: Provider) -> &str {
        match provider {
            Provider::Deepgram => &self.deepgram_key,
            Provider::Groq => &self.groq_key,
            Provider::Openai => &self.openai_key,
            Provider::Elevenlabs => &self.elevenlabs_key,
            Provider::Local => "",
            Provider::FluidAudio => "",
            Provider::Qwen3 => "",
            Provider::Gemma => "",
        }
    }
}

#[derive(Default)]
pub struct AppState {
    /// Long-lived audio capture engine. Holds the cpal stream warm
    /// across recordings within the idle-keepalive window so rapid-fire
    /// recordings don't pay cold-start latency. See `audio.rs`.
    pub audio_engine: AudioEngine,
    pub settings: Mutex<Settings>,
    pub hotkey_capture_active: std::sync::atomic::AtomicBool,
    /// Single source of truth for the recording state machine. Driven by both
    /// the global hotkey (`hotkey.rs`) and the click-to-record command so the
    /// two input methods can't diverge. See [`RecMode`].
    pub recording_mode: Mutex<RecMode>,
    /// Proper-noun candidates captured from the focused window at recording:start.
    /// Taken (cleared) at recording:stop and passed to STT as initial_prompt bias.
    pub pending_context: Mutex<Option<Vec<String>>>,
    /// Generation counter incremented on every recording:start. Used to reject
    /// late-arriving screen captures from an earlier recording.
    pub context_generation: AtomicU64,
    /// Generation counter for the chip bar's post-transcript cleanup. Each
    /// recording:start bumps it; the async post-insert task captures the
    /// value at spawn time and only fires `hide_chipbar` if the counter
    /// still matches. Prevents the previous recording's 1.5 s "result
    /// hold" timer from racing in and hiding the chip bar of a
    /// just-started new recording.
    pub chipbar_generation: AtomicU64,
    /// Abort handle for the in-flight correction watcher, if any.
    /// Single-watcher invariant: spawning a new one cancels the previous —
    /// see `correction_watcher::spawn_watcher`. No observer leaks under
    /// rapid-fire dictation.
    pub correction_watcher_abort: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    /// Hand-off slot for the Qwen3 streaming session's authoritative final
    /// transcript, produced by the live-preview task on stop and consumed by
    /// the recording:stop handler. `(session, Some(text))` on success,
    /// `(session, None)` if the session finished empty/errored (→ batch
    /// fallback). The batch path overflows on long audio ("Qwen3 transcription
    /// failed"), so for Qwen3 the streaming final — which accumulates the whole
    /// recording — is the authoritative transcript instead.
    pub qwen3_stream_final: Mutex<Option<(u64, Option<String>)>>,
    /// Hand-off slot for the Gemma windowed-preview transcript, mirroring
    /// `qwen3_stream_final`. The Gemma preview windows audio into ≤8 s pieces and
    /// commits each at a pause, so its accumulated transcript covers the WHOLE
    /// recording — unlike the stop handler's single-shot whole-clip decode, which
    /// the Gemma audio encoder truncates at ~30 s. The preview task fills this on
    /// stop; the recording:stop handler consumes it as the authoritative final
    /// for long clips. `(session, Some(text))` on success, `(session, None)` if
    /// the windowed transcript was empty/unusable (→ whole-clip fallback).
    pub gemma_stream_final: Mutex<Option<(u64, Option<String>)>>,
    /// Generation counter for model-warming sessions. Each warm request (model
    /// switch, app open) bumps it; the spawned warm task captures the value and
    /// only emits its `model:warmed`/`model:warm-failed` result if the counter
    /// still matches — so a newer switch silently supersedes an in-flight warm
    /// instead of flashing a stale "ready" for the wrong model. See `warm.rs`.
    pub warming_generation: AtomicU64,
    /// The provider currently being warmed, or `None` when idle/done. A snapshot
    /// the chip bar queries on mount (`current_warming`) so an app-open warm that
    /// fired its `model:warming` event before the chipbar webview's listener was
    /// ready still shows the HUD. Set/cleared by `warm.rs`.
    pub warming_now: Mutex<Option<Provider>>,
}

pub type SharedState = Arc<AppState>;

#[tauri::command]
pub fn get_settings(state: tauri::State<'_, SharedState>) -> Settings {
    state.settings.lock().clone()
}

#[tauri::command]
pub fn set_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, SharedState>,
    settings: Settings,
) {
    let (prev_provider, prev_model, prev_dict_len) = {
        let s = state.settings.lock();
        (s.provider, s.model.clone(), s.correction_dictionary.len())
    };
    let new_dict_len = settings.correction_dictionary.len();
    if new_dict_len != prev_dict_len {
        log::info!(
            "[vibeking] set_settings — correction_dictionary {} -> {}",
            prev_dict_len,
            new_dict_len
        );
    }
    let model_switched = prev_provider != settings.provider || prev_model != settings.model;
    *state.settings.lock() = settings.clone();
    // Apply the Gemma memory policy live (0 secs = Keep-loaded / never unload).
    crate::gemma_server::set_idle_policy(if settings.gemma_keep_loaded {
        0
    } else {
        settings.gemma_idle_timeout_min.saturating_mul(60)
    });
    // Reconcile the always-on idle bar against the (possibly new)
    // persistent_bar flag: show/resize it when enabled, hide it when not.
    // Cheap and idempotent; skips itself while a recording is in flight.
    crate::windows::reconcile_persistent_bar(&app, state.inner());
    // The user just switched local engine/model → eagerly warm it (load the
    // model + sidecar into memory) so the first dictation is instant, showing a
    // "Warming…" HUD meanwhile. No-op for cloud providers / unchanged selection.
    if model_switched {
        crate::warm::warm_current_model(&app);
    }
    // Broadcast to all webviews so the main app window's settings hook
    // refreshes immediately (otherwise the chipbar's persist of a new
    // correction is invisible to the Corrections route until reload).
    if let Err(e) = app.emit("settings:changed", &settings) {
        log::error!("[vibeking] settings:changed emit failed: {e}");
    }
}

/// Click-to-record entry point for the always-on idle bar. Toggles the shared
/// recording state and emits the same `recording:start` / `recording:stop`
/// events the global hotkey does, so the existing listeners in `lib.rs` drive
/// mic capture and the chip bar identically — a click behaves exactly like a
/// tap-toggle of the hotkey (and Esc still cancels it).
#[tauri::command]
pub fn ui_toggle_recording<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, SharedState>,
) {
    let mut mode = state.recording_mode.lock();
    match *mode {
        RecMode::Idle => {
            *mode = RecMode::Toggle;
            drop(mode);
            let _ = app.emit("recording:start", ());
            let _ = app.emit("mode:changed", "toggle");
        }
        _ => {
            *mode = RecMode::Idle;
            drop(mode);
            let _ = app.emit("recording:stop", ());
        }
    }
}

/// Returned to the frontend on every refinement-mode change so the chip bar
/// can render the badge without a settings round-trip. `mode: None` means the
/// user has cycled to the "Off" slot — no refinement runs at next stop.
#[derive(Debug, Clone, Serialize)]
pub struct ActiveRefinementMode {
    pub mode: Option<RefinementMode>,
}

fn lookup_mode<'a>(settings: &'a Settings, id: &str) -> Option<&'a RefinementMode> {
    settings.refinement_modes.iter().find(|m| m.id == id)
}

fn active_mode_payload(settings: &Settings) -> ActiveRefinementMode {
    let mode = settings
        .active_refinement_mode_id
        .as_deref()
        .and_then(|id| lookup_mode(settings, id))
        .cloned();
    ActiveRefinementMode { mode }
}

/// Set the active refinement mode and broadcast. Plain-function entry point
/// shared by the Tauri command and any other in-process caller (e.g. the
/// hotkey thread). Returns the broadcast payload.
pub fn apply_active_refinement_mode<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &SharedState,
    id: Option<String>,
) -> ActiveRefinementMode {
    let payload = {
        let mut s = state.settings.lock();
        s.active_refinement_mode_id = id;
        active_mode_payload(&s)
    };
    let _ = app.emit("refinement:active-changed", &payload);
    payload
}

/// Advance to the next entry in `quick_switch_mode_ids` (wrapping past the
/// last position into the "Off" sentinel, then wrapping again to the first
/// id). Shared by the Tauri command and the Shift+Tab hotkey handler.
pub fn apply_cycle_refinement_mode<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &SharedState,
) -> ActiveRefinementMode {
    let payload = {
        let mut s = state.settings.lock();
        let cycle = s.quick_switch_mode_ids.clone();
        let current = s.active_refinement_mode_id.clone();
        s.active_refinement_mode_id = next_cycle_id(&cycle, current.as_deref());
        active_mode_payload(&s)
    };
    let _ = app.emit("refinement:active-changed", &payload);
    payload
}

/// Set the active refinement mode and broadcast. Passing `None` selects the
/// "Off" slot. Persisted-shape mutation is the caller's responsibility — the
/// frontend's `useSettings.update` round-trip is the canonical save path.
#[tauri::command]
pub fn set_active_refinement_mode<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, SharedState>,
    id: Option<String>,
) -> ActiveRefinementMode {
    apply_active_refinement_mode(&app, state.inner(), id)
}

/// Advance to the next entry in `quick_switch_mode_ids` (wrapping past the
/// last position into the "Off" sentinel, then wrapping again to the first
/// id). Called by the Shift+Tab hotkey handler. Emits
/// `refinement:active-changed` so the chip bar updates without a poll.
#[tauri::command]
pub fn cycle_refinement_mode<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, SharedState>,
) -> ActiveRefinementMode {
    apply_cycle_refinement_mode(&app, state.inner())
}

/// Cycle order is `[id_0, id_1, ..., id_n, Off, id_0, ...]`. An unknown
/// current id (mode deleted out from under us) is treated as "before the
/// start" and lands on the first cycle entry. Empty cycle → always Off.
fn next_cycle_id(cycle: &[String], current: Option<&str>) -> Option<String> {
    if cycle.is_empty() {
        return None;
    }
    match current {
        None => Some(cycle[0].clone()),
        Some(id) => match cycle.iter().position(|c| c == id) {
            Some(i) if i + 1 < cycle.len() => Some(cycle[i + 1].clone()),
            Some(_) => None,
            None => Some(cycle[0].clone()),
        },
    }
}

#[tauri::command]
pub fn set_hotkey_capture(state: tauri::State<'_, SharedState>, on: bool) {
    state
        .hotkey_capture_active
        .store(on, std::sync::atomic::Ordering::Relaxed);
}
