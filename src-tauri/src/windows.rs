//! Chip bar window positioning, sizing, and show/hide.
//!
//! The chip bar lives in one transparent, borderless, status-level window
//! that is centred near the bottom of the screen. It has two footprints:
//!
//!   - **Full** — used while recording / processing / showing a result or a
//!     correction prompt. Big enough for the live-transcript card and the
//!     done-diff card.
//!   - **Idle** — the optional always-on "idle bar" (see `Settings::persistent_bar`).
//!     Shrunk to hug the compact bar so the transparent window doesn't swallow
//!     clicks meant for apps behind it.
//!
//! Both footprints share the same bottom margin + the chip's own `pb-[40px]`
//! anchor, so the bar's bottom edge stays put when the window resizes between
//! them — no visual jump as idle ↔ recording.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{
    Emitter, LogicalPosition, LogicalSize, Manager, Runtime, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

use crate::state::{RecMode, SharedState};

const CHIPBAR_BOTTOM_MARGIN: f64 = 200.0;
pub const CHIPBAR_LABEL: &str = "chipbar";

/// The mode picker lives in its OWN window — not inside the chip bar — so opening
/// it never resizes the chip window (the resize is what made the bar flicker) and
/// its webview is never clipped (a clipped WKWebView ignores `mouseMoved`, which
/// killed hover when we tried freezing the chip webview). See `show_modepicker`.
pub const MODEPICKER_LABEL: &str = "modepicker";
/// Picker window footprint. Tall enough for a full refinement list; the menu
/// surface inside caps + scrolls and is bottom-anchored just above the bar.
const MODEPICKER_SIZE: (f64, f64) = (230.0, 400.0);

/// Full chip footprint (recording / processing / correction cards) for the
/// DEFAULT (no always-on bar) flow, sitting comfortably above the Dock region.
const FULL_SIZE: (f64, f64) = (760.0, 340.0);

/// Shared width for the always-on bar AND the recording chip when the bar is
/// enabled. Keeping them the same width means starting a recording only grows
/// the window's *height* — no width jump — so the idle bar morphs in place into
/// the recording chip instead of one vanishing and the other appearing.
const IDLE_WIDTH: f64 = 560.0;
/// Compact idle-bar footprint. Short, resting near the bottom of the screen.
const IDLE_SIZE: (f64, f64) = (IDLE_WIDTH, 72.0);
/// Taller idle footprint used only while the mode-picker popover is open, so
/// the upward-opening menu isn't clipped by the short window. The bar stays
/// anchored to the bottom (see `position`), so it doesn't shift — the window
/// just grows upward to reveal the menu. Tall enough for a full refinement
/// list; the picker itself caps + scrolls beyond that (see ModePicker).
const IDLE_TALL_SIZE: (f64, f64) = (IDLE_WIDTH, 440.0);
/// Tall footprint when the always-on bar is enabled — same width, grown
/// upward to fit the live-transcript + result cards. The frontend asks for this
/// (via `chipbar_grow`) only when the content actually needs the height, so the
/// idle ↔ recording-compact morph happens with NO window resize at all.
const FULL_SIZE_PERSISTENT: (f64, f64) = (IDLE_WIDTH, 400.0);
/// Bottom margin for every persistent-bar window size. Because all chip
/// containers share the same `pb-[18px]`, a single margin keeps the bar's
/// bottom edge pinned at the same screen spot (`36 + 18` = 54px up) no matter
/// the window height — so growing/shrinking only moves the TOP edge. (Coupled
/// to the `pb-[18px]` in ChipBar.tsx — move them together.)
const IDLE_BOTTOM_MARGIN: f64 = 36.0;

pub(crate) fn persistent_bar_enabled<R: Runtime>(app: &tauri::AppHandle<R>) -> bool {
    app.state::<SharedState>().settings.lock().persistent_bar
}

fn is_recording_idle<R: Runtime>(app: &tauri::AppHandle<R>) -> bool {
    matches!(
        *app.state::<SharedState>().recording_mode.lock(),
        RecMode::Idle
    )
}

/// Show the chip bar at its **full** footprint — the recording / processing /
/// correction surface. Called on `recording:start` and by the correction watcher.
pub fn show_chipbar<R: Runtime>(app: &tauri::AppHandle<R>) {
    let Some(window) = app.get_webview_window(CHIPBAR_LABEL) else {
        return;
    };
    // With the always-on bar enabled, the recording chip stays the idle bar's
    // exact window (compact, same position) so it morphs in place — the HTML
    // bar element animates its width/background, no window resize. The window
    // only grows later, via `chipbar_grow`, when the live card / result card
    // needs the height.
    if persistent_bar_enabled(app) {
        apply(&window, IDLE_SIZE, IDLE_BOTTOM_MARGIN);
    } else {
        apply(&window, FULL_SIZE, CHIPBAR_BOTTOM_MARGIN);
    }
}

