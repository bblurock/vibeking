//! Model warming — eagerly load a local STT engine's model + sidecar into
//! memory so the first dictation after a model switch or app launch is instant,
//! surfacing a "Warming…" HUD on the chip bar while the multi-GB load happens.
//!
//! Triggered from two places: [`crate::state::set_settings`] (the user picked a
//! new local engine) and the `.setup()` closure in `lib.rs` (app open, for the
//! currently selected engine). Cloud providers and Whisper need no warming and
//! are skipped silently (see [`crate::stt::provider_needs_warm`]).

use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::state::SharedState;
use crate::stt::{self, Provider};

#[derive(Clone, Serialize)]
struct WarmEvent {
    provider: Provider,
}

#[derive(Clone, Serialize)]
struct WarmFailedEvent {
    provider: Provider,
    message: String,
}

/// Is the engine for `provider` installed (downloaded / set up) and therefore
/// warmable? An un-installed engine is the EnginePicker/download flow's job — we
/// must not flash a "Warming…" HUD that can only fail.
fn engine_ready(provider: Provider, model: Option<&str>) -> bool {
    match provider {
        Provider::Qwen3 => crate::local_qwen3::engine_ready(),
        Provider::FluidAudio => crate::local_parakeet::engine_ready(),
        Provider::Gemma => crate::gemma_server::engine_ready(),
        // Whisper is "ready" to warm once its model file is downloaded.
        Provider::Local => {
            let model_id = model
                .filter(|m| !m.is_empty())
                .unwrap_or(crate::local_stt::DEFAULT_MODEL_ID);
            crate::local_stt::model_exists(model_id)
        }
        _ => false,
    }
}

/// Warm the currently-selected local model (if any), emitting `model:warming`
/// → `model:warmed` / `model:warm-failed` so the chip bar can show a spinner.
/// Fire-and-forget: spawns the actual load on the async runtime and returns
/// immediately. Safe to call repeatedly — the underlying engine warms are
/// idempotent, and a stale in-flight warm is superseded via
/// [`crate::state::AppState::warming_generation`].
pub fn warm_current_model<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<SharedState>().inner().clone();
    let (provider, model, gemma_streaming) = {
        let s = state.settings.lock();
        (s.provider, s.model.clone(), s.gemma_streaming)
    };

    if !stt::provider_needs_warm(provider) {
        return;
    }
    if !engine_ready(provider, model.as_deref()) {
        // Not installed yet — the download/setup flow owns this case; warming
        // here would only ever fail and flash a misleading HUD.
        return;
    }
    if state.audio_engine.is_recording() {
        // Don't contend with a live recording for the engine; the lazy
        // ensure_running on stop handles that path.
        return;
    }

    let gen = state.warming_generation.fetch_add(1, Ordering::Relaxed) + 1;
    *state.warming_now.lock() = Some(provider);
    log::info!("[vibeking warm] warming {provider:?} (gen={gen})");
    let _ = app.emit("model:warming", WarmEvent { provider });

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = stt::warm_engine(provider, model.as_deref(), gemma_streaming).await;
        // A newer switch/open superseded this warm → drop its result silently so
        // we don't flash a stale "ready" for a model that's no longer selected.
        // The newer warm owns `warming_now`, so we don't clear it here either.
        if state.warming_generation.load(Ordering::Relaxed) != gen {
            return;
        }
        *state.warming_now.lock() = None;
        match result {
            Ok(()) => {
                log::info!("[vibeking warm] {provider:?} warmed (gen={gen})");
                let _ = app.emit("model:warmed", WarmEvent { provider });
            }
            Err(e) => {
                log::error!("[vibeking warm] {provider:?} warm failed: {e}");
                let _ = app.emit(
                    "model:warm-failed",
                    WarmFailedEvent {
                        provider,
                        message: e.to_string(),
                    },
                );
            }
        }
    });
}

/// The provider currently being warmed, if any. The chip bar queries this on
/// mount so an app-open warm whose `model:warming` event fired before the
/// chipbar webview was listening still shows the "Warming…" HUD.
#[tauri::command]
pub fn current_warming(state: tauri::State<'_, SharedState>) -> Option<Provider> {
    *state.warming_now.lock()
}
