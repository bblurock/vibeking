//! Tauri commands for managing local STT engines.
//!
//! Both engines share these commands; the optional `engine` arg selects
//! between them. Backwards-compat: an absent / null `engine` is treated
//! as `"whisper"` so older frontends keep working.

use serde::Serialize;
use tauri::{Emitter, Runtime};

/// Engine identifier accepted by all model commands.
///
/// `"whisper"` → whisper.cpp models managed by `local_stt`.
/// `"parakeet"` → FluidAudio Parakeet TDT v3 managed by `local_parakeet`.
const DEFAULT_ENGINE: &str = "whisper";

#[derive(Debug, Clone, Serialize)]
pub struct ModelStatus {
    pub id: String,
    pub exists: bool,
    pub path: String,
    pub engine: String,
    /// True while the engine's `prepare_engine` is currently downloading or
    /// compiling. Survives frontend reloads — the JS in-flight tracker is
    /// in-memory and gets wiped by Cmd+R, so the row/banner consult this
    /// field to stay in sync with a long-running Rust prep.
    /// Only meaningful for engines that have a discrete prepare phase
    /// (Parakeet); whisper.cpp keeps it `false`.
    pub preparing: bool,
}

#[tauri::command]
pub fn model_status(
    model_id: Option<String>,
    engine: Option<String>,
) -> Result<ModelStatus, String> {
    let engine = engine.unwrap_or_else(|| DEFAULT_ENGINE.to_string());
    match engine.as_str() {
        "whisper" => whisper_status(model_id, engine),
        "parakeet" => parakeet_status(engine),
        "qwen3" => qwen3_status(engine),
        "gemma" => gemma_status(engine),
        other => Err(format!("unknown engine: {other}")),
    }
}

#[tauri::command]
pub async fn download_model<R: Runtime>(
    app: tauri::AppHandle<R>,
    model_id: Option<String>,
    engine: Option<String>,
) -> Result<String, String> {
    let engine = engine.unwrap_or_else(|| DEFAULT_ENGINE.to_string());
    match engine.as_str() {
        "whisper" => whisper_download(app, model_id, engine).await,
        "parakeet" => parakeet_download(app, engine).await,
        "qwen3" => qwen3_download(app, engine).await,
        "gemma" => gemma_download(app, engine).await,
        other => Err(format!("unknown engine: {other}")),
    }
}

#[tauri::command]
pub fn delete_model(model_id: Option<String>, engine: Option<String>) -> Result<(), String> {
    let engine = engine.unwrap_or_else(|| DEFAULT_ENGINE.to_string());
    match engine.as_str() {
        "whisper" => whisper_delete(model_id),
        "parakeet" => parakeet_delete(),
        "qwen3" => qwen3_delete(),
        "gemma" => gemma_delete(),
        other => Err(format!("unknown engine: {other}")),
    }
}

