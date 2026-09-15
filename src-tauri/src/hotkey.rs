//! Global hotkey listener for Vibeking.
//!
//! macOS-only. Uses a CGEventTap at `kCGSessionEventTap` to observe:
//!   - flagsChanged for the user's record hotkey (modifier-only mode)
//!   - keyDown for Esc (cancel), record-hotkey combos, and Shift+Tab
//!     to cycle refinement modes while recording
//!
//! Emits Tauri events:
//!   - `recording:start`   (entered recording mode, either PTT or toggle)
//!   - `recording:stop`    (recording ended, transcribe)
//!   - `recording:cancel`  (Esc pressed, discard)
//!   - `mode:changed`      (push-to-talk vs toggle classification)
//!   - `refinement:active-changed` (Shift+Tab during recording cycles the
//!                                  active refinement mode)

#[cfg(target_os = "macos")]
pub use platform::start;

#[cfg(not(target_os = "macos"))]
pub fn start(_app: &tauri::AppHandle) {}

#[cfg(target_os = "macos")]
mod platform {
    use core_foundation::runloop::CFRunLoop;
    use core_graphics::event::{
        CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
        CGEventType, CallbackResult, EventField,
    };
    use std::time::{Duration, Instant};
    use tauri::{AppHandle, Emitter};

    const ESC_KEYCODE: i64 = 53;
    const TAB_KEYCODE: i64 = 48;
    /// Combined left+right shift device flags (NX_DEVICELSHIFTKEYMASK |
    /// NX_DEVICERSHIFTKEYMASK). The CGEventFlags raw bits include these
    /// device-specific bits — see the modifier_lookup table for the matching
    /// pattern used by the user-configurable record hotkey.
    const SHIFT_DEVICE_MASK: u64 = 0x0000_0002 | 0x0000_0004;
    const HOLD_THRESHOLD_MS: u64 = 250;

    fn modifier_lookup(id: &str) -> Option<(i64, u64)> {
        match id {
            "left-control" => Some((59, 0x0000_0001)),
            "right-control" => Some((62, 0x0000_2000)),
            "left-shift" => Some((56, 0x0000_0002)),
            "right-shift" => Some((60, 0x0000_0004)),
            "left-command" => Some((55, 0x0000_0008)),
            "right-command" => Some((54, 0x0000_0010)),
            "left-option" => Some((58, 0x0000_0020)),
            "right-option" => Some((61, 0x0000_0040)),
            _ => None,
        }
    }

    fn key_lookup(id: &str) -> Option<i64> {
        const LETTERS: &[(&str, i64)] = &[
            ("a", 0),
            ("b", 11),
            ("c", 8),
            ("d", 2),
            ("e", 14),
            ("f", 3),
            ("g", 5),
            ("h", 4),
            ("i", 34),
            ("j", 38),
            ("k", 40),
            ("l", 37),
            ("m", 46),
            ("n", 45),
            ("o", 31),
            ("p", 35),
            ("q", 12),
            ("r", 15),
            ("s", 1),
            ("t", 17),
            ("u", 32),
            ("v", 9),
            ("w", 13),
            ("x", 7),
            ("y", 16),
            ("z", 6),
            ("0", 29),
            ("1", 18),
            ("2", 19),
            ("3", 20),
            ("4", 21),
            ("5", 23),
            ("6", 22),
            ("7", 26),
            ("8", 28),
            ("9", 25),
            ("space", 49),
            ("enter", 36),
            ("tab", 48),
            ("backquote", 50),
            ("slash", 44),
            ("minus", 27),
            ("equal", 24),
            ("bracket-left", 33),
            ("bracket-right", 30),
            ("semicolon", 41),
            ("quote", 39),
            ("comma", 43),
            ("period", 47),
            ("backslash", 42),
            ("arrow-up", 126),
            ("arrow-down", 125),
            ("arrow-left", 123),
            ("arrow-right", 124),
            ("f1", 122),
            ("f2", 120),
            ("f3", 99),
            ("f4", 118),
            ("f5", 96),
            ("f6", 97),
            ("f7", 98),
            ("f8", 100),
            ("f9", 101),
            ("f10", 109),
            ("f11", 103),
            ("f12", 111),
        ];
        LETTERS.iter().find(|(s, _)| *s == id).map(|(_, k)| *k)
    }

    /// Parsed hotkey spec.
    /// - `modifier_only=true`: PTT/toggle hold logic. `keycode` is the modifier itself.
    /// - `modifier_only=false`: keyDown combo. `keycode` is the main key.
    #[derive(Debug, Clone)]
    struct HotkeySpec {
        modifier_mask: u64,
        keycode: i64,
        modifier_only: bool,
    }