/// Grow / shrink the persistent chip window to fit the current content. The
/// frontend calls this when the chip needs the tall footprint (live transcript,
/// result/error card) or can return to compact (idle bar, recording with no
/// live text yet). Anchored at the same bottom margin, so only the top edge
/// moves. No-op without the always-on bar (that flow manages its own sizing).
#[tauri::command]
pub async fn chipbar_grow<R: Runtime>(app: tauri::AppHandle<R>, tall: bool) -> Result<(), String> {
    if !persistent_bar_enabled(&app) {
        return Ok(());
    }
    let Some(window) = app.get_webview_window(CHIPBAR_LABEL) else {
        return Ok(());
    };
    let size = if tall {
        FULL_SIZE_PERSISTENT
    } else {
        IDLE_SIZE
    };
    log::info!("[vibeking chipbar] grow tall={tall} -> size={size:?}");
    apply(&window, size, IDLE_BOTTOM_MARGIN);
    Ok(())
}

/// Show the chip bar at its compact **idle** footprint — the always-on bar the
/// user can click to start dictation, resting near the bottom of the screen.
pub fn show_idle_bar<R: Runtime>(app: &tauri::AppHandle<R>) {
    let Some(window) = app.get_webview_window(CHIPBAR_LABEL) else {
        return;
    };
    apply(&window, IDLE_SIZE, IDLE_BOTTOM_MARGIN);
}

/// Grow / shrink the idle window while the mode-picker popover is open. The bar
/// stays put (it's anchored to the window bottom); only the window's top edge
/// moves, revealing space above the bar for the upward-opening menu.
#[tauri::command]
pub async fn chipbar_idle_tall<R: Runtime>(
    app: tauri::AppHandle<R>,
    tall: bool,
) -> Result<(), String> {
    // Only meaningful for the idle bar — ignore if a recording is in flight.
    if !is_recording_idle(&app) || !persistent_bar_enabled(&app) {
        return Ok(());
    }
    let Some(window) = app.get_webview_window(CHIPBAR_LABEL) else {
        return Ok(());
    };
    let size = if tall { IDLE_TALL_SIZE } else { IDLE_SIZE };
    apply(&window, size, IDLE_BOTTOM_MARGIN);
    Ok(())
}

/// Hide the chip bar — UNLESS the persistent idle bar is enabled, in which case
/// "hiding" means falling back to the idle bar instead of vanishing. Emitting
/// `chipbar:reset` lets the frontend drop whatever result/error phase it was
/// showing and re-render the idle bar. Every recording-flow hide path routes
/// through here, so the idle-bar fallback is automatic.
pub fn hide_chipbar<R: Runtime>(app: &tauri::AppHandle<R>) {
    if persistent_bar_enabled(app) {
        show_idle_bar(app);
        let _ = app.emit("chipbar:reset", ());
        return;
    }
    if let Some(window) = app.get_webview_window(CHIPBAR_LABEL) {
        let _ = window.hide();
    }
}