/// Event payload for `model:progress`. `engine` lets the frontend filter
/// progress events when multiple engines might emit concurrently (e.g.
/// future prefetch-on-onboarding flow).
#[derive(Debug, Clone, Serialize)]
struct ModelProgress {
    id: String,
    engine: String,
    downloaded: u64,
    total: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
struct ModelComplete {
    id: String,
    engine: String,
}

// ---------------- whisper.cpp engine wrappers ----------------

#[cfg(target_os = "macos")]
fn whisper_status(model_id: Option<String>, engine: String) -> Result<ModelStatus, String> {
    let id = model_id.unwrap_or_else(|| local_stt::DEFAULT_MODEL_ID.to_string());
    let path = local_stt::model_path(&id).map_err(|e| e.to_string())?;
    Ok(ModelStatus {
        exists: local_stt::model_exists(&id),
        path: path.display().to_string(),
        id,
        engine,
        preparing: false, // whisper has a real download-progress stream; no separate prep flag
    })
}

#[cfg(not(target_os = "macos"))]
fn whisper_status(_model_id: Option<String>, _engine: String) -> Result<ModelStatus, String> {
    Err("local STT only available on macOS".into())
}

#[cfg(target_os = "macos")]
async fn whisper_download<R: Runtime>(
    app: tauri::AppHandle<R>,
    model_id: Option<String>,
    engine: String,
) -> Result<String, String> {
    let id = model_id.unwrap_or_else(|| local_stt::DEFAULT_MODEL_ID.to_string());
    let app_handle = app.clone();
    let id_for_progress = id.clone();
    let engine_for_progress = engine.clone();
    let path = local_stt::download_model(&id, move |downloaded, total| {
        let _ = app_handle.emit(
            "model:progress",
            ModelProgress {
                id: id_for_progress.clone(),
                engine: engine_for_progress.clone(),
                downloaded,
                total,
            },
        );
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = app.emit(
        "model:complete",
        ModelComplete {
            id: id.clone(),
            engine,
        },
    );
    Ok(path.display().to_string())
}

#[cfg(not(target_os = "macos"))]
async fn whisper_download<R: Runtime>(
    _app: tauri::AppHandle<R>,
    _model_id: Option<String>,
    _engine: String,
) -> Result<String, String> {
    Err("local STT only available on macOS".into())
}

#[cfg(target_os = "macos")]
fn whisper_delete(model_id: Option<String>) -> Result<(), String> {
    let id = model_id.unwrap_or_else(|| local_stt::DEFAULT_MODEL_ID.to_string());
    let path = local_stt::model_path(&id).map_err(|e| e.to_string())?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("remove {}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn whisper_delete(_model_id: Option<String>) -> Result<(), String> {
    Err("local STT only available on macOS".into())
}

// ---------------- Parakeet engine wrappers ----------------

#[cfg(target_os = "macos")]
fn parakeet_status(engine: String) -> Result<ModelStatus, String> {
    // Parakeet has a single bundled model managed by the upstream Swift
    // package — there's no per-file path we can hand back. `path` is left
    // empty; the frontend should treat its absence as "engine-managed".
    Ok(ModelStatus {
        exists: local_parakeet::engine_ready(),
        path: String::new(),
        id: local_parakeet::ENGINE_ID.to_string(),
        engine,
        preparing: local_parakeet::is_preparing(),
    })
}

#[cfg(not(target_os = "macos"))]
fn parakeet_status(_engine: String) -> Result<ModelStatus, String> {
    Err("Parakeet engine only available on macOS".into())
}

#[cfg(target_os = "macos")]
async fn parakeet_download<R: Runtime>(
    app: tauri::AppHandle<R>,
    engine: String,
) -> Result<String, String> {
    let id = local_parakeet::ENGINE_ID.to_string();
    let app_handle = app.clone();
    let id_for_progress = id.clone();
    let engine_for_progress = engine.clone();
    local_parakeet::prepare_engine(move |downloaded, total| {
        let _ = app_handle.emit(
            "model:progress",
            ModelProgress {
                id: id_for_progress.clone(),
                engine: engine_for_progress.clone(),
                downloaded,
                total,
            },
        );
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = app.emit(
        "model:complete",
        ModelComplete {
            id: id.clone(),
            engine,
        },
    );
    Ok(id)
}

#[cfg(not(target_os = "macos"))]
async fn parakeet_download<R: Runtime>(
    _app: tauri::AppHandle<R>,
    _engine: String,
) -> Result<String, String> {
    Err("Parakeet engine only available on macOS".into())
}

#[cfg(target_os = "macos")]
fn parakeet_delete() -> Result<(), String> {
    local_parakeet::clear_engine_cache().map_err(|e| e.to_string())
}

#[cfg(not(target_os = "macos"))]
fn parakeet_delete() -> Result<(), String> {
    Err("Parakeet engine only available on macOS".into())
}

// ---------------- Qwen3 engine wrappers (Chinese specialist) ----------------

#[cfg(target_os = "macos")]
fn qwen3_status(engine: String) -> Result<ModelStatus, String> {
    Ok(ModelStatus {
        exists: local_qwen3::engine_ready(),
        path: String::new(), // engine-managed, like Parakeet
        id: local_qwen3::ENGINE_ID.to_string(),
        engine,
        preparing: local_qwen3::is_preparing(),
    })
}

#[cfg(not(target_os = "macos"))]
fn qwen3_status(_engine: String) -> Result<ModelStatus, String> {
    Err("Qwen3 engine only available on macOS".into())
}

#[cfg(target_os = "macos")]
async fn qwen3_download<R: Runtime>(
    app: tauri::AppHandle<R>,
    engine: String,
) -> Result<String, String> {
    let id = local_qwen3::ENGINE_ID.to_string();
    let app_handle = app.clone();
    let id_for_progress = id.clone();
    let engine_for_progress = engine.clone();
    local_qwen3::prepare_engine(move |downloaded, total| {
        let _ = app_handle.emit(
            "model:progress",
            ModelProgress {
                id: id_for_progress.clone(),
                engine: engine_for_progress.clone(),
                downloaded,
                total,
            },
        );
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = app.emit(
        "model:complete",
        ModelComplete {
            id: id.clone(),
            engine,
        },
    );
    Ok(id)
}

#[cfg(not(target_os = "macos"))]
async fn qwen3_download<R: Runtime>(
    _app: tauri::AppHandle<R>,
    _engine: String,
) -> Result<String, String> {
    Err("Qwen3 engine only available on macOS".into())
}

#[cfg(target_os = "macos")]
fn qwen3_delete() -> Result<(), String> {
    local_qwen3::clear_engine_cache().map_err(|e| e.to_string())
}

#[cfg(not(target_os = "macos"))]
fn qwen3_delete() -> Result<(), String> {
    Err("Qwen3 engine only available on macOS".into())
}

// ---------------- Gemma engine wrappers (MLX sidecar) ----------------

#[cfg(target_os = "macos")]
fn gemma_status(engine: String) -> Result<ModelStatus, String> {
    Ok(ModelStatus {
        exists: gemma_server::engine_ready(),
        path: String::new(), // venv-managed
        id: gemma_server::ENGINE_ID.to_string(),
        engine,
        preparing: gemma_server::is_preparing(),
    })
}

#[cfg(not(target_os = "macos"))]
fn gemma_status(_engine: String) -> Result<ModelStatus, String> {
    Err("Gemma engine only available on macOS".into())
}

#[cfg(target_os = "macos")]
async fn gemma_download<R: Runtime>(
    app: tauri::AppHandle<R>,
    engine: String,
) -> Result<String, String> {
    // "Download" here = create the venv + install mlx-vlm (the ~8 GB model
    // itself downloads lazily on the first transcription / server start).
    let id = gemma_server::ENGINE_ID.to_string();
    let app_handle = app.clone();
    let id_for_progress = id.clone();
    let engine_for_progress = engine.clone();
    gemma_server::setup(move |downloaded, total| {
        let _ = app_handle.emit(
            "model:progress",
            ModelProgress {
                id: id_for_progress.clone(),
                engine: engine_for_progress.clone(),
                downloaded,
                total,
            },
        );
    })
    .await
    .map_err(|e| e.to_string())?;
    let _ = app.emit(
        "model:complete",
        ModelComplete {
            id: id.clone(),
            engine,
        },
    );
    Ok(id)
}

#[cfg(not(target_os = "macos"))]
async fn gemma_download<R: Runtime>(
    _app: tauri::AppHandle<R>,
    _engine: String,
) -> Result<String, String> {
    Err("Gemma engine only available on macOS".into())
}

#[cfg(target_os = "macos")]
fn gemma_delete() -> Result<(), String> {
    gemma_server::clear_engine().map_err(|e| e.to_string())
}

#[cfg(not(target_os = "macos"))]
fn gemma_delete() -> Result<(), String> {
    Err("Gemma engine only available on macOS".into())
}

#[cfg(target_os = "macos")]
use crate::gemma_server;
#[cfg(target_os = "macos")]
use crate::local_parakeet;
#[cfg(target_os = "macos")]
use crate::local_qwen3;
#[cfg(target_os = "macos")]
use crate::local_stt;
