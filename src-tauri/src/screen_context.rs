//! Read focused-window text via AX (fast path) and Vision OCR (fallback).
//!
//! When the user presses the dictation hotkey we have ~1s of speech-onset
//! latency to gather context that can be fed to Whisper as `initial_prompt`.
//! This module orchestrates two paths:
//!
//! 1. **AX traversal** — fast (~5-50ms) on apps that expose accessibility,
//!    capped at 300ms with a hard timeout to defend against pathologically
//!    large AX trees (e.g. a 200-page Notion doc).
//! 2. **Vision OCR fallback** — invoked when AX returned nothing useful
//!    (length below `MIN_USEFUL_LEN`). Lives in the sibling `screen_ocr`
//!    module. We also cap that with a 500ms paranoia timeout even though
//!    OCR has its own internal budget.
//!
//! All non-catastrophic errors are downgraded to `Ok(None)` so a capture
//! failure never blocks dictation.

use anyhow::Result;
use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::NSString;
use serde::Serialize;
use std::time::{Duration, Instant};
use tauri::Emitter;

use crate::proper_nouns;
use crate::state::ScreenContextMode;

/// Minimum text length we consider "useful" content for the focused-
/// element fast-path inside AX traversal. Below this we skip the
/// focused element's own value and instead walk up to find the smallest
/// enclosing content container (WebArea, ScrollArea, etc.).
const MIN_USEFUL_LEN: usize = 20;

/// Hard budget for the AX traversal. The traversal itself is normally
/// fast; this exists to bound the worst case on huge trees.
const AX_BUDGET: Duration = Duration::from_millis(300);

/// Hard budget for the OCR fallback. The OCR module has its own internal
/// budget; this is a defensive ceiling.
const OCR_BUDGET: Duration = Duration::from_millis(500);

/// Traversal limits — mirror the spike probe so behavior is identical.
const MAX_DEPTH: u32 = 8;
const MAX_NODES: u32 = 500;

#[derive(Debug, Clone, Serialize)]
pub struct ContextCapture {
    pub text: String,
    pub source: CaptureSource,
    pub bundle_id: Option<String>,
    pub app_name: Option<String>,
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CaptureSource {
    Ax,
    Ocr,
}

/// Payload for the `context:captured` Tauri event.
///
/// We deliberately omit the raw captured text — it may contain passwords,
/// PII, or drafts. Only the already-filtered proper-noun candidate list
/// crosses the boundary, alongside metadata useful for the debug UI.
#[derive(Debug, Clone, Serialize)]
pub struct ContextCapturedEvent {
    pub candidates: Vec<String>,
    pub source: CaptureSource,
    pub bundle_id: Option<String>,
    pub app_name: Option<String>,
    pub latency_ms: u64,
    /// Length in chars of the captured raw text (NOT the text itself).
    pub raw_text_len: usize,
}

/// Cheap probe: is the process AX-trusted?
/// Caller can use this for a permission banner.
pub fn is_ax_trusted() -> bool {
    unsafe { ax_ffi::AXIsProcessTrusted() }
}

/// Capture text from the currently focused window using the user-chosen
/// strategy.
///
/// AX and OCR are *alternatives*, not a fallback chain — the caller picks
/// one based on the trade-off they care about:
/// - [`ScreenContextMode::Ax`] — fast (~5-50ms), no extra permission,
///   coverage varies by app.
/// - [`ScreenContextMode::Ocr`] — slower (~80-300ms), needs Screen
///   Recording permission, works on anything visible.
/// - [`ScreenContextMode::Off`] — early return; should be gated by the
///   caller but we honor it defensively here too.
///
/// Returns `Ok(None)` (not `Err`) on permission denied, empty window,
/// timeout, or any non-catastrophic failure. The caller treats absent
/// context as "no biasing" — we never want a capture failure to block
/// dictation.
pub async fn capture<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    mode: ScreenContextMode,
) -> Result<Option<ContextCapture>> {
    let started = Instant::now();

    match mode {
        ScreenContextMode::Off => Ok(None),
        ScreenContextMode::Ax => {
            let Some(ax) = capture_ax().await else {
                return Ok(None);
            };
            if ax.text.trim().is_empty() {
                return Ok(None);
            }
            let capture = ContextCapture {
                text: ax.text,
                source: CaptureSource::Ax,
                bundle_id: ax.bundle_id,
                app_name: ax.app_name,
                latency_ms: started.elapsed().as_millis() as u64,
            };
            emit_captured(&app, &capture);
            Ok(Some(capture))
        }
        ScreenContextMode::Ocr => {
            // For OCR mode we still grab the AX app metadata cheaply
            // (bundle_id + app_name don't require traversing the tree),
            // but we don't use AX text at all.
            let (bundle_id, app_name) = capture_ax_meta_only().await;
            let Some(text) = capture_ocr().await else {
                return Ok(None);
            };
            if text.trim().is_empty() {
                return Ok(None);
            }
            let capture = ContextCapture {
                text,
                source: CaptureSource::Ocr,
                bundle_id,
                app_name,
                latency_ms: started.elapsed().as_millis() as u64,
            };
            emit_captured(&app, &capture);
            Ok(Some(capture))
        }
    }
}