/// Recover a display-stranded chip bar by rebuilding the window — the
/// community-recommended fix, done correctly.
///
/// After a display reconfiguration macOS strands the window `onscreen=false` and NO
/// in-process re-composition recovers it (re-assert `canJoinAllSpaces`, re-order,
/// frame-nudge, `moveToActiveSpace` — all fail). A fresh window composites cleanly,
/// so we destroy and rebuild. The crash we hit before was doing this on a fixed
/// timer, which raced the async teardown → "label already exists" → uncatchable
/// Obj-C abort. The correct pattern (per tauri#3688 / #9353) is to WAIT until the
/// label is actually freed before rebuilding — so here we poll `get_webview_window`
/// until it returns `None`, and only then build. We build ONLY when the label is
/// free, so the crashing conflict path can never happen.
///
/// Guaranteed recovery: if the label never frees, the rebuild fails, or the
/// rebuilt window STILL can't composite (i.e. the stranding is a per-process
/// window-server state that a same-process window inherits), we fall back to
/// `app.restart()` — a fresh process always works.
pub fn recreate_chipbar<R: Runtime>(app: &tauri::AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // 1. Destroy the stranded window (main thread).
        let app_d = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(old) = app_d.get_webview_window(CHIPBAR_LABEL) {
                let _ = old.destroy();
            }
        });

        // 2. Wait until Tauri has actually released the label. destroy() removes it
        //    asynchronously; building the same label before it's gone is what threw
        //    the uncatchable exception. Only proceed once it's None.
        let mut freed = false;
        for _ in 0..60 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if app.get_webview_window(CHIPBAR_LABEL).is_none() {
                freed = true;
                break;
            }
        }
        if !freed {
            log::warn!("[vibeking chipbar] recreate: label not released in 3s — restarting app");
            app.restart();
        }

        // 3. Build a fresh window (main thread). The label is free, so no conflict.
        let app_b = app.clone();
        let built = Arc::new(AtomicBool::new(false));
        let built_flag = built.clone();
        let _ = app.run_on_main_thread(move || {
            match WebviewWindowBuilder::new(
                &app_b,
                CHIPBAR_LABEL,
                WebviewUrl::App("index.html?window=chipbar".into()),
            )
            .title("Vibeking Chip Bar")
            .inner_size(FULL_SIZE.0, FULL_SIZE.1)
            .position(0.0, 0.0)
            .resizable(false)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .focused(false)
            .visible(false)
            .visible_on_all_workspaces(true)
            .accept_first_mouse(true)
            .build()
            {
                Ok(window) => {
                    crate::macos_window::configure_chipbar(&window);
                    built_flag.store(true, Ordering::Relaxed);
                }
                Err(e) => log::error!("[vibeking chipbar] recreate build failed: {e}"),
            }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        if !built.load(Ordering::Relaxed) {
            log::warn!("[vibeking chipbar] recreate: rebuild failed — restarting app");
            app.restart();
        }
        log::info!("[vibeking chipbar] recreated chip window (fresh NSWindow)");

        // 4. Restore the always-on bar and VERIFY it actually composited. If a
        //    same-process fresh window still can't (the stranding is per-process
        //    window-server state), restart — the guaranteed recovery.
        if persistent_bar_enabled(&app) {
            show_idle_bar(&app);
            let _ = app.emit("chipbar:reset", ());
            tokio::time::sleep(Duration::from_millis(800)).await;
            if let Some(window) = app.get_webview_window(CHIPBAR_LABEL) {
                if !crate::macos_window::chipbar_onscreen(&window) {
                    log::warn!(
                        "[vibeking chipbar] recreated window STILL stranded — restarting app"
                    );
                    app.restart();
                }
                log::info!("[vibeking chipbar] recreation recovered the bar (on screen)");
            }
        }
    });
}

/// Bring the window in line with the current `persistent_bar` setting. Called
/// from `set_settings` on every settings change (including the initial push
/// from the frontend at launch). No-ops while a recording is in flight so a
/// stray settings write can't resize the window out from under an active card.
pub fn reconcile_persistent_bar<R: Runtime>(app: &tauri::AppHandle<R>, _state: &SharedState) {
    if !is_recording_idle(app) {
        return;
    }
    if persistent_bar_enabled(app) {
        show_idle_bar(app);
        // Clear any stale phase (e.g. a `done`/`error`/`polishing` left over
        // from a recording that finished while the bar was OFF) so turning the
        // bar on always reveals the idle bar, not a frozen previous result.
        let _ = app.emit("chipbar:reset", ());
    } else if let Some(window) = app.get_webview_window(CHIPBAR_LABEL) {
        let _ = window.hide();
    }
}

