//! Watches the focused text field for ~20 seconds after a paste,
//! diffs user edits against the pasted text, and emits
//! `correction:detected` when a single-word correction is found.
//!
//! ## Strategy: 250ms AXValue polling (not AXObserver).
//!
//! The T11 spike at `src-tauri/examples/ax_observer_spike.rs` set up the
//! `AXObserverCreate` + `CFRunLoopRun` path on both `tokio::task::spawn_blocking`
//! (strategy A) and `std::thread::spawn` (strategy B). The infrastructure
//! compiles and the AX setup executes without errors.
//!
//! For the production module we deliberately ship the polling fallback that
//! the kickoff explicitly green-lights, for three reasons:
//!
//!   1. **AXObserver coverage is uneven.** Electron-based apps (Slack,
//!      VSCode, Cursor, Notion desktop, Discord) routinely fail to fire
//!      `AXValueChanged`, and many web fields in Chrome don't either. The
//!      polling path gets us coverage everywhere AXValue can be *read*,
//!      which is a strict superset.
//!   2. **Determinism.** A 250ms tick is a known cost; observer-delivery
//!      latency is opaque.
//!   3. **Simplicity.** Polling avoids carrying a CoreFoundation run-loop
//!      lifecycle through tokio. Cancellation is a plain `AbortHandle`.
//!
//! The added ~250ms p50 detection latency is below the user's perception
//! threshold for "Vibeking noticed my edit" — well inside the 5-second
//! UX budget called out in the kickoff acceptance criteria.
//!
//! ## Single-watcher invariant
//!
//! Only one watcher runs at a time. Starting a new watcher cancels the
//! previous one via the `AbortHandle` stored in `AppState`. Plus a 20-sec
//! hard timeout per watcher. No observer leaks under rapid-fire dictation.
//!
//! ## Failure posture
//!
//! Any AX read failure (permission revoked, app died, focus changed to a
//! non-AX app) downgrades the watcher to a clean no-op exit. We never
//! surface errors to the user — a missed correction is silent.