/// Lightweight AX query for just the frontmost app's bundle_id + name.
/// Skips the full traversal entirely so OCR-mode users don't pay AX cost.
async fn capture_ax_meta_only() -> (Option<String>, Option<String>) {
    let work = tokio::task::spawn_blocking(|| -> (Option<String>, Option<String>) {
        unsafe {
            let system = ax_ffi::AXUIElementCreateSystemWide();
            if system.is_null() {
                return (None, None);
            }
            let Some(app) = ax_copy_element(system, "AXFocusedApplication") else {
                ax_release(system);
                return (None, None);
            };
            let mut pid: i32 = 0;
            let result = if ax_ffi::AXUIElementGetPid(app, &mut pid) == ax_ffi::K_AX_ERROR_SUCCESS
                && pid > 0
            {
                running_app_info(pid)
            } else {
                (None, None)
            };
            ax_release(app);
            ax_release(system);
            result
        }
    });
    match tokio::time::timeout(Duration::from_millis(100), work).await {
        Ok(Ok(meta)) => meta,
        _ => (None, None),
    }
}

/// Emit the `context:captured` event with metadata + proper-noun
/// candidates. We intentionally never put the raw captured text on the
/// event bus — it may contain anything (passwords, PII, drafts) — and
/// the candidate list is already filtered to ≤50 strings.
fn emit_captured<R: tauri::Runtime>(app: &tauri::AppHandle<R>, capture: &ContextCapture) {
    let candidates = proper_nouns::extract_candidates(&capture.text, 50);
    let raw_text_len = capture.text.chars().count();
    log::info!(
        "[vibeking screen_context] captured source={:?} app={:?} raw_chars={} candidates={} latency={}ms",
        capture.source,
        capture.app_name.as_deref().unwrap_or("?"),
        raw_text_len,
        candidates.len(),
        capture.latency_ms,
    );
    let payload = ContextCapturedEvent {
        candidates,
        source: capture.source,
        bundle_id: capture.bundle_id.clone(),
        app_name: capture.app_name.clone(),
        latency_ms: capture.latency_ms,
        raw_text_len,
    };
    let _ = app.emit("context:captured", payload);
}

// ---------------------------------------------------------------------------
// AX traversal (async wrapper over blocking work)
// ---------------------------------------------------------------------------

/// Result of an AX capture attempt, populated by the blocking worker.
struct AxCapture {
    text: String,
    bundle_id: Option<String>,
    app_name: Option<String>,
}