    fn parse_hotkey(s: &str) -> HotkeySpec {
        let parts: Vec<&str> = s.split('+').collect();
        let mut modifier_mask: u64 = 0;
        let mut last_modifier_keycode: Option<i64> = None;
        let mut key_keycode: Option<i64> = None;

        for p in &parts {
            if let Some((kc, mask)) = modifier_lookup(p) {
                modifier_mask |= mask;
                last_modifier_keycode = Some(kc);
            } else if let Some(kc) = key_lookup(p) {
                key_keycode = Some(kc);
            }
        }

        match (key_keycode, last_modifier_keycode) {
            (Some(kc), _) => HotkeySpec {
                modifier_mask,
                keycode: kc,
                modifier_only: false,
            },
            (None, Some(kc)) => HotkeySpec {
                modifier_mask,
                keycode: kc,
                modifier_only: true,
            },
            _ => HotkeySpec {
                modifier_mask: 0x0000_2000,
                keycode: 62,
                modifier_only: true,
            },
        }
    }

    use crate::state::{RecMode, SharedState};
    use tauri::Manager;

    fn emit(app: &AppHandle, event: &str) {
        let _ = app.emit(event, ());
    }
    fn emit_mode(app: &AppHandle, mode: &str) {
        let _ = app.emit("mode:changed", mode);
    }

    pub fn start(app: &AppHandle) {
        let app = app.clone();
        std::thread::Builder::new()
            .name("vibeking-hotkey".into())
            .spawn(move || run_event_tap(app))
            .ok();
    }

    fn run_event_tap(app: AppHandle) {
        let events = vec![CGEventType::FlagsChanged, CGEventType::KeyDown];

        let callback_app = app.clone();
        let tap = CGEventTap::new(
            CGEventTapLocation::Session,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            events,
            move |_proxy, event_type, event| {
                handle_event(&callback_app, event_type, event);
                CallbackResult::Keep
            },
        );

        let tap = match tap {
            Ok(tap) => tap,
            Err(_) => {
                log::error!(
                    "[vibeking] CGEventTap failed to create. Grant Input Monitoring permission \
                     (System Settings → Privacy & Security → Input Monitoring)."
                );
                let _ = app.emit("permissions:event-tap-failed", ());
                return;
            }
        };

        let current = CFRunLoop::get_current();
        let source = match tap.mach_port().create_runloop_source(0) {
            Ok(src) => src,
            Err(_) => {
                log::info!("[vibeking] could not create runloop source for event tap");
                return;
            }
        };
        current.add_source(&source, unsafe {
            core_foundation::runloop::kCFRunLoopCommonModes
        });
        tap.enable();
        CFRunLoop::run_current();
    }

    fn handle_event(app: &AppHandle, event_type: CGEventType, event: &CGEvent) {
        if capture_active(app) {
            return;
        }
        match event_type {
            CGEventType::FlagsChanged => handle_flags_changed(app, event),
            CGEventType::KeyDown => handle_key_down(app, event),
            _ => {}
        }
    }

    fn shared(app: &AppHandle) -> SharedState {
        app.state::<SharedState>().inner().clone()
    }

