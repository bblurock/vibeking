//! Clipboard sandwich text insertion.
//!
//! Vendored from EpicenterHQ/epicenter (MIT) —
//! `apps/whispering/src-tauri/src/lib.rs:198-262` (write_text).
//!
//! Saves the user's clipboard, writes the transcript, synthesizes ⌘V, restores.

use anyhow::{anyhow, Result};
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_clipboard_manager::ClipboardExt;

#[derive(Debug, Clone, Serialize)]
struct InsertCompletePayload {
    pasted_text: String,
    original_clipboard: Option<String>,
    ts_ms: u64,
}

pub async fn write_text<R: Runtime>(app: AppHandle<R>, text: String) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }

    let original_clipboard = app.clipboard().read_text().ok();

    app.clipboard()
        .write_text(&text)
        .map_err(|e| anyhow!("clipboard write: {e}"))?;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| anyhow!("enigo: {e}"))?;

    #[cfg(target_os = "macos")]
    let (modifier, v_key) = (Key::Meta, Key::Other(9));
    #[cfg(target_os = "windows")]
    let (modifier, v_key) = (Key::Control, Key::Other(0x56));
    #[cfg(target_os = "linux")]
    let (modifier, v_key) = (Key::Control, Key::Unicode('v'));

    enigo
        .key(modifier, Direction::Press)
        .map_err(|e| anyhow!("press modifier: {e}"))?;
    enigo
        .key(v_key, Direction::Press)
        .map_err(|e| anyhow!("press v: {e}"))?;
    enigo
        .key(v_key, Direction::Release)
        .map_err(|e| anyhow!("release v: {e}"))?;
    enigo
        .key(modifier, Direction::Release)
        .map_err(|e| anyhow!("release modifier: {e}"))?;

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    if let Some(content) = &original_clipboard {
        let _ = app.clipboard().write_text(content);
    }

    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let payload = InsertCompletePayload {
        pasted_text: text.clone(),
        original_clipboard: original_clipboard.clone(),
        ts_ms,
    };
    if let Err(e) = app.emit("insert:complete", &payload) {
        log::error!("[vibeking insert] emit insert:complete failed: {e}");
    }

    Ok(())
}