/// Run the AX traversal on a blocking thread with a hard timeout.
/// Always returns `Some` with whatever was collected, or `None` on
/// timeout / panic / failure.
async fn capture_ax() -> Option<AxCapture> {
    let work = tokio::task::spawn_blocking(capture_ax_blocking);
    match tokio::time::timeout(AX_BUDGET, work).await {
        Ok(Ok(result)) => result,
        Ok(Err(join_err)) => {
            log::warn!("[screen_context] AX worker panicked: {join_err}");
            None
        }
        Err(_) => {
            log::warn!("[screen_context] AX traversal exceeded {:?}", AX_BUDGET);
            None
        }
    }
}

/// Synchronous AX traversal — the heavy lifting.
///
/// Strategy (in order, first success wins):
///   1. The focused element's own `AXValue` — for textareas / text fields
///      that the cursor is sitting in, this is the exact content the user is
///      replying to or composing inside.
///   2. Walk up from the focused element to the smallest enclosing **content
///      container** (`AXWebArea`, `AXTextArea`, `AXScrollArea`, `AXTable`,
///      `AXOutline`, `AXList`, `AXSheet`). Traverse that subtree with the
///      role filter applied — this scopes us to actual page/document
///      content and skips browser chrome.
///   3. Fall back to the focused window with the role filter — still skips
///      toolbars / menubars / extensions / bookmark bar.
fn capture_ax_blocking() -> Option<AxCapture> {
    if !unsafe { ax_ffi::AXIsProcessTrusted() } {
        return None;
    }

    let mut out = AxCapture {
        text: String::new(),
        bundle_id: None,
        app_name: None,
    };

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
        if ax_ffi::AXUIElementGetPid(app, &mut pid) == ax_ffi::K_AX_ERROR_SUCCESS && pid > 0 {
            let (b, n) = running_app_info(pid);
            out.bundle_id = b;
            out.app_name = n;
        }

        let focused = ax_copy_element(app, "AXFocusedUIElement");

        // ---- Strategy 1: focused element's own value -----------------------
        if let Some(f) = focused {
            if let Some(val) = ax_copy_string(f, "AXValue") {
                let trimmed = val.trim();
                if trimmed.chars().count() >= MIN_USEFUL_LEN {
                    out.text = trimmed.to_string();
                    ax_release(f);
                    ax_release(app);
                    ax_release(system);
                    return Some(out);
                }
            }

            // ---- Strategy 2: nearest content-container ancestor ------------
            if let Some(container) = find_content_ancestor(f) {
                let mut state = TraversalState {
                    nodes_visited: 0,
                    fragments: Vec::new(),
                };
                traverse_content(container, 0, &mut state);
                if !state.fragments.is_empty() {
                    out.text = state.fragments.join(" \n");
                }
                ax_release(container);
            }

            ax_release(f);
        }

        // ---- Strategy 3: focused window with role filter -------------------
        if out.text.chars().count() < MIN_USEFUL_LEN {
            if let Some(w) = ax_copy_element(app, "AXFocusedWindow") {
                let mut state = TraversalState {
                    nodes_visited: 0,
                    fragments: Vec::new(),
                };
                traverse_content(w, 0, &mut state);
                if !state.fragments.is_empty() {
                    out.text = state.fragments.join(" \n");
                }
                ax_release(w);
            }
        }

        ax_release(app);
        ax_release(system);
    }

    Some(out)
}

struct TraversalState {
    nodes_visited: u32,
    fragments: Vec<String>,
}

/// AX roles we never collect from or descend into. These are pure chrome —
/// in browsers they pull in toolbar buttons, extension icons, bookmarks bar,
/// sidebar items, etc., which crowd out the actual page content.
const CHROME_ROLES: &[&str] = &[
    "AXButton",
    "AXMenuButton",
    "AXPopUpButton",
    "AXCheckBox",
    "AXRadioButton",
    "AXToolbar",
    "AXMenuBar",
    "AXMenuBarItem",
    "AXMenu",
    "AXMenuItem",
    "AXTab",
    "AXTabGroup",
    "AXScrollBar",
    "AXSplitter",
    "AXIncrementor",
    "AXImage",
    "AXBusyIndicator",
    "AXProgressIndicator",
    "AXDisclosureTriangle",
];