/// Show the mode picker as a popover anchored just above the chip bar's top-right
/// (where the inline menu used to drop from). Focuses it so an outside click
/// blurs it → auto-hide (wired in `lib.rs`). No-op without the always-on bar.
#[tauri::command]
pub async fn show_modepicker<R: Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    if !persistent_bar_enabled(&app) || !is_recording_idle(&app) {
        return Ok(());
    }
    let Some(picker) = app.get_webview_window(MODEPICKER_LABEL) else {
        return Ok(());
    };
    let Some(chip) = app.get_webview_window(CHIPBAR_LABEL) else {
        return Ok(());
    };
    #[cfg(target_os = "macos")]
    {
        let p = picker.clone();
        let _ = picker.run_on_main_thread(move || {
            position_modepicker(&p, &chip);
            let _ = p.show();
            let _ = p.set_focus();
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = chip;
        let _ = picker.show();
        let _ = picker.set_focus();
    }
    Ok(())
}

/// Hide the mode picker and tell the chip bar it closed (so the chip's open-state
/// — chevron rotation, click-to-toggle — stays in sync).
#[tauri::command]
pub fn hide_modepicker<R: Runtime>(app: tauri::AppHandle<R>) {
    if let Some(picker) = app.get_webview_window(MODEPICKER_LABEL) {
        let _ = picker.hide();
        let _ = app.emit("modepicker:closed", ());
    }
}

/// Place the picker just above the bar's top edge, right-aligned to the bar (the
/// inline menu used `bottom-full right-1.5`). Coordinates are native Cocoa points
/// (y-up, origin bottom-left); the chip window's bottom sits `IDLE_BOTTOM_MARGIN`
/// above the screen bottom with the bar `pb-[18px]` + ~44px tall inside it.
#[cfg(target_os = "macos")]
fn position_modepicker<R: Runtime>(picker: &WebviewWindow<R>, chip: &WebviewWindow<R>) {
    use objc2::msg_send;
    use objc2_app_kit::NSWindow;
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let (Ok(pp), Ok(cp)) = (picker.ns_window(), chip.ns_window()) else {
        return;
    };
    let picker_ns = pp as *mut NSWindow;
    let chip_ns = cp as *mut NSWindow;
    if picker_ns.is_null() || chip_ns.is_null() {
        return;
    }
    unsafe {
        let chip_frame: NSRect = msg_send![chip_ns, frame];
        let (w, h) = MODEPICKER_SIZE;
        // Right edge ~16px in from the bar's right edge; bottom ~8px above the
        // bar's top (18px pb + 44px bar + 8px gap = 70 above the window bottom).
        let x = chip_frame.origin.x + chip_frame.size.width - 16.0 - w;
        let y = chip_frame.origin.y + 70.0;
        let frame = NSRect {
            origin: NSPoint { x, y },
            size: NSSize {
                width: w,
                height: h,
            },
        };
        let _: () = msg_send![picker_ns, setFrame: frame, display: true, animate: false];
    }
}

/// Resize → reposition → show. When the window is already visible (idle → record
/// and back), this must be ATOMIC: Tauri's separate `set_size` + `set_position`
/// land on the window server in an unpredictable order, so for a frame the
/// window is the new size at the old origin (or vice-versa) — which the user
/// saw as the recording bar flickering ~300px above before snapping into place.
/// macOS `setFrame:` sets origin + size in one call, killing the flicker.
///
/// AppKit (NSWindow/NSScreen) is main-thread-only; this is called from event
/// handlers that may run off the main thread (calling it inline there crashes),
/// so the native work is dispatched onto the main thread. The Tauri path is the
/// fallback (non-macOS, or a window with no screen yet — i.e. first show).
fn apply<R: Runtime>(window: &WebviewWindow<R>, size: (f64, f64), bottom_margin: f64) {
    #[cfg(target_os = "macos")]
    {
        let win = window.clone();
        let dispatched = window
            .run_on_main_thread(move || {
                if !set_frame_atomic(&win, size, bottom_margin) {
                    // No screen (hidden window) → Tauri sizing, then show.
                    let _ = win.set_size(LogicalSize {
                        width: size.0,
                        height: size.1,
                    });
                    let _ = position(&win, size, bottom_margin);
                }
                let _ = win.show();
            })
            .is_ok();
        if dispatched {
            return;
        }
    }
    let _ = window.set_size(LogicalSize {
        width: size.0,
        height: size.1,
    });
    let _ = position(window, size, bottom_margin);
    let _ = window.show();
}

/// How long to keep the window hidden (alpha 0) after a size-changing setFrame,
/// giving the out-of-process WKWebView time to repaint at the new bounds before
/// we fade it back in. The webview re-renders within ~1-2 frames; 55 ms is a
/// comfortable margin without a perceptible blank. Tunable.
#[cfg(target_os = "macos")]
const RESIZE_REVEAL_DELAY_MS: u64 = 55;

/// Atomic origin+size set via the native NSWindow, anchored so the window's
/// BOTTOM edge stays `bottom_margin` above the screen bottom — the window grows
/// upward as height increases, keeping the bar pinned. Returns false (→ caller
/// falls back to the Tauri path) if the window has no screen yet (e.g. hidden).
/// MUST be called on the main thread (see `apply`).
///
/// On a SIZE CHANGE (e.g. the compact-pill → live-transcript grow, 72→400px) the
/// window is hidden (alpha 0) across the setFrame and faded back in after the
/// webview has repainted. WKWebView composites its content in an out-of-process
/// layer that, on an upward resize, momentarily strands the stale frame at the
/// TOP of the new bounds — the bar appeared to leap up hundreds of px then drop
/// back. Hiding for the one frame it takes to repaint means that stale frame is
/// never composited at the wrong gravity. A same-size reposition skips the mask
/// entirely (no needless blink), so the frequent same-footprint applies are
/// untouched.
#[cfg(target_os = "macos")]
fn set_frame_atomic<R: Runtime>(
    window: &WebviewWindow<R>,
    size: (f64, f64),
    bottom_margin: f64,
) -> bool {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSScreen, NSWindow};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let Ok(ptr) = window.ns_window() else {
        return false;
    };
    let ns_window = ptr as *mut NSWindow;
    if ns_window.is_null() {
        return false;
    }
    unsafe {
        // Re-assert the floating config on every show/resize — macOS strips the
        // status level + join-all-Spaces flags across Space/display changes, and
        // this is the cheapest place to restore them before the window is drawn.
        crate::macos_window::reassert_floating(ns_window);
        let screen: *mut NSScreen = msg_send![ns_window, screen];
        if screen.is_null() {
            return false;
        }
        // NSScreen.frame is in global Cocoa points (y-up, origin at the screen's
        // bottom-left). Sizes here are logical points too, so no scaling needed.
        let screen_frame: NSRect = msg_send![screen, frame];
        let (w, h) = size;
        let x = screen_frame.origin.x + (screen_frame.size.width - w) / 2.0;
        let y = screen_frame.origin.y + bottom_margin;
        let frame = NSRect {
            origin: NSPoint { x, y },
            size: NSSize {
                width: w,
                height: h,
            },
        };

        // Is this an actual resize, or just a reposition at the current size?
        let current: NSRect = msg_send![ns_window, frame];
        let resizing =
            (current.size.width - w).abs() > 0.5 || (current.size.height - h).abs() > 0.5;

        if resizing {
            // Hide across the resize so the webview's stale frame can't flash at
            // the wrong gravity, then fade it back in once it has repainted.
            let _: () = msg_send![ns_window, setAlphaValue: 0.0_f64];
            let _: () = msg_send![ns_window, setFrame: frame, display: false, animate: false];

            let win = window.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(RESIZE_REVEAL_DELAY_MS)).await;
                let win_inner = win.clone();
                let _ = win.run_on_main_thread(move || {
                    let Ok(ptr) = win_inner.ns_window() else {
                        return;
                    };
                    let nsw = ptr as *mut NSWindow;
                    if nsw.is_null() {
                        return;
                    }
                    // `animator()` eases alpha 0→1 (default ~0.25s) so the
                    // newly-sized bar fades in instead of popping.
                    let animator: *mut AnyObject = msg_send![nsw, animator];
                    let _: () = msg_send![animator, setAlphaValue: 1.0_f64];
                });
            });
        } else {
            // Same footprint — make sure we're visible (a prior resize may have
            // left alpha mid-fade) and set the frame normally.
            let _: () = msg_send![ns_window, setAlphaValue: 1.0_f64];
            let _: () = msg_send![ns_window, setFrame: frame, display: true, animate: false];
        }
    }
    true
}

