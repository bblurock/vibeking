//! macOS-specific NSWindow tweaks for the chip bar.

use core::ffi::{c_char, c_void};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use block2::RcBlock;
use objc2::ffi::object_setClass;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send, ClassType};
use objc2_app_kit::{NSPanel, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use tauri::{Manager, Runtime, WebviewWindow};

const NS_STATUS_WINDOW_LEVEL: i64 = 25;

/// Re-apply the chip bar's *floating* configuration: status window level,
/// join-all-Spaces, full-screen-auxiliary, stationary, ignores-cycle, and
/// don't-hide-on-deactivate. macOS silently drops these across display
/// reconfiguration (sleep/wake, external-monitor connect/disconnect) and Space
/// switches — which is exactly what made the always-on bar fall behind every
/// window (or off the active Space) and stay gone until an app relaunch. It's a
/// handful of cheap, idempotent message sends, so it's safe to re-run on every
/// show/resize and on the system notifications that trigger the drop.
///
/// Caller must hold a non-null `*mut NSWindow` and be on the main thread.
pub(crate) unsafe fn reassert_floating(ns_window: *mut NSWindow) {
    let _: () = msg_send![ns_window, setLevel: NS_STATUS_WINDOW_LEVEL];
    let behavior: NSWindowCollectionBehavior = NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::FullScreenAuxiliary
        | NSWindowCollectionBehavior::Stationary
        | NSWindowCollectionBehavior::IgnoresCycle;
    let _: () = msg_send![ns_window, setCollectionBehavior: behavior];
    let _: () = msg_send![ns_window, setHidesOnDeactivate: false];
}

pub fn configure_chipbar<R: Runtime>(window: &WebviewWindow<R>) {
    let Ok(ns_window_ptr) = window.ns_window() else {
        return;
    };
    let ns_window = ns_window_ptr as *mut NSWindow;
    if ns_window.is_null() {
        return;
    }

    unsafe {
        // Re-class the chip-bar NSWindow as an NSPanel so it can be made
        // *non-activating*. This is what lets the always-on idle bar work:
        // clicking its mic must NOT steal key focus from the user's target app,
        // otherwise the post-dictation ⌘V paste would land in the chip bar
        // instead of where the user was typing. `becomesKeyOnlyIfNeeded` keeps
        // the correction-prompt text field usable — the panel still takes key
        // focus when the user clicks a control that genuinely needs it.
        //
        // NSPanel is a subclass of NSWindow, so existing NSWindow message sends
        // (level, collection behavior, …) remain valid after the reclass. This
        // is the same technique the `tauri-nspanel` plugin uses.
        object_setClass(
            ns_window as *mut objc2::runtime::AnyObject,
            NSPanel::class() as *const _ as *mut _,
        );
        let style: NSWindowStyleMask = msg_send![ns_window, styleMask];
        let _: () = msg_send![
            ns_window,
            setStyleMask: style | NSWindowStyleMask::NonactivatingPanel
        ];
        let _: () = msg_send![ns_window, setBecomesKeyOnlyIfNeeded: true];
        let _: () = msg_send![ns_window, setFloatingPanel: true];

        // The level + collection-behavior + hides-on-deactivate flags live in the
        // shared re-assert helper so the watchdog (below) restores the exact same
        // configuration macOS strips on Space/display changes.
        reassert_floating(ns_window);
        let _: () = msg_send![ns_window, setMovableByWindowBackground: false];
    }
}

/// Register native observers that re-assert the chip bar's floating config when
/// macOS would otherwise drop it. Without this the always-on bar permanently
/// falls behind / off the active Space after any of these events, and stays gone
/// until the app is relaunched (it's one window that's never ordered out, so a
/// re-record can't recover it either). The observer tokens live for the whole
/// app lifetime (intentionally retained-and-leaked — we never unregister). The
/// blocks fire on the main thread, where these notifications are posted.
pub fn install_chipbar_watchdog<R: Runtime>(app: &tauri::AppHandle<R>) {
    unsafe {
        // Space switches + wake-from-sleep are NSWorkspace notifications, posted
        // to the workspace's own notification center.
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        if !workspace.is_null() {
            let ws_center: *mut AnyObject = msg_send![workspace, notificationCenter];
            register(app, ws_center, b"NSWorkspaceActiveSpaceDidChangeNotification\0", false);
            register(app, ws_center, b"NSWorkspaceDidWakeNotification\0", true);
        }
        // Display reconfiguration (external monitor connect/disconnect, resolution
        // change) is an NSApplication notification on the default center. This is
        // the trigger that STRANDS the chip off the active Space (onscreen=false
        // with level/alpha/bounds all still correct) — re-asserting collection
        // behavior and ordering-front don't recover it; only a re-order does. So
        // it gets the heavier debounced recovery (see `on_system_event`).
        let def_center: *mut AnyObject = msg_send![class!(NSNotificationCenter), defaultCenter];
        register(
            app,
            def_center,
            b"NSApplicationDidChangeScreenParametersNotification\0",
            true,
        );
    }
}

/// Add one block-based observer for `name` (a nul-terminated ASCII notification
/// name) to `center`, re-asserting the chip bar's floating config when it fires.
/// `display_change` marks notifications (screen-params, wake) that can strand the
/// window off the active Space and therefore need the debounced re-order recovery.
unsafe fn register<R: Runtime>(
    app: &tauri::AppHandle<R>,
    center: *mut AnyObject,
    name: &[u8],
    display_change: bool,
) {
    if center.is_null() {
        return;
    }
    let ns_name: *mut AnyObject = msg_send![
        class!(NSString),
        stringWithUTF8String: name.as_ptr() as *const c_char
    ];
    if ns_name.is_null() {
        return;
    }
    let app = app.clone();
    // The block ABI here is `void(^)(NSNotification *)`; we ignore the payload, so
    // a single opaque pointer arg matches the calling convention.
    let block = RcBlock::new(move |_notif: *mut c_void| {
        on_system_event(&app, display_change);
    });
    let token: *mut AnyObject = msg_send![
        center,
        addObserverForName: ns_name,
        object: null_mut::<AnyObject>(),
        queue: null_mut::<AnyObject>(),
        usingBlock: &*block,
    ];
    // Keep the registration alive for the app's lifetime.
    if !token.is_null() {
        let _: *mut AnyObject = msg_send![token, retain];
    }
}

/// React to a Space/display/wake notification. Always does the cheap, flicker-free
/// re-assert (level + collection behavior + reposition). On a `display_change`
/// (which can strand the window off the active Space — confirmed: `onscreen=false`
/// with everything else healthy, recoverable only by a re-order, not by `show()`),
/// it also schedules a debounced hide()+show() recovery.
fn on_system_event<R: Runtime>(app: &tauri::AppHandle<R>, display_change: bool) {
    let Some(window) = app.get_webview_window(crate::windows::CHIPBAR_LABEL) else {
        return;
    };
    let win = window.clone();
    let _ = window.run_on_main_thread(move || {
        let Ok(ptr) = win.ns_window() else {
            return;
        };
        let ns_window = ptr as *mut NSWindow;
        if ns_window.is_null() {
            return;
        }
        unsafe {
            reassert_floating(ns_window);
            ensure_on_screen(ns_window);
            // Only re-front a window that's actually meant to be visible — never
            // un-hide the chip in the non-persistent flow where idle == hidden.
            let visible: bool = msg_send![ns_window, isVisible];
            if visible {
                let _: () = msg_send![ns_window, orderFrontRegardless];
            }
        }
    });

    if display_change {
        schedule_display_recovery(app);
    }
}

/// Coalesces the burst of notifications a single display reconfiguration emits.
static DISPLAY_RECOVERY_GEN: AtomicU64 = AtomicU64::new(0);

/// True if the chip-bar window is actually composited on screen — the reliable
/// `CGWindowList` truth, not the lying `isVisible`/`occlusionState`. Used to detect
/// the stranding and to verify a recreation recovered it.
pub(crate) fn chipbar_onscreen<R: Runtime>(window: &WebviewWindow<R>) -> bool {
    let Ok(ptr) = window.ns_window() else {
        return false;
    };
    let ns_window = ptr as *mut NSWindow;
    if ns_window.is_null() {
        return false;
    }
    let num: i64 = unsafe { msg_send![ns_window, windowNumber] };
    if num <= 0 {
        return false;
    }
    crate::screen_ocr::window_is_onscreen(num as u32)
}

/// After a display change settles, detect whether the always-on bar got stranded
/// (`CGWindowList` says off-screen while it should be visible) and, if so, recover
/// it by recreating the window the correct way (see `windows::recreate_chipbar`).
/// Debounced so a resolution change's flurry of notifications triggers ONE check.
fn schedule_display_recovery<R: Runtime>(app: &tauri::AppHandle<R>) {
    let gen = DISPLAY_RECOVERY_GEN.fetch_add(1, Ordering::Relaxed) + 1;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(900)).await;
        if DISPLAY_RECOVERY_GEN.load(Ordering::Relaxed) != gen {
            return; // superseded by a later notification in the same burst
        }
        // Only the always-on bar is "supposed to be visible while idle", so it's the
        // only case where off-screen unambiguously means the bug (the non-persistent
        // chip is legitimately hidden at idle).
        if !crate::windows::persistent_bar_enabled(&app) {
            return;
        }
        let Some(window) = app.get_webview_window(crate::windows::CHIPBAR_LABEL) else {
            return;
        };
        if chipbar_onscreen(&window) {
            return; // still composited — the display change didn't strand it
        }
        log::info!("[vibeking chipbar] display change stranded the bar (off-screen) — recovering");
        crate::windows::recreate_chipbar(&app);
    });
}

