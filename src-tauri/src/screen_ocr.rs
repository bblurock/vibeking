//! Vision-framework OCR fallback for the focused window.
//!
//! When the AX traversal returns nothing useful (sandboxed apps, image-heavy
//! PDFs, Discord/Slack chrome that hides body text), we fall back to taking
//! a screenshot of the focused window and running on-device OCR via Apple's
//! Vision framework.
//!
//! Triggers the macOS Screen Recording permission the first time it runs.
//! On permission denial, `CGWindowListCreateImage` returns NULL — we detect
//! that and return `Ok(None)` rather than surfacing an error.
//!
//! All of this runs synchronously inside a `spawn_blocking` worker so the
//! Vision call doesn't park the tokio runtime. The caller in
//! `screen_context.rs` also wraps us with a 500 ms timeout for safety.

use anyhow::Result;
use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::TCFType;
use core_foundation::dictionary::CFDictionaryRef;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::window::{
    kCGNullWindowID, kCGWindowImageBestResolution, kCGWindowImageBoundsIgnoreFraming,
    kCGWindowLayer, kCGWindowListExcludeDesktopElements, kCGWindowListOptionIncludingWindow,
    kCGWindowListOptionOnScreenOnly, kCGWindowNumber, kCGWindowOwnerPID, CGWindowID,
};
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use std::ffi::c_void;

/// True if the window with `window_id` is currently composited on screen.
///
/// `CGWindowListCopyWindowInfo(OnScreenOnly | IncludingWindow, id)` returns the
/// window's info ONLY when it is actually on screen — so an empty result means
/// the window is off-screen / stranded. This is the authoritative signal (the same
/// `kCGWindowIsOnscreen` truth an external `CGWindowList` dump reports), unlike
/// `NSWindow.isVisible` / `isOnActiveSpace` / `occlusionState`, all of which lie
/// for a display-stranded window. Used to detect the chip-bar stranding bug and to
/// verify a recreation actually recovered it.
pub(crate) fn window_is_onscreen(window_id: CGWindowID) -> bool {
    let array_ref: CFArrayRef = unsafe {
        ffi::CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly | kCGWindowListOptionIncludingWindow,
            window_id,
        )
    };
    if array_ref.is_null() {
        return false;
    }
    let array: CFArray = unsafe { CFArray::wrap_under_create_rule(array_ref) };
    !array.get_all_values().is_empty()
}