    fn capture_active(app: &AppHandle) -> bool {
        shared(app)
            .hotkey_capture_active
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn current_hotkey(app: &AppHandle) -> HotkeySpec {
        let id = shared(app).settings.lock().record_hotkey.clone();
        parse_hotkey(&id)
    }

    fn handle_flags_changed(app: &AppHandle, event: &CGEvent) {
        let spec = current_hotkey(app);
        if !spec.modifier_only {
            return; // combo hotkeys fire on keydown
        }
        let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
        if keycode != spec.keycode {
            return;
        }
        let flags = event.get_flags().bits();
        let pressed = (flags & spec.modifier_mask) != 0;

        // The recording state machine lives in shared state so a UI
        // click-to-record and the hotkey can't diverge. We hold the lock only
        // for the transition decision, then emit outside it.
        let shared = shared(app);
        let mut mode = shared.recording_mode.lock();
        let prev = *mode;
        // Diagnostic: trace every modifier event we act on, plus the
        // resulting transition. Surfaces key-bounce / double-fire bursts
        // that show up as recording:start → recording:stop within ~20 ms
        // (chip bar blinks, audio engine captures effectively nothing,
        // silent-clip guard discards). Format chosen for easy grep:
        //   `[vibeking hotkey] flagsChanged pressed=… prev=… → next=… emit=…`
        match (prev, pressed) {
            (RecMode::Idle, true) => {
                *mode = RecMode::Pressed {
                    since: Instant::now(),
                };
                trace_transition(prev, &mode, pressed, Some("recording:start"), None);
                emit(app, "recording:start");
            }
            (RecMode::Pressed { since }, false) => {
                let elapsed_ms = since.elapsed().as_millis();
                if since.elapsed() < Duration::from_millis(HOLD_THRESHOLD_MS) {
                    // Quick tap → enter Toggle. Recording keeps running
                    // until the next press of the modifier. The mode
                    // change is surfaced via `mode:changed` so the chip
                    // bar can flip its label to "Toggle · tap to stop"
                    // — the previous silent-toggle behaviour confused
                    // users who thought they had stopped recording and
                    // then "started a new one" with a press that was
                    // really stopping the still-running toggle.
                    *mode = RecMode::Toggle;
                    log::info!(
                        "[vibeking hotkey] quick-tap → Toggle (held {} ms < {} ms threshold)",
                        elapsed_ms,
                        HOLD_THRESHOLD_MS,
                    );
                    trace_transition(prev, &mode, pressed, None, Some("toggle"));
                    emit_mode(app, "toggle");
                } else {
                    // Held past the threshold → push-to-talk, release
                    // stops the recording.
                    *mode = RecMode::Idle;
                    log::info!(
                        "[vibeking hotkey] PTT release after {} ms → stop",
                        elapsed_ms,
                    );
                    trace_transition(prev, &mode, pressed, Some("recording:stop"), None);
                    emit(app, "recording:stop");
                }
            }
            (RecMode::Pressed { since }, true) => {
                if since.elapsed() >= Duration::from_millis(HOLD_THRESHOLD_MS) {
                    *mode = RecMode::PushToTalk;
                    trace_transition(prev, &mode, pressed, None, Some("push-to-talk"));
                    emit_mode(app, "push-to-talk");
                } else {
                    log::info!(
                        "[vibeking hotkey] duplicate-press while Pressed (held {} ms) — ignored",
                        since.elapsed().as_millis(),
                    );
                }
            }
            (RecMode::PushToTalk, false) => {
                *mode = RecMode::Idle;
                trace_transition(prev, &mode, pressed, Some("recording:stop"), None);
                emit(app, "recording:stop");
            }
            (RecMode::Toggle, true) => {
                *mode = RecMode::Idle;
                trace_transition(prev, &mode, pressed, Some("recording:stop"), None);
                emit(app, "recording:stop");
            }
            (m, p) => {
                log::info!(
                    "[vibeking hotkey] no-op transition prev={:?} pressed={}",
                    m,
                    p,
                );
            }
        }
    }

    fn trace_transition(
        prev: RecMode,
        next: &RecMode,
        pressed: bool,
        emit: Option<&str>,
        mode_emit: Option<&str>,
    ) {
        log::info!(
            "[vibeking hotkey] flagsChanged pressed={} prev={:?} → next={:?} emit={} mode={}",
            pressed,
            prev,
            next,
            emit.unwrap_or("-"),
            mode_emit.unwrap_or("-"),
        );
    }

    fn handle_key_down(app: &AppHandle, event: &CGEvent) {
        let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
        let flags = event.get_flags().bits();

        // Shift+Tab while recording → cycle refinement mode. Gated on the
        // recording state machine so Shift+Tab outside of a recording is a
        // no-op (preserving the OS's reverse-tab focus shortcut). The event
        // tap is ListenOnly so we cannot suppress the keystroke — the focused
        // app will still see Shift+Tab and may move focus. Acceptable for the
        // prototype; revisit if it proves disruptive in dogfooding.
        if keycode == TAB_KEYCODE && (flags & SHIFT_DEVICE_MASK) != 0 {
            let shared = shared(app);
            let is_recording = !matches!(*shared.recording_mode.lock(), RecMode::Idle);
            if is_recording {
                let payload = crate::state::apply_cycle_refinement_mode(app, &shared);
                log::info!(
                    "[vibeking hotkey] Shift+Tab cycle → {:?}",
                    payload.mode.as_ref().map(|m| m.id.as_str()),
                );
                return;
            }
        }

        // Combo hotkey: keydown + matching modifier mask triggers record toggle.
        let spec = current_hotkey(app);
        if !spec.modifier_only && keycode == spec.keycode {
            let mods_pressed = (flags & spec.modifier_mask) == spec.modifier_mask;
            if mods_pressed {
                let shared = shared(app);
                let mut mode = shared.recording_mode.lock();
                let prev = *mode;
                match *mode {
                    RecMode::Idle => {
                        *mode = RecMode::Toggle;
                        log::info!(
                            "[vibeking hotkey] combo keyDown prev={:?} → Toggle emit=recording:start",
                            prev,
                        );
                        emit(app, "recording:start");
                        emit_mode(app, "toggle");
                    }
                    _ => {
                        *mode = RecMode::Idle;
                        log::info!(
                            "[vibeking hotkey] combo keyDown prev={:?} → Idle emit=recording:stop",
                            prev,
                        );
                        emit(app, "recording:stop");
                    }
                }
                return;
            }
        }

        if keycode != ESC_KEYCODE {
            return;
        }

        let shared = shared(app);
        let mut mode = shared.recording_mode.lock();
        if !matches!(*mode, RecMode::Idle) {
            *mode = RecMode::Idle;
            emit(app, "recording:cancel");
        }
    }
}