/// If the window's frame no longer intersects any screen (e.g. the external
/// display it lived on was unplugged), recenter it horizontally near the bottom
/// of the primary screen at its current size. A normal apply() refines the exact
/// anchor on the next recording; this just guarantees it isn't lost off-screen.
unsafe fn ensure_on_screen(ns_window: *mut NSWindow) {
    let frame: NSRect = msg_send![ns_window, frame];
    let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
    if screens.is_null() {
        return;
    }
    let count: usize = msg_send![screens, count];
    if count == 0 {
        return;
    }
    let mut primary_visible = NSRect {
        origin: NSPoint { x: 0.0, y: 0.0 },
        size: NSSize {
            width: 0.0,
            height: 0.0,
        },
    };
    for i in 0..count {
        let screen: *mut AnyObject = msg_send![screens, objectAtIndex: i];
        if screen.is_null() {
            continue;
        }
        let vf: NSRect = msg_send![screen, visibleFrame];
        if i == 0 {
            primary_visible = vf;
        }
        if rects_intersect(frame, vf) {
            return; // still on a real screen — leave it where the user has it.
        }
    }
    let w = frame.size.width;
    let h = frame.size.height;
    let x = primary_visible.origin.x + (primary_visible.size.width - w) / 2.0;
    let y = primary_visible.origin.y + 60.0;
    let recentered = NSRect {
        origin: NSPoint { x, y },
        size: NSSize {
            width: w,
            height: h,
        },
    };
    let _: () = msg_send![ns_window, setFrame: recentered, display: true, animate: false];
}

/// Cocoa-rect overlap test (y-up; both rects in the same global screen space).
fn rects_intersect(a: NSRect, b: NSRect) -> bool {
    let ax2 = a.origin.x + a.size.width;
    let ay2 = a.origin.y + a.size.height;
    let bx2 = b.origin.x + b.size.width;
    let by2 = b.origin.y + b.size.height;
    a.origin.x < bx2 && b.origin.x < ax2 && a.origin.y < by2 && b.origin.y < ay2
}