#[tauri::command]
pub async fn chipbar_show<R: Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    show_chipbar(&app);
    Ok(())
}

#[tauri::command]
pub async fn chipbar_hide<R: Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    hide_chipbar(&app);
    Ok(())
}

/// Bring the main vibeking window to the front. Driven from the idle bar's
/// open-app button. Done in Rust (not via the JS window API) because the
/// chipbar webview's capability scope doesn't grant it control over the main
/// window — and Rust can also activate the app, which the non-activating
/// chip-bar panel can't do on its own.
#[tauri::command]
pub fn focus_main_window<R: Runtime>(app: tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn position<R: Runtime>(
    window: &WebviewWindow<R>,
    size: (f64, f64),
    bottom_margin: f64,
) -> tauri::Result<()> {
    let Some(monitor) = window.current_monitor()? else {
        return Ok(());
    };
    let scale = monitor.scale_factor();
    let monitor_size = monitor.size().to_logical::<f64>(scale);
    let monitor_position = monitor.position().to_logical::<f64>(scale);
    let (window_width, window_height) = size;

    let x = monitor_position.x + (monitor_size.width - window_width) / 2.0;
    let y = monitor_position.y + monitor_size.height - bottom_margin - window_height;

    window.set_position(LogicalPosition { x, y })?;
    Ok(())
}