/// AX roles considered meaningful "content containers" — the smallest such
/// ancestor of the focused element scopes our traversal to actual document
/// content rather than the whole window.
const CONTENT_CONTAINER_ROLES: &[&str] = &[
    "AXWebArea",
    "AXTextArea",
    "AXScrollArea",
    "AXTable",
    "AXOutline",
    "AXList",
    "AXSheet",
];

/// Bounded walk up via `AXParent` looking for a content-container ancestor.
/// Returns a +1 retained ref the caller must release, or `None`.
unsafe fn find_content_ancestor(focused: ax_ffi::AXUIElementRef) -> Option<ax_ffi::AXUIElementRef> {
    if focused.is_null() {
        return None;
    }

    // Check the focused element itself first.
    if let Some(role) = unsafe { ax_copy_string(focused, "AXRole") } {
        if CONTENT_CONTAINER_ROLES.contains(&role.as_str()) {
            unsafe { ax_ffi::CFRetain(focused as core_foundation::base::CFTypeRef) };
            return Some(focused);
        }
    }

    let mut current: Option<ax_ffi::AXUIElementRef> =
        unsafe { ax_copy_element(focused, "AXParent") };

    for _ in 0..16 {
        let Some(cur) = current else {
            return None;
        };
        if let Some(role) = unsafe { ax_copy_string(cur, "AXRole") } {
            if CONTENT_CONTAINER_ROLES.contains(&role.as_str()) {
                return Some(cur); // ownership transferred to caller
            }
        }
        let parent = unsafe { ax_copy_element(cur, "AXParent") };
        unsafe { ax_release(cur) };
        current = parent;
    }

    if let Some(c) = current {
        unsafe { ax_release(c) };
    }
    None
}