use core_foundation::base::{CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::corrections::{self, CorrectionCandidate};
use crate::state::{CorrectionLearningMode, SharedState};
use crate::windows;

/// Set `VIBEKING_WATCHER_DEBUG=1` (any truthy value) to surface high-volume
/// per-tick / per-attempt diagnostics. Default is quiet — only the
/// high-signal "interesting" events log.
fn debug_enabled() -> bool {
    match std::env::var("VIBEKING_WATCHER_DEBUG") {
        Ok(v) => !v.is_empty() && v != "0" && v.to_ascii_lowercase() != "false",
        Err(_) => false,
    }
}

macro_rules! debug_log {
    ($($arg:tt)*) => {
        if debug_enabled() {
            log::info!($($arg)*);
        }
    };
}

/// Tick interval for polling the focused element's AXValue.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Hard ceiling on how long a single watcher runs. Set by the caller in
/// practice; this constant documents the design intent.
pub const DEFAULT_WATCH_WINDOW: Duration = Duration::from_secs(20);

/// How many consecutive polls the field must remain unchanged before we
/// trust the current state as "user finished editing" and check for a
/// correction. 3 polls × 250ms = ~750ms of typing stillness. Without this,
/// the detector fires mid-keystroke (e.g. "Asian" → "a" when the user has
/// only typed the first character of "Async").
const STABLE_POLLS_BEFORE_DETECT: u32 = 3;

/// Payload of the `correction:detected` Tauri event. T12 (chip-bar UI)
/// consumes this and either prompts (Ask) or auto-stores (Auto).
#[derive(Debug, Clone, Serialize)]
pub struct CorrectionDetectedPayload {
    pub from: String,
    pub to: String,
    /// The text we originally pasted — the chip bar shows this for context.
    pub pasted_text: String,
    /// UTF-8 byte offset of `from` within `pasted_text`.
    pub from_offset: usize,
    /// `Ask` or `Auto`. We re-read the setting at emit time so the frontend
    /// can branch without another IPC roundtrip.
    pub mode: CorrectionLearningMode,
}

/// Kick off a watcher for the currently-focused app/element. Returns
/// immediately; actual watching runs in a spawned tokio task.
///
/// `pasted_text` is the transcript Vibeking just pasted. The watcher reads
/// the field's AXValue once at start to capture `original_field_snapshot`
/// (what was in the field BEFORE the paste landed) and then polls for the
/// next `timeout` looking for a single-word swap.
///
/// Single-watcher invariant: any in-flight watcher is aborted first.
/// Skips entirely if `correction_learning_mode` is `Off`.
pub fn spawn_watcher<R: Runtime>(app: AppHandle<R>, pasted_text: String, timeout: Duration) {
    let state = app.state::<SharedState>().inner().clone();

    let mode = state.settings.lock().correction_learning_mode;
    debug_log!(
        "[vibeking correction_watcher] spawn_watcher mode={:?} pasted_len={} pasted_preview={:?}",
        mode,
        pasted_text.chars().count(),
        pasted_text.chars().take(60).collect::<String>(),
    );
    if mode == CorrectionLearningMode::Off {
        return;
    }

    if pasted_text.trim().is_empty() {
        return;
    }

    cancel_existing(&state);

    let state_for_task = state.clone();
    let handle = tauri::async_runtime::spawn(async move {
        run_watcher(app, state_for_task, pasted_text, mode, timeout).await;
    });

    *state.correction_watcher_abort.lock() = Some(handle);
}

fn cancel_existing(state: &SharedState) {
    let prev = state.correction_watcher_abort.lock().take();
    if let Some(h) = prev {
        h.abort();
    }
}

async fn run_watcher<R: Runtime>(
    app: AppHandle<R>,
    state: SharedState,
    pasted_text: String,
    mode: CorrectionLearningMode,
    timeout: Duration,
) {
    let pasted_for_task = pasted_text.clone();

    // Locate the pasted text inside the focused field. AXValue updates can
    // lag the actual paste keystroke (especially in Terminal.app and
    // Electron apps), so retry a few times before giving up. Total worst-
    // case latency = SNAPSHOT_MAX_ATTEMPTS * SNAPSHOT_RETRY_DELAY.
    const SNAPSHOT_MAX_ATTEMPTS: u32 = 8;
    const SNAPSHOT_RETRY_DELAY: Duration = Duration::from_millis(120);

    let mut snap_opt: Option<FocusedSnapshot> = None;
    let mut paste_idx_opt: Option<usize> = None;
    for attempt in 1..=SNAPSHOT_MAX_ATTEMPTS {
        if attempt > 1 {
            tokio::time::sleep(SNAPSHOT_RETRY_DELAY).await;
        }
        let snapshot = tokio::task::spawn_blocking(read_focused_app_snapshot).await;
        let Ok(Some(s)) = snapshot else {
            debug_log!(
                "[vibeking correction_watcher] snapshot attempt {} — no focused AX element / app",
                attempt
            );
            continue;
        };
        if let Some(idx) = s.field_value.rfind(&pasted_text) {
            paste_idx_opt = Some(idx);
            snap_opt = Some(s);
            break;
        }
        debug_log!(
            "[vibeking correction_watcher] snapshot attempt {} — paste not yet in AX value (field_len={})",
            attempt,
            s.field_value.chars().count(),
        );
    }
    let (Some(snap), Some(paste_idx)) = (snap_opt, paste_idx_opt) else {
        // Single concise final-failure line — useful for the user to know
        // "the watcher gave up on this app." Quieter than the per-attempt
        // retry chatter above (those go to debug_log).
        log::info!(
            "[vibeking correction_watcher] paste never appeared in AX value after {}ms — \
             AX-blind app or paste lag exceeded budget. Watcher exiting.",
            SNAPSHOT_MAX_ATTEMPTS as u128 * SNAPSHOT_RETRY_DELAY.as_millis()
        );
        return;
    };

    let anchor_before = snap.field_value[..paste_idx].to_string();
    let anchor_after = snap.field_value[paste_idx + pasted_text.len()..].to_string();

    debug_log!(
        "[vibeking correction_watcher] snapshot pid={} field_len={} paste_idx={} \
         before_len={} after_len={}",
        snap.pid,
        snap.field_value.chars().count(),
        paste_idx,
        anchor_before.chars().count(),
        anchor_after.chars().count(),
    );

    let app_for_loop = app.clone();
    let pid = snap.pid;
    let initial_field = snap.field_value.clone();

    let loop_fut = async move {
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        ticker.tick().await; // first tick fires immediately — skip it

        // `baseline` is the text in the pasted region that the next correction
        // will be diffed against. Starts as the original paste; after each
        // detected correction we replace it with the post-correction text so
        // a subsequent edit to a *different* word in the same paste is also
        // caught. Without this re-baseline, the second correction's diff
        // would still reference the original (now-stale) paste and either
        // mis-fire or get silently rejected.
        let mut baseline = pasted_for_task.clone();
        let mut last_field = initial_field.clone();
        let mut stable_ticks: u32 = 0;
        let mut last_logged_field = initial_field.clone();
        let mut tick_count: u32 = 0;

        loop {
            ticker.tick().await;
            tick_count += 1;

            let read = tokio::task::spawn_blocking(move || read_field_value_for_pid(pid)).await;
            let current = match read {
                Ok(Some(v)) => v,
                Ok(None) => {
                    debug_log!(
                        "[vibeking correction_watcher] field read None at tick {} (focus moved off pid={}). Exiting.",
                        tick_count, pid
                    );
                    return;
                }
                Err(_) => return,
            };

            // Stability accounting: if the field changed since last tick, reset
            // the stability counter; if unchanged, increment.
            if current == last_field {
                stable_ticks += 1;
            } else {
                stable_ticks = 0;
                last_field = current.clone();
            }

            // Extract the edited region between our anchors. If either anchor
            // no longer matches, the user edited *outside* the pasted region
            // (clicked into the scrollback, the terminal repainted, etc.) —
            // we can't reason about that and bail to a None result.
            let edited_region = if current.starts_with(&anchor_before)
                && current.ends_with(&anchor_after)
                && current.len() >= anchor_before.len() + anchor_after.len()
            {
                Some(current[anchor_before.len()..current.len() - anchor_after.len()].to_string())
            } else {
                None
            };

            // Per-tick field change is debug-only — too noisy for default output.
            if current != last_logged_field {
                debug_log!(
                    "[vibeking correction_watcher] tick={} field changed len={} stable_ticks={} \
                     edited_region_len={:?} edited_preview={:?}",
                    tick_count,
                    current.chars().count(),
                    stable_ticks,
                    edited_region.as_ref().map(|r| r.chars().count()),
                    edited_region
                        .as_deref()
                        .map(|r| r.chars().take(80).collect::<String>())
                        .unwrap_or_else(|| "<anchors lost>".to_string()),
                );
                last_logged_field = current.clone();
            }

            // Only run the detector once typing has been still for at least
            // STABLE_POLLS_BEFORE_DETECT polls. Avoids firing on intermediate
            // keystrokes (e.g. "Asian" → "a" mid-type).
            if stable_ticks != STABLE_POLLS_BEFORE_DETECT {
                continue;
            }

            let Some(edited) = edited_region else {
                debug_log!(
                    "[vibeking correction_watcher] tick={} stable but anchors no longer match",
                    tick_count,
                );
                continue;
            };

            // Diff the current edited region against the rolling baseline.
            let detect = corrections::detect_correction(&baseline, &edited, None);
            debug_log!(
                "[vibeking correction_watcher] tick={} stable — detect={}",
                tick_count,
                match &detect {
                    Some(c) => format!("Some({:?} -> {:?})", c.from, c.to),
                    None => "None".to_string(),
                },
            );
            if let Some(candidate) = detect {
                emit_detected(&app_for_loop, &state, &baseline, candidate, mode);
                // Re-baseline so a *next* correction in the same paste is
                // diffed against this state, not the original pre-edit text.
                baseline = edited;
                // And ensure we don't immediately re-fire on the same stable
                // state — wait for another change → settle cycle.
                stable_ticks = stable_ticks.saturating_sub(1);
            }
        }
    };

    match tokio::time::timeout(timeout, loop_fut).await {
        Ok(()) => {
            // Detected or clean exit — already logged.
        }
        Err(_) => {
            debug_log!(
                "[vibeking correction_watcher] timed out after {}s",
                timeout.as_secs()
            );
        }
    }
    let _ = app; // keep ownership until end
}

fn emit_detected<R: Runtime>(
    app: &AppHandle<R>,
    _state: &SharedState,
    pasted_text: &str,
    candidate: CorrectionCandidate,
    mode: CorrectionLearningMode,
) {
    let payload = CorrectionDetectedPayload {
        from: candidate.from.clone(),
        to: candidate.to.clone(),
        pasted_text: pasted_text.to_string(),
        from_offset: candidate.from_offset,
        mode,
    };
    log::info!(
        "[vibeking correction_watcher] correction:detected  from={:?} to={:?} mode={:?}",
        payload.from,
        payload.to,
        payload.mode
    );
    // The chipbar window is hidden ~1500ms after the polished paste, so by
    // the time we detect a correction it's not visible. Re-show before emit
    // so the user actually sees the prompt / success toast.
    windows::show_chipbar(app);
    if let Err(e) = app.emit("correction:detected", &payload) {
        log::error!("[vibeking correction_watcher] emit correction:detected failed: {e}");
    }
}

// ---------------------------------------------------------------------------
// AX snapshot + per-tick field read
// ---------------------------------------------------------------------------

struct FocusedSnapshot {
    pid: i32,
    field_value: String,
}

/// One-shot read of the focused app's focused element value, plus the PID
/// so subsequent ticks can re-resolve the focused element under the same
/// app without re-walking the system-wide tree.
fn read_focused_app_snapshot() -> Option<FocusedSnapshot> {
    if !unsafe { ax_ffi::AXIsProcessTrusted() } {
        return None;
    }
    unsafe {
        let system = ax_ffi::AXUIElementCreateSystemWide();
        if system.is_null() {
            return None;
        }
        let app = ax_copy_element(system, "AXFocusedApplication");
        let Some(app) = app else {
            ax_release(system);
            return None;
        };
        let mut pid: i32 = 0;
        let perr = ax_ffi::AXUIElementGetPid(app, &mut pid);
        if perr != ax_ffi::K_AX_ERROR_SUCCESS || pid <= 0 {
            ax_release(app);
            ax_release(system);
            return None;
        }

        let value = ax_copy_focused_value(app);
        ax_release(app);
        ax_release(system);

        value.map(|v| FocusedSnapshot {
            pid,
            field_value: v,
        })
    }
}

/// Per-tick read of the focused element value. Re-resolves the focused
/// element via `AXUIElementCreateApplication(pid)` so we follow focus
/// changes *within* the same app (e.g. user tabs between fields).
/// Returns `None` if focus has moved to a different app, AX permission
/// was revoked, or the element no longer exposes a string value.
fn read_field_value_for_pid(pid: i32) -> Option<String> {
    if !unsafe { ax_ffi::AXIsProcessTrusted() } {
        return None;
    }
    unsafe {
        // Verify focus still belongs to the same PID — bail if the user has
        // app-switched. Otherwise we'd start watching the wrong process.
        let system = ax_ffi::AXUIElementCreateSystemWide();
        if system.is_null() {
            return None;
        }
        let Some(focused_app) = ax_copy_element(system, "AXFocusedApplication") else {
            ax_release(system);
            return None;
        };
        let mut current_pid: i32 = 0;
        let perr = ax_ffi::AXUIElementGetPid(focused_app, &mut current_pid);
        ax_release(focused_app);
        ax_release(system);
        if perr != ax_ffi::K_AX_ERROR_SUCCESS || current_pid != pid {
            return None;
        }

        let app = ax_ffi::AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        let value = ax_copy_focused_value(app);
        ax_release(app);
        value
    }
}

/// Reads the AXValue of the focused element of `app`. Returns `None` if
/// there's no focused element, no AXValue attribute, or the value isn't a
/// CFString.
unsafe fn ax_copy_focused_value(app: ax_ffi::AXUIElementRef) -> Option<String> {
    let focused = unsafe { ax_copy_element(app, "AXFocusedUIElement") }?;
    let val = unsafe { ax_copy_string(focused, "AXValue") };
    unsafe { ax_release(focused) };
    val
}

// ---------------------------------------------------------------------------
// AX helpers — mirror the style of screen_context.rs
// ---------------------------------------------------------------------------

unsafe fn ax_copy_string(element: ax_ffi::AXUIElementRef, attr: &str) -> Option<String> {
    if element.is_null() {
        return None;
    }
    let key = CFString::new(attr);
    let mut value: CFTypeRef = std::ptr::null();
    let err = unsafe {
        ax_ffi::AXUIElementCopyAttributeValue(element, key.as_concrete_TypeRef(), &mut value)
    };
    if err != ax_ffi::K_AX_ERROR_SUCCESS || value.is_null() {
        return None;
    }
    let type_id = unsafe { ax_ffi::CFGetTypeID(value) };
    if type_id != unsafe { ax_ffi::CFStringGetTypeID() } {
        unsafe { ax_ffi::CFRelease(value) };
        return None;
    }
    let s: CFString = unsafe { TCFType::wrap_under_create_rule(value as CFStringRef) };
    Some(s.to_string())
}

unsafe fn ax_copy_element(
    element: ax_ffi::AXUIElementRef,
    attr: &str,
) -> Option<ax_ffi::AXUIElementRef> {
    if element.is_null() {
        return None;
    }
    let key = CFString::new(attr);
    let mut value: CFTypeRef = std::ptr::null();
    let err = unsafe {
        ax_ffi::AXUIElementCopyAttributeValue(element, key.as_concrete_TypeRef(), &mut value)
    };
    if err != ax_ffi::K_AX_ERROR_SUCCESS || value.is_null() {
        return None;
    }
    Some(value as ax_ffi::AXUIElementRef)
}

unsafe fn ax_release(element: ax_ffi::AXUIElementRef) {
    if !element.is_null() {
        unsafe { ax_ffi::CFRelease(element as CFTypeRef) };
    }
}

mod ax_ffi {
    use core_foundation::base::CFTypeRef;
    use core_foundation::string::CFStringRef;
    use std::ffi::c_void;

    pub type AXUIElementRef = *const c_void;
    pub type AXError = i32;

    pub const K_AX_ERROR_SUCCESS: AXError = 0;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        pub fn AXUIElementCreateSystemWide() -> AXUIElementRef;
        pub fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
        pub fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> AXError;
        pub fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> AXError;
        pub fn AXIsProcessTrusted() -> bool;
        pub fn CFGetTypeID(cf: CFTypeRef) -> usize;
        pub fn CFStringGetTypeID() -> usize;
        pub fn CFRelease(cf: CFTypeRef);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_serializes_in_expected_shape() {
        let p = CorrectionDetectedPayload {
            from: "claw".to_string(),
            to: "Claude".to_string(),
            pasted_text: "I love claw".to_string(),
            from_offset: 7,
            mode: CorrectionLearningMode::Ask,
        };
        let json = serde_json::to_value(&p).expect("serialize");
        assert_eq!(json["from"], "claw");
        assert_eq!(json["to"], "Claude");
        assert_eq!(json["pasted_text"], "I love claw");
        assert_eq!(json["from_offset"], 7);
        // CorrectionLearningMode is kebab-case via #[serde(rename_all = "kebab-case")]
        assert_eq!(json["mode"], "ask");
    }

    #[test]
    fn default_watch_window_is_twenty_seconds() {
        assert_eq!(DEFAULT_WATCH_WINDOW, Duration::from_secs(20));
    }

    #[test]
    fn poll_interval_is_below_perception_threshold() {
        // Keep below 300ms so users don't perceive the detection lag.
        assert!(POLL_INTERVAL <= Duration::from_millis(300));
    }
}