/// Capture the focused window via CGWindowList, then OCR it with Vision.
///
/// Returns:
///   - `Ok(Some(text))` when OCR succeeded and produced non-empty text.
///   - `Ok(None)` on: no frontmost app, no captureable window, screen-recording
///     permission denied, empty OCR result, OR any non-catastrophic failure.
///   - `Err` only for genuinely unexpected things (none expected in practice;
///     all known failure modes are downgraded to `Ok(None)`).
pub async fn capture_focused_window_text() -> Result<Option<String>> {
    let result =
        tokio::task::spawn_blocking(|| -> Result<Option<String>> { capture_blocking() }).await;

    match result {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => {
            log::warn!("[screen_ocr] vision failed: {e:#}");
            Ok(None)
        }
        Err(e) => {
            log::warn!("[screen_ocr] task panicked: {e:?}");
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// Sync pipeline: frontmost PID -> window ID -> CGImage -> Vision OCR
// ---------------------------------------------------------------------------

fn capture_blocking() -> Result<Option<String>> {
    let Some(pid) = frontmost_app_pid() else {
        log::debug!("[screen_ocr] no frontmost application");
        return Ok(None);
    };

    let Some(window_id) = focused_window_id_for_pid(pid) else {
        log::debug!("[screen_ocr] no on-screen window for pid {pid}");
        return Ok(None);
    };

    // `CGRectNull` is the documented sentinel meaning "use the window's own
    // bounds". We construct it manually since the constant isn't re-exported
    // by core-graphics. CGRectNull = origin (Inf, Inf), zero size.
    let null_rect = CGRect {
        origin: CGPoint {
            x: f64::INFINITY,
            y: f64::INFINITY,
        },
        size: CGSize {
            width: 0.0,
            height: 0.0,
        },
    };

    // We bypass the safe `core_graphics::window::create_image` wrapper because
    // it returns a `CGImage` newtype that doesn't expose its raw pointer
    // without a trait whose crate we can't add. Going through raw FFI is
    // straightforward and gives us a `CGImageRef` we can hand to Vision
    // directly, then explicitly release.
    let cg_image_ptr: *mut c_void = unsafe {
        ffi::CGWindowListCreateImage(
            null_rect,
            kCGWindowListOptionIncludingWindow,
            window_id,
            kCGWindowImageBoundsIgnoreFraming | kCGWindowImageBestResolution,
        )
    };
    if cg_image_ptr.is_null() {
        // Most common cause: screen-recording permission not granted. The
        // system will have already shown the TCC prompt on first call.
        log::debug!("[screen_ocr] CGWindowListCreateImage returned null (perm denied?)");
        return Ok(None);
    }

    // From here on we own a +1 retained CGImageRef and MUST release it exactly
    // once on every exit path. RAII guard handles both the success path and
    // any (theoretical) panic inside the Vision call.
    struct CGImageGuard(*mut c_void);
    impl Drop for CGImageGuard {
        fn drop(&mut self) {
            unsafe { ffi::CGImageRelease(self.0) }
        }
    }
    let _image_guard = CGImageGuard(cg_image_ptr);

    let text = unsafe { run_vision_ocr(cg_image_ptr) };
    Ok(text)
}

// ---------------------------------------------------------------------------
// Frontmost-app PID via NSWorkspace
// ---------------------------------------------------------------------------

/// Returns the PID of the frontmost (foreground) application, or `None` if
/// there isn't one (e.g. Mission Control overlay, kiosk-mode lockdown).
fn frontmost_app_pid() -> Option<i32> {
    unsafe {
        let workspace_cls = class!(NSWorkspace);
        let workspace: *mut AnyObject = msg_send![workspace_cls, sharedWorkspace];
        if workspace.is_null() {
            return None;
        }
        let app: *mut AnyObject = msg_send![workspace, frontmostApplication];
        if app.is_null() {
            return None;
        }
        let pid: i32 = msg_send![app, processIdentifier];
        if pid > 0 {
            Some(pid)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// CGWindowList -> focused window ID
// ---------------------------------------------------------------------------

/// Iterate on-screen windows and return the window number belonging to the
/// frontmost app whose window layer is 0 (a normal app window, not a menu
/// bar item or HUD).
///
/// We can't ask AX for the window number directly — AX gives us a window
/// element but no `kCGWindowNumber`. So we filter the on-screen window list
/// by PID + layer instead. The list is roughly Z-ordered (front-most first),
/// so the first match is the right one.
fn focused_window_id_for_pid(target_pid: i32) -> Option<CGWindowID> {
    // Use the raw CFArrayRef so we can read its entries as plain
    // CFDictionaryRefs without forcing core-foundation's generic type
    // parameters to line up.
    let array_ref: CFArrayRef = unsafe {
        ffi::CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID,
        )
    };
    if array_ref.is_null() {
        return None;
    }
    // Take ownership so it CFReleases on drop.
    let array: CFArray = unsafe { CFArray::wrap_under_create_rule(array_ref) };

    for value in array.get_all_values() {
        let dict_ref = value as CFDictionaryRef;
        if dict_ref.is_null() {
            continue;
        }

        // PID match. CFDictionaryGetValue returns a borrowed (+0) pointer —
        // do NOT release it.
        let pid_val = unsafe { ffi::CFDictionaryGetValue(dict_ref, kCGWindowOwnerPID as _) };
        let Some(pid) = read_cf_number_i32(pid_val) else {
            continue;
        };
        if pid != target_pid {
            continue;
        }

        // Layer must be 0 (normal application window). Higher layers are
        // status bars, menubars, transient panels, etc.
        let layer_val = unsafe { ffi::CFDictionaryGetValue(dict_ref, kCGWindowLayer as _) };
        let layer = read_cf_number_i32(layer_val).unwrap_or(i32::MAX);
        if layer != 0 {
            continue;
        }

        // kCGWindowNumber — the actual window id to pass to
        // CGWindowListCreateImage. Stored as a 64-bit number per the docs.
        let number_val = unsafe { ffi::CFDictionaryGetValue(dict_ref, kCGWindowNumber as _) };
        if let Some(n) = read_cf_number_i64(number_val) {
            if n > 0 && n <= u32::MAX as i64 {
                return Some(n as CGWindowID);
            }
        }
    }

    None
}

/// Read a CFNumber pointer as an i32. Returns `None` on null / wrong type.
fn read_cf_number_i32(ptr: *const c_void) -> Option<i32> {
    if ptr.is_null() {
        return None;
    }
    let mut out: i32 = 0;
    let ok = unsafe {
        ffi::CFNumberGetValue(
            ptr as ffi::CFNumberRef,
            ffi::K_CF_NUMBER_SINT32_TYPE,
            &mut out as *mut i32 as *mut c_void,
        )
    };
    if ok {
        Some(out)
    } else {
        None
    }
}

/// Read a CFNumber pointer as an i64. Returns `None` on null / wrong type.
fn read_cf_number_i64(ptr: *const c_void) -> Option<i64> {
    if ptr.is_null() {
        return None;
    }
    let mut out: i64 = 0;
    let ok = unsafe {
        ffi::CFNumberGetValue(
            ptr as ffi::CFNumberRef,
            ffi::K_CF_NUMBER_SINT64_TYPE,
            &mut out as *mut i64 as *mut c_void,
        )
    };
    if ok {
        Some(out)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Vision OCR via raw obj-c messaging
// ---------------------------------------------------------------------------

#[link(name = "Vision", kind = "framework")]
extern "C" {}

/// Run Vision's `VNRecognizeTextRequest` synchronously over the given
/// CGImage pointer. Returns joined text or `None` if no observations.
///
/// SAFETY: `cg_image_ptr` must be a valid, retained CGImageRef owned by the
/// caller for the duration of this call. Vision does not consume it.
unsafe fn run_vision_ocr(cg_image_ptr: *mut c_void) -> Option<String> {
    // ---- 1. Build VNRecognizeTextRequest ------------------------------
    let request: *mut AnyObject = msg_send![class!(VNRecognizeTextRequest), new];
    if request.is_null() {
        return None;
    }
    // `new` returns +1 retained. We release at the end / on every early exit.

    // .fast = 0; .accurate = 1. Fast is ~5x quicker and plenty for proper
    // noun extraction (we don't need character-perfect transcription).
    let _: () = msg_send![request, setRecognitionLevel: 0i64];
    let _: () = msg_send![request, setUsesLanguageCorrection: false];

    // ---- 2. Build VNImageRequestHandler -------------------------------
    let alloc: *mut AnyObject = msg_send![class!(VNImageRequestHandler), alloc];
    if alloc.is_null() {
        let _: () = msg_send![request, release];
        return None;
    }
    let handler: *mut AnyObject = msg_send![
        alloc,
        initWithCGImage: cg_image_ptr,
        options: std::ptr::null::<AnyObject>(),
    ];
    if handler.is_null() {
        // alloc consumed by failed init — don't double-release.
        let _: () = msg_send![request, release];
        return None;
    }

    // ---- 3. Wrap the request in an NSArray ----------------------------
    // `arrayWithObject:` returns an autoreleased NSArray — we do not release.
    let requests_array: *mut AnyObject = msg_send![
        class!(NSArray),
        arrayWithObject: request,
    ];
    if requests_array.is_null() {
        let _: () = msg_send![handler, release];
        let _: () = msg_send![request, release];
        return None;
    }

    // ---- 4. Perform synchronously -------------------------------------
    let mut error_ptr: *mut AnyObject = std::ptr::null_mut();
    let ok: bool = msg_send![
        handler,
        performRequests: requests_array,
        error: &mut error_ptr,
    ];

    if !ok {
        if !error_ptr.is_null() {
            log::debug!("[screen_ocr] Vision performRequests returned error");
        }
        let _: () = msg_send![handler, release];
        let _: () = msg_send![request, release];
        return None;
    }

    // ---- 5. Collect results -------------------------------------------
    // `-results` returns an autoreleased NSArray, no release needed.
    let results: *mut AnyObject = msg_send![request, results];
    if results.is_null() {
        let _: () = msg_send![handler, release];
        let _: () = msg_send![request, release];
        return None;
    }

    let count: usize = msg_send![results, count];
    let mut lines: Vec<String> = Vec::with_capacity(count);

    for i in 0..count {
        let obs: *mut AnyObject = msg_send![results, objectAtIndex: i];
        if obs.is_null() {
            continue;
        }
        let candidates: *mut AnyObject = msg_send![obs, topCandidates: 1usize];
        if candidates.is_null() {
            continue;
        }
        let cand_count: usize = msg_send![candidates, count];
        if cand_count == 0 {
            continue;
        }
        let top: *mut AnyObject = msg_send![candidates, objectAtIndex: 0usize];
        if top.is_null() {
            continue;
        }
        let s: *mut AnyObject = msg_send![top, string];
        if s.is_null() {
            continue;
        }
        // -[NSString UTF8String] returns a *const c_char with no ownership —
        // backed by the NSString's autorelease pool entry. Copy immediately
        // into a Rust String to escape the lifetime.
        let utf8: *const std::os::raw::c_char = msg_send![s, UTF8String];
        if utf8.is_null() {
            continue;
        }
        if let Ok(rs) = std::ffi::CStr::from_ptr(utf8).to_str() {
            let trimmed = rs.trim();
            if !trimmed.is_empty() {
                lines.push(trimmed.to_string());
            }
        }
    }

    // ---- 6. Release -------------------------------------------------
    let _: () = msg_send![handler, release];
    let _: () = msg_send![request, release];

    let joined = lines.join(" ").trim().to_string();
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

// ---------------------------------------------------------------------------
// Raw FFI bindings
// ---------------------------------------------------------------------------

mod ffi {
    use core_foundation::array::CFArrayRef;
    use core_foundation::base::CFTypeRef;
    use core_foundation::dictionary::CFDictionaryRef;
    use core_graphics::geometry::CGRect;
    use core_graphics::window::{CGWindowID, CGWindowImageOption, CGWindowListOption};
    use std::ffi::c_void;

    // CFNumberType enum value for kCFNumberSInt32Type (Apple docs).
    pub const K_CF_NUMBER_SINT32_TYPE: i64 = 3;
    pub const K_CF_NUMBER_SINT64_TYPE: i64 = 4;

    pub type CFNumberRef = *const c_void;
    pub type CFNumberType = i64;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        pub fn CGWindowListCreateImage(
            screen_bounds: CGRect,
            list_option: CGWindowListOption,
            window_id: CGWindowID,
            image_option: CGWindowImageOption,
        ) -> *mut c_void; // CGImageRef

        pub fn CGImageRelease(image: *mut c_void);

        pub fn CGWindowListCopyWindowInfo(
            option: CGWindowListOption,
            relative_to_window: CGWindowID,
        ) -> CFArrayRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub fn CFDictionaryGetValue(dict: CFDictionaryRef, key: *const c_void) -> *const c_void;

        pub fn CFNumberGetValue(
            number: CFNumberRef,
            the_type: CFNumberType,
            value_ptr: *mut c_void,
        ) -> bool;

        // Currently unused — kept for documentation / future use.
        #[allow(dead_code)]
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
    fn public_api_signature_compiles() {
        // The caller in screen_context.rs depends on this exact signature.
        let _: fn() -> _ = capture_focused_window_text;
    }
}