unsafe fn traverse_content(
    element: ax_ffi::AXUIElementRef,
    depth: u32,
    state: &mut TraversalState,
) {
    if state.nodes_visited >= MAX_NODES || depth > MAX_DEPTH || element.is_null() {
        return;
    }
    state.nodes_visited += 1;

    // Skip pure-chrome roles entirely (don't collect, don't descend).
    if let Some(role) = unsafe { ax_copy_string(element, "AXRole") } {
        if CHROME_ROLES.contains(&role.as_str()) {
            return;
        }
    }

    for attr in ["AXValue", "AXSelectedText", "AXTitle", "AXDescription"] {
        if let Some(s) = unsafe { ax_copy_string(element, attr) } {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                state.fragments.push(trimmed.to_string());
            }
        }
    }

    let children = unsafe { ax_copy_children(element) };
    for child in children {
        unsafe {
            traverse_content(child, depth + 1, state);
            ax_release(child);
        }
        if state.nodes_visited >= MAX_NODES {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// OCR fallback
// ---------------------------------------------------------------------------

async fn capture_ocr() -> Option<String> {
    let fut = crate::screen_ocr::capture_focused_window_text();
    match tokio::time::timeout(OCR_BUDGET, fut).await {
        Ok(Ok(Some(text))) => Some(text),
        Ok(Ok(None)) => None,
        Ok(Err(e)) => {
            log::warn!("[screen_context] OCR fallback failed: {e}");
            None
        }
        Err(_) => {
            log::warn!("[screen_context] OCR fallback exceeded {:?}", OCR_BUDGET);
            None
        }
    }
}

// ---------------------------------------------------------------------------
// AX helpers (reused from examples/ax_probe.rs)
// ---------------------------------------------------------------------------

/// Copy a CFString attribute from an AX element.
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

/// Copy an AX element attribute. Returns a raw +1 AXUIElementRef — caller
/// is responsible for `ax_release`.
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

/// Copy the AX children array. Returns owned +1 refs.
unsafe fn ax_copy_children(element: ax_ffi::AXUIElementRef) -> Vec<ax_ffi::AXUIElementRef> {
    if element.is_null() {
        return Vec::new();
    }
    let key = CFString::new("AXChildren");
    let mut value: CFTypeRef = std::ptr::null();
    let err = unsafe {
        ax_ffi::AXUIElementCopyAttributeValue(element, key.as_concrete_TypeRef(), &mut value)
    };
    if err != ax_ffi::K_AX_ERROR_SUCCESS || value.is_null() {
        return Vec::new();
    }
    let type_id = unsafe { ax_ffi::CFGetTypeID(value) };
    if type_id != unsafe { ax_ffi::CFArrayGetTypeID() } {
        unsafe { ax_ffi::CFRelease(value) };
        return Vec::new();
    }
    let array: CFArray<CFType> = unsafe { TCFType::wrap_under_create_rule(value as CFArrayRef) };
    let mut out = Vec::with_capacity(array.len() as usize);
    for v in array.get_all_values() {
        // CFRetain each entry so it outlives the array we just unwrapped.
        unsafe {
            ax_ffi::CFRetain(v as CFTypeRef);
        }
        out.push(v as ax_ffi::AXUIElementRef);
    }
    out
}

unsafe fn ax_release(element: ax_ffi::AXUIElementRef) {
    if !element.is_null() {
        unsafe { ax_ffi::CFRelease(element as CFTypeRef) };
    }
}

/// Look up the frontmost app's bundle id + localized name via NSRunningApplication,
/// avoiding the need for the NSWorkspace/NSRunningApplication features on
/// `objc2-app-kit`.
unsafe fn running_app_info(pid: i32) -> (Option<String>, Option<String>) {
    let cls = class!(NSRunningApplication);
    let app: *mut AnyObject =
        unsafe { msg_send![cls, runningApplicationWithProcessIdentifier: pid] };
    if app.is_null() {
        return (None, None);
    }
    let bundle_id_ns: *mut NSString = unsafe { msg_send![app, bundleIdentifier] };
    let name_ns: *mut NSString = unsafe { msg_send![app, localizedName] };
    let bundle_id = if bundle_id_ns.is_null() {
        None
    } else {
        Some(unsafe { (*bundle_id_ns).to_string() })
    };
    let name = if name_ns.is_null() {
        None
    } else {
        Some(unsafe { (*name_ns).to_string() })
    };
    (bundle_id, name)
}

// ---------------------------------------------------------------------------
// FFI bindings
// ---------------------------------------------------------------------------

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
        pub fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> AXError;
        pub fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> AXError;
        pub fn AXIsProcessTrusted() -> bool;
        pub fn CFGetTypeID(cf: CFTypeRef) -> usize;
        pub fn CFStringGetTypeID() -> usize;
        pub fn CFArrayGetTypeID() -> usize;
        pub fn CFRelease(cf: CFTypeRef);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_ax_trusted_returns_a_bool_without_panicking() {
        // We don't assert the value — it depends on the test environment's
        // accessibility permissions — but the call must not panic.
        let _trusted: bool = is_ax_trusted();
    }

    #[test]
    fn public_api_compiles() {
        // Smoke test: ensure the public function signature is what the
        // caller expects. `capture` is generic over `tauri::Runtime`,
        // takes an `AppHandle` plus a `ScreenContextMode`.
        let _: fn(tauri::AppHandle<tauri::Wry>, ScreenContextMode) -> _ = capture;
    }

    #[test]
    fn min_useful_len_is_twenty() {
        // Sanity check on the threshold the AX focused-element fast-path uses.
        assert_eq!(MIN_USEFUL_LEN, 20);
    }
}
