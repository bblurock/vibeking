mod audio;
mod input_device_cache;
mod audio_io;
pub mod corrections;
mod gemma_server;
mod hotkey;
mod insert;
mod local_parakeet;
mod local_qwen3;
mod local_stt;
mod model_cmds;
mod permissions;
mod polish;
pub mod proper_nouns;
mod state;
mod stt;
mod tray;
mod warm;
mod windows;

#[cfg(target_os = "macos")]
mod correction_watcher;
#[cfg(target_os = "macos")]
mod macos_window;
#[cfg(target_os = "macos")]
mod screen_context;
#[cfg(target_os = "macos")]
mod screen_ocr;

use serde::Serialize;
use std::sync::Arc;
use tauri::{Emitter, Listener, Manager};

#[derive(Debug, Clone, Serialize)]
struct TranscriptEvent {
    raw: String,
    text: String,
    provider: String,
    model: String,
    duration_ms: u64,
    translated: bool,
    polished: bool,
    /// Recording-session identifier (matches state.chipbar_generation at
    /// the time of transcribe spawn). The frontend filters transcript
    /// events whose session is older than its currently-tracked one so
    /// late-arriving events from a previous recording can't overwrite
    /// the chip-bar state of the new one.
    session: u64,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // First in the chain so later plugin/setup failures are captured. Logs
        // to stdout + a rotating file in the OS log dir
        // (~/Library/Logs/com.vibeking.app/vibeking.log on macOS). Our crate at
        // Debug so screen_ocr/screen_context debug lines are kept; deps at Info.
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .level_for("vibeking_lib", log::LevelFilter::Debug)
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("vibeking".into()),
                    }),
                ])
                .max_file_size(5_000_000) // ~5 MB per file
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepAll)
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_sql::Builder::default().build())
        .plugin(tauri_plugin_os::init())
        .manage(Arc::new(state::AppState::default()))
        .invoke_handler(tauri::generate_handler![
            windows::chipbar_show,
            windows::chipbar_hide,
            windows::chipbar_idle_tall,
            windows::chipbar_grow,
            windows::show_modepicker,
            windows::hide_modepicker,
            windows::focus_main_window,
            state::get_settings,
            state::set_settings,
            state::cycle_refinement_mode,
            state::set_active_refinement_mode,
            state::set_hotkey_capture,
            state::ui_toggle_recording,
            model_cmds::model_status,
            model_cmds::download_model,
            model_cmds::delete_model,
            polish::list_polish_models,
            warm::current_warming,
            audio::input_devices,
            audio::start_mic_monitor,
            audio::stop_mic_monitor,
            permissions::check_permissions,
            permissions::open_settings_pane,
        ])
        .setup(|app| {
            // Identify every log file: version + OS + arch. This is what tells us
            // whether a report came from an Intel Mac, Apple Silicon, or Windows.
            let pkg = app.package_info();
            log::info!(
                "[vibeking] v{} starting — os: {} {}, arch: {}",
                pkg.version,
                std::env::consts::OS,
                tauri_plugin_os::version(),
                std::env::consts::ARCH
            );

            // Persist panics (with backtrace) before the process dies, chaining the
            // default hook so terminal users still see them on stderr.
            let prev_hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                log::error!(
                    "[vibeking] PANIC: {info}\n{}",
                    std::backtrace::Backtrace::force_capture()
                );
                prev_hook(info);
            }));

            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("chipbar") {
                macos_window::configure_chipbar(&window);
                // Re-assert the floating config when macOS drops it on Space
                // switches, display reconfiguration, and wake-from-sleep — the
                // root cause of the always-on bar vanishing until a relaunch.
                macos_window::install_chipbar_watchdog(app.handle());
            }

            // Auto-hide the mode picker when it loses focus (outside click).
            if let Some(picker) = app.get_webview_window(windows::MODEPICKER_LABEL) {
                let h = app.handle().clone();
                picker.on_window_event(move |event| {
                    if let tauri::WindowEvent::Focused(false) = event {
                        if let Some(p) = h.get_webview_window(windows::MODEPICKER_LABEL) {
                            if p.is_visible().unwrap_or(false) {
                                let _ = p.hide();
                                let _ = h.emit("modepicker:closed", ());
                            }
                        }
                    }
                });
            }

            let handle = app.handle().clone();

            // Warm the persisted local STT engine at launch — the earliest point —
            // so the FIRST dictation after a fresh start is hot, not cold (a 4-6 GB
            // model load + sidecar spawn + first-inference compile). Rust boots with
            // Settings::default() (cloud Deepgram → no warm), and the user's real
            // selection only arrives later when the chipbar webview mounts and pushes
            // it via set_settings — too late to beat a quick first recording. So read
            // the persisted provider/model straight from the tauri-plugin-store file
            // the frontend writes (key "settings" in "vibeking.settings.json", see
            // src/lib/persist.ts), seed them into state, and kick off the warm now.
            // The later set_settings push then sees an unchanged provider and won't
            // double-warm; warm_current_model is idempotent regardless. The `provider`
            // / `model` keys share the same name + value shape across the JS and Rust
            // settings, so reading just those two from the JS-shaped blob is safe.
            {
                use tauri_plugin_store::StoreExt;
                if let Ok(store) = handle.store("vibeking.settings.json") {
                    if let Some(blob) = store.get("settings") {
                        let provider = blob
                            .get("provider")
                            .cloned()
                            .and_then(|p| serde_json::from_value::<stt::Provider>(p).ok());
                        if let Some(provider) = provider {
                            let model = blob
                                .get("model")
                                .and_then(|m| m.as_str())
                                .map(str::to_string);
                            // Also seed the Gemma streaming flag (camelCase in the
                            // JS-written store) so the boot warm spawns the CORRECT
                            // sidecar mode. Streaming is now the default, so a store
                            // that predates the flag (key absent) warms streaming
                            // too — without this the first recording would have to
                            // kill + reload the stateless server (a multi-second
                            // model reload seen as a delayed live preview).
                            let gemma_streaming = blob
                                .get("gemmaStreaming")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(true);
                            // Seed the Gemma memory policy from the store too, so the
                            // idle watcher honors the user's choice from the very first
                            // recording (before the frontend pushes settings back).
                            let gemma_keep_loaded = blob
                                .get("gemmaKeepLoaded")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            let gemma_idle_timeout_min = blob
                                .get("gemmaIdleTimeoutMin")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(15);
                            let state = handle.state::<state::SharedState>().inner().clone();
                            {
                                let mut s = state.settings.lock();
                                s.provider = provider;
                                if let Some(m) = model {
                                    s.model = Some(m);
                                }
                                s.gemma_streaming = gemma_streaming;
                                s.gemma_keep_loaded = gemma_keep_loaded;
                                s.gemma_idle_timeout_min = gemma_idle_timeout_min;
                            }
                            gemma_server::set_idle_policy(if gemma_keep_loaded {
                                0
                            } else {
                                gemma_idle_timeout_min.saturating_mul(60)
                            });
                            warm::warm_current_model(&handle);
                        }
                    }
                }
            }

            if let Err(e) = tray::setup(&handle) {
                log::error!("[vibeking] tray setup failed: {e}");
            }

            // Keep the tray labels + language filter in sync when the
            // frontend mutates settings. `maybe_rebuild` early-exits when
            // neither the UI language nor the provider changed, so this
            // is cheap for the bulk of mutations (hotwords, dictionary,
            // etc.).
            let tray_handle = handle.clone();
            app.listen_any("settings:changed", move |_| {
                tray::maybe_rebuild(&tray_handle);
            });

            // recording:start → begin mic capture + show chip bar + kick off
            // screen-context capture for proper-noun biasing.
            let start_handle = handle.clone();
            app.listen_any("recording:start", move |_| {
                let h = start_handle.clone();
                let state = h.state::<state::SharedState>().inner().clone();
                let mic_device = {
                    let s = state.settings.lock();
                    let trimmed = s.mic_device.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                };
                // Bump chipbar generation so any still-pending hide_chipbar
                // from a previous recording's 1.5 s "result hold" timer
                // is invalidated and won't hide our new chip bar.
                let gen = state
                    .chipbar_generation
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1;
                log::info!(
                    "[vibeking listener] recording:start  gen={gen} mic={:?}",
                    mic_device,
                );

                // Idle cold-start mitigation: when Gemma streaming is the engine and
                // the sidecar was idle-unloaded, kick off the model reload NOW (the
                // instant the user pressed record) instead of waiting for the live-
                // preview task's first await — so the streaming session has the best
                // chance of being ready in time to show live preview. Best-effort;
                // recover_gemma_streaming() at stop is the correctness guarantee if
                // it's still not ready. Gated on !is_running() so it never touches the
                // warm path; ensure_streaming's spawn gate dedupes with the preview task.
                let (provider, gemma_streaming, gemma_model) = {
                    let s = state.settings.lock();
                    (
                        s.provider,
                        s.gemma_streaming,
                        s.model
                            .clone()
                            .filter(|m| !m.is_empty())
                            .unwrap_or_else(|| gemma_server::DEFAULT_MODEL.to_string()),
                    )
                };
                let gemma_cold = provider == stt::Provider::Gemma
                    && gemma_server::engine_ready()
                    && !gemma_server::is_running();
                // Instrumentation point #1 — one line that makes "first record
                // after long idle, engine cold" a fact. `grep "gen={gen}"` then
                // tells the whole story of this recording.
                log::info!(
                    "[vibeking record] gen={gen} provider={provider:?} gemma_streaming={gemma_streaming} \
                     gemma_running={} idle_secs={:?} cold={gemma_cold}",
                    gemma_server::is_running(),
                    gemma_server::idle_secs(),
                );
                if gemma_streaming && gemma_cold {
                    log::info!(
                        "[vibeking prewarm] gen={gen} gemma cold — preloading streaming sidecar"
                    );
                    let model = gemma_model.clone();
                    let warm_h = h.clone();
                    // Surface a "Warming…" badge on the recording chip so the user
                    // understands the ~5s cold-load (no live preview yet) — the
                    // stop-time recovery still guarantees the transcript. Emit
                    // model:warmed on completion (success OR failure) purely to
                    // clear the badge; a mid-recording failure must not flash an
                    // error HUD, so we don't emit model:warm-failed here.
                    let _ = warm_h.emit("model:warming", serde_json::json!({ "provider": "gemma" }));
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = gemma_server::ensure_streaming(&model).await {
                            log::error!("[vibeking prewarm] gemma ensure_streaming: {e}");
                        }
                        let _ =
                            warm_h.emit("model:warmed", serde_json::json!({ "provider": "gemma" }));
                    });
                }

                match state.audio_engine.start_recording(mic_device) {
                    Ok(resolution) => {
                        windows::show_chipbar(&h);
                        // Always log the device cpal actually opened — the single
                        // most useful line when "no audio captured" (tells a wrong/
                        // virtual default device apart from a real-but-silent one).
                        log::info!(
                            "[vibeking] mic resolved: requested={:?} actual={:?} fell_back={}",
                            resolution.requested, resolution.actual, resolution.fell_back,
                        );
                        // Surface the resolved mic so the chip bar can render a
                        // "(AirPods unavailable)" hint when the saved device wasn't
                        // found and we fell back. Emitted after show_chipbar so the
                        // listener is already mounted.
                        let _ = h.emit("recording:mic-resolved", &resolution);

                        // Liveness watcher: while this recording is active, watch the
                        // mic's audio heartbeat (`audible_seq`). The audio thread
                        // self-heals a dead warm stream by rebuilding it; if even that
                        // can't restore audio (muted mic, wrong device, dead BT link),
                        // surface a "check your mic" hint so the user isn't speaking
                        // into the void. The liveness signal is the audio thread's
                        // `live_seq` heartbeat, which ticks on the mic's ambient noise
                        // floor — so a normal thinking pause keeps it alive and only a
                        // genuinely dead/muted/permission-denied stream stalls it. We
                        // never warn just because the user went quiet. The 3 s warn
                        // threshold is longer than the audio thread's 2.5 s rebuild
                        // window so self-heal gets first crack before we bother the user.
                        let health_state = state.clone();
                        let health_h = h.clone();
                        let session = gen;
                        tauri::async_runtime::spawn(async move {
                            use std::sync::atomic::Ordering::Relaxed;
                            use std::time::{Duration, Instant};
                            const POLL: Duration = Duration::from_millis(400);
                            const SILENCE_WARN: Duration = Duration::from_secs(3);
                            let mut last_seq = health_state.audio_engine.live_seq();
                            let mut last_live_at = Instant::now();
                            let mut warned = false;
                            loop {
                                tokio::time::sleep(POLL).await;
                                if !health_state.audio_engine.is_recording()
                                    || health_state.chipbar_generation.load(Relaxed) != session
                                {
                                    break;
                                }
                                let seq = health_state.audio_engine.live_seq();
                                if seq != last_seq {
                                    last_seq = seq;
                                    last_live_at = Instant::now();
                                    if warned {
                                        warned = false;
                                        let _ = health_h.emit(
                                            "recording:audio-health",
                                            serde_json::json!({ "live": true, "session": session }),
                                        );
                                    }
                                } else if !warned && last_live_at.elapsed() > SILENCE_WARN {
                                    warned = true;
                                    // The stream is genuinely delivering no audio. Check
                                    // the actual mic permission so the UI can say WHY —
                                    // a denied permission is the most common real cause
                                    // and is directly actionable (open Settings).
                                    let mic_permission =
                                        permissions::check_permissions().microphone;
                                    log::info!(
                                        "[vibeking] dead mic stream {}ms into recording (mic_permission={mic_permission}) — warning user",
                                        last_live_at.elapsed().as_millis()
                                    );
                                    let _ = health_h.emit(
                                        "recording:audio-health",
                                        serde_json::json!({
                                            "live": false,
                                            "micPermission": mic_permission,
                                            "session": session,
                                        }),
                                    );
                                }
                            }
                        });
                    }
                    Err(e) => {
                        log::error!("[vibeking] failed to start audio: {e}");
                        let _ = h.emit("recording:error", format!("audio: {e}"));
                    }
                }

                // Live dictation preview (all providers). Periodically
                // re-transcribe the audio captured so far with the active
                // engine and emit transcript:interim, so the chip bar shows
                // what the user has said in real time. Parakeet is ~free
                // (RTF 0.02); cloud / Gemma calls self-throttle because the
                // loop awaits each request before sleeping again. Preview
                // only — the authoritative transcript is the batch pass on
                // stop, so flicker/imperfection here never reaches the user.
                {
                    let settings_snap = state.settings.lock().clone();
                    let preview_state = state.clone();
                    let preview_h = h.clone();
                    let session = gen;
                    tauri::async_runtime::spawn(async move {
                        use std::sync::atomic::Ordering::Relaxed;

                        // Qwen3: native incremental streaming (processes only the
                        // NEW audio each step) — far faster + flicker-free vs
                        // re-transcribing the whole buffer. Everything else falls
                        // through to the universal sliding-window below.
                        if settings_snap.provider == stt::Provider::Qwen3
                            && local_qwen3::engine_ready()
                        {
                            let lang = settings_snap.stt_language.clone();
                            let hint = if lang == "auto" { None } else { Some(lang) };
                            // On any early bail-out, publish an empty stream-final
                            // for this session so the stop handler's
                            // await_qwen3_stream_final resolves immediately (→ batch,
                            // → Whisper fallback) instead of polling the full 15 s
                            // MAX_WAIT for a final that will never be written.
                            if let Err(e) = local_qwen3::streaming_start(hint.as_deref()).await {
                                log::error!("[vibeking preview] qwen3 streaming_start: {e}");
                                *preview_state.qwen3_stream_final.lock() = Some((session, None));
                                return;
                            }
                            let (rate, channels) = preview_state.audio_engine.capture_meta();
                            if rate == 0 {
                                let _ = local_qwen3::streaming_finish();
                                *preview_state.qwen3_stream_final.lock() = Some((session, None));
                                return;
                            }
                            let mut cursor = 0usize;
                            // Qwen3's streaming partial is the CUMULATIVE current
                            // transcript (each feed re-emits the full hypothesis,
                            // lightly re-punctuated), so we emit it as-is. `last`
                            // dedups identical partials.
                            let mut last = String::new();
                            loop {
                                if !preview_state.audio_engine.is_recording()
                                    || preview_state.chipbar_generation.load(Relaxed) != session
                                {
                                    break;
                                }
                                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                                let (chunk, next) =
                                    preview_state.audio_engine.read_samples_from(cursor);
                                cursor = next;
                                if chunk.is_empty() {
                                    continue;
                                }
                                let mono: Vec<f32> = if channels <= 1 {
                                    chunk
                                } else {
                                    let ch = channels as usize;
                                    chunk
                                        .chunks(ch)
                                        .map(|f| f.iter().sum::<f32>() / ch as f32)
                                        .collect()
                                };
                                let mono16k = audio_io::resample_to_16k(&mono, rate);
                                match local_qwen3::streaming_feed(&mono16k) {
                                    Ok(Some(p)) => {
                                        if p != last
                                            && preview_state.chipbar_generation.load(Relaxed)
                                                == session
                                        {
                                            last = p.clone();
                                            let _ = preview_h.emit(
                                                "transcript:interim",
                                                serde_json::json!({ "text": p, "session": session }),
                                            );
                                        }
                                    }
                                    Ok(None) => {}
                                    Err(e) => {
                                        log::error!("[vibeking preview] qwen3 feed: {e}");
                                        break;
                                    }
                                }
                            }
                            // Finalize the streaming session and hand its
                            // accumulated transcript to the stop handler as the
                            // AUTHORITATIVE final for Qwen3 — the batch path
                            // overflows on long audio. Feed any tail captured
                            // since the last tick first so the last word isn't
                            // clipped. (Only for our own session; a superseding
                            // recording leaves the dangling session for the next
                            // streaming_start to reset.)
                            if preview_state.chipbar_generation.load(Relaxed) == session {
                                let (tail, _) =
                                    preview_state.audio_engine.read_samples_from(cursor);
                                if !tail.is_empty() {
                                    let mono: Vec<f32> = if channels <= 1 {
                                        tail
                                    } else {
                                        let ch = channels as usize;
                                        tail.chunks(ch)
                                            .map(|f| f.iter().sum::<f32>() / ch as f32)
                                            .collect()
                                    };
                                    let mono16k = audio_io::resample_to_16k(&mono, rate);
                                    let _ = local_qwen3::streaming_feed(&mono16k);
                                }
                                // streaming_finish returns the complete transcript
                                // of all audio fed — the authoritative Qwen3 final
                                // (the batch path overflows on long audio).
                                let final_text = match local_qwen3::streaming_finish() {
                                    Ok(t) if !t.trim().is_empty() => Some(t),
                                    Ok(_) => None,
                                    Err(e) => {
                                        log::error!(
                                            "[vibeking preview] qwen3 streaming_finish: {e}"
                                        );
                                        None
                                    }
                                };
                                *preview_state.qwen3_stream_final.lock() =
                                    Some((session, final_text));
                            }
                            return;
                        }

                        // EXPERIMENTAL streaming Gemma (Settings.gemma_streaming):
                        // stateful MLX sidecar with KV-cache reuse — prefills only
                        // NEW audio per tick instead of re-decoding the whole growing
                        // window. Mirrors the Qwen3 streaming branch above but drives
                        // gemma_stream_server.py over the session HTTP API; the
                        // authoritative final comes from session/finish (not
                        // truncated at the 30 s audio-encoder cap). When the flag is
                        // off, control falls through to the windowed loop below.
                        // See resources/gemma_stream_server.py.
                        if settings_snap.gemma_streaming
                            && settings_snap.provider == stt::Provider::Gemma
                            && gemma_server::engine_ready()
                        {
                            let lang = settings_snap.stt_language.clone();
                            let hint = if lang == "auto" { None } else { Some(lang) };
                            let (port, sid) = match stt::gemma_session_start(
                                settings_snap.model.as_deref(),
                                hint.as_deref(),
                            )
                            .await
                            {
                                Ok(v) => v,
                                Err(e) => {
                                    // Instrumentation #2 — distinguish "sidecar still
                                    // warming after idle-unload" (running but model not
                                    // loaded) from a genuine error. Either way the
                                    // stop-time recovery re-runs the captured clip.
                                    log::warn!(
                                        "[vibeking preview] gen={session} gemma session start failed \
                                         (running={}, likely cold-start warming): {e} — \
                                         deferring to stop-time recovery",
                                        gemma_server::is_running(),
                                    );
                                    *preview_state.gemma_stream_final.lock() =
                                        Some((session, None));
                                    return;
                                }
                            };
                            let (rate, channels) = preview_state.audio_engine.capture_meta();
                            if rate == 0 {
                                // Instrumentation #4 — cold-audio race: the cpal stream
                                // hadn't published capture meta yet when the (slow,
                                // cold) session opened. Audio survives in the buffer;
                                // stop-time recovery catches it.
                                log::warn!(
                                    "[vibeking preview] gen={session} cold audio — capture_meta \
                                     rate=0, aborting live preview (stop-time recovery will catch it)"
                                );
                                stt::gemma_session_cancel(port, &sid).await;
                                *preview_state.gemma_stream_final.lock() = Some((session, None));
                                return;
                            }
                            let mut cursor = 0usize;
                            let mut last = String::new();
                            // A single feed error (e.g. an HTTP 500 from a window-slide
                            // edge in the long-form session) must NOT kill live preview
                            // for the rest of the recording — tolerate transient errors
                            // and only give up after several in a row. session/finish
                            // (or stop-time recovery) is still the authoritative final.
                            let mut feed_errors = 0u32;
                            const MAX_FEED_ERRORS: u32 = 3;
                            // Wait for ~250 ms of NEW audio before a feed — each feed
                            // re-encodes + decodes, so tiny chunks aren't worth a tick.
                            let min_feed =
                                (rate as usize / 4) * (channels.max(1) as usize);
                            loop {
                                if !preview_state.audio_engine.is_recording()
                                    || preview_state.chipbar_generation.load(Relaxed) != session
                                {
                                    break;
                                }
                                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                                let (chunk, next) =
                                    preview_state.audio_engine.read_samples_from(cursor);
                                if chunk.len() < min_feed.max(1) {
                                    continue; // accumulate until enough new audio
                                }
                                cursor = next;
                                let mono: Vec<f32> = if channels <= 1 {
                                    chunk
                                } else {
                                    let ch = channels as usize;
                                    chunk
                                        .chunks(ch)
                                        .map(|f| f.iter().sum::<f32>() / ch as f32)
                                        .collect()
                                };
                                let mono16k = audio_io::resample_to_16k(&mono, rate);
                                match stt::gemma_session_feed(port, &sid, &mono16k).await {
                                    Ok(p) => {
                                        feed_errors = 0;
                                        if !p.is_empty()
                                            && p != last
                                            && preview_state.chipbar_generation.load(Relaxed)
                                                == session
                                        {
                                            last = p.clone();
                                            let _ = preview_h.emit(
                                                "transcript:interim",
                                                serde_json::json!({ "text": p, "session": session }),
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        feed_errors += 1;
                                        log::warn!(
                                            "[vibeking preview] gen={session} gemma session feed \
                                             error {feed_errors}/{MAX_FEED_ERRORS} (continuing): {e}"
                                        );
                                        if feed_errors >= MAX_FEED_ERRORS {
                                            log::error!(
                                                "[vibeking preview] gen={session} too many feed \
                                                 errors — stopping live preview (final still comes \
                                                 from session/finish or stop-time recovery)"
                                            );
                                            break;
                                        }
                                    }
                                }
                            }
                            // Finalize: feed the tail captured since the last tick, then
                            // take session/finish as the authoritative final. Only for
                            // our own session — a superseding recording owns the slot.
                            if preview_state.chipbar_generation.load(Relaxed) == session {
                                let (tail, _) =
                                    preview_state.audio_engine.read_samples_from(cursor);
                                if !tail.is_empty() {
                                    let mono: Vec<f32> = if channels <= 1 {
                                        tail
                                    } else {
                                        let ch = channels as usize;
                                        tail.chunks(ch)
                                            .map(|f| f.iter().sum::<f32>() / ch as f32)
                                            .collect()
                                    };
                                    let mono16k = audio_io::resample_to_16k(&mono, rate);
                                    let _ = stt::gemma_session_feed(port, &sid, &mono16k).await;
                                }
                                let final_text =
                                    match stt::gemma_session_finish(port, &sid).await {
                                        Ok(t) if !t.trim().is_empty() => Some(t),
                                        Ok(_) => None,
                                        Err(e) => {
                                            log::error!(
                                                "[vibeking preview] gemma session finish: {e}"
                                            );
                                            None
                                        }
                                    };
                                *preview_state.gemma_stream_final.lock() =
                                    Some((session, final_text));
                            } else {
                                stt::gemma_session_cancel(port, &sid).await;
                            }
                            return;
                        }

                        // Gemma 4 E4B (mlx-vlm): clip-based, NO token streaming — each
                        // call returns the whole window's transcript as one blob. The
                        // generic loop below re-decodes the whole GROWING buffer every
                        // pass, which is O(length) on a slow multimodal model: past
                        // ~30-40 s the per-pass decode outruns the speech you add, so
                        // the live card visibly FREEZES while recording continues (the
                        // authoritative whole-clip decode on stop still recovers the
                        // full text — only the live preview stalls). Fix: keep a
                        // COMMITTED prefix and only re-decode a
                        // bounded TRAILING WINDOW, folding it into the prefix at pauses
                        // (VAD) or a hard cap, so every pass is constant-cost regardless
                        // of recording length. No stream_final slot: the stop handler's
                        // whole-clip batch decode remains the authoritative E4B final.
                        if settings_snap.provider == stt::Provider::Gemma
                            && gemma_server::engine_ready()
                        {
                            use std::time::Duration;
                            const PASS_INTERVAL: Duration = Duration::from_millis(700);
                            const MIN_PASS_MS: u64 = 400; // need new audio to bother
                            const COMMIT_PAUSE_MS: u64 = 1_200; // min window to commit on a pause
                            const HARD_MAX_MS: u64 = 8_000; // force-commit a runaway-long window
                            const TAIL_SILENCE_MS: u64 = 350; // "paused" = this much quiet tail
                            // Cap a preview window's generation — windows are ≤8 s, so a
                            // long output is a refusal/hallucination ramble; the stop
                            // decode (max_tokens 1024) stays authoritative.
                            const PREVIEW_MAX_TOKENS: u32 = 256;
                            // Throttle per-token interim emits so a fast token burst
                            // (E4B ≈160 tok/s) doesn't flood the chipbar event channel.
                            const EMIT_THROTTLE_MS: u64 = 60;

                            let mut committed = String::new();
                            let mut commit_cursor = 0usize;
                            // Brief initial gather so the first window has real audio.
                            tokio::time::sleep(Duration::from_millis(450)).await;
                            loop {
                                if !preview_state.audio_engine.is_recording()
                                    || preview_state.chipbar_generation.load(Relaxed) != session
                                {
                                    break;
                                }
                                let (window, win_end) =
                                    preview_state.audio_engine.read_samples_from(commit_cursor);
                                let (rate, channels) =
                                    preview_state.audio_engine.capture_meta();
                                if rate == 0 || window.is_empty() {
                                    tokio::time::sleep(Duration::from_millis(250)).await;
                                    continue;
                                }
                                let frame = channels.max(1) as u64;
                                let win_ms =
                                    window.len() as u64 * 1000 / (rate as u64 * frame).max(1);
                                if win_ms < MIN_PASS_MS {
                                    tokio::time::sleep(Duration::from_millis(250)).await;
                                    continue;
                                }
                                // Trim trailing silence from the DECODE window so Gemma
                                // doesn't read the pause as "your turn" and reply; keep
                                // the original `window` for the pause/commit checks.
                                let speech_len =
                                    trim_trailing_silence_len(&window, rate, channels);
                                let decode_window = &window[..speech_len];
                                // No speech yet: hold the committed prefix and wait — a
                                // chat-tuned LLM fed silence/ambient hallucinates a reply.
                                if !has_speech(decode_window, rate, channels) {
                                    tokio::time::sleep(PASS_INTERVAL).await;
                                    continue;
                                }
                                let wav = encode_wav_bytes(decode_window, rate, channels);
                                let input = stt::TranscribeInput {
                                    wav: &wav,
                                    api_key: "",
                                    language: Some(settings_snap.stt_language.as_str()),
                                    hotwords: &[],
                                    context_candidates: &[],
                                    model: settings_snap.model.as_deref(),
                                };
                                // Stream the window's tokens as they decode so the
                                // live card flows token-by-token instead of snapping
                                // in one blob per pass. `committed` is borrowed
                                // read-only for the call; the refusal/hallucination
                                // guards run on each cumulative partial so neither
                                // ever flashes mid-stream.
                                // `&AtomicU64` is Send (Cell is not), keeping the
                                // streaming future Send for the spawned preview task.
                                let stream_start = std::time::Instant::now();
                                let last_emit_ms = std::sync::atomic::AtomicU64::new(0);
                                let win_text = match stt::transcribe_gemma_streaming(
                                    input,
                                    PREVIEW_MAX_TOKENS,
                                    |cumulative| {
                                        let elapsed =
                                            stream_start.elapsed().as_millis() as u64;
                                        if elapsed
                                            < last_emit_ms.load(Relaxed) + EMIT_THROTTLE_MS
                                        {
                                            return;
                                        }
                                        if cumulative.is_empty()
                                            || is_preview_hallucination(cumulative)
                                            || is_chat_refusal(cumulative)
                                        {
                                            return;
                                        }
                                        if preview_state.chipbar_generation.load(Relaxed)
                                            != session
                                        {
                                            return;
                                        }
                                        last_emit_ms.store(elapsed, Relaxed);
                                        let display =
                                            join_transcript(&committed, cumulative);
                                        let _ = preview_h.emit(
                                            "transcript:interim",
                                            serde_json::json!({
                                                "text": display,
                                                "session": session,
                                            }),
                                        );
                                    },
                                )
                                .await
                                {
                                    Ok(out) => out.text.trim().to_string(),
                                    Err(e) => {
                                        log::error!("[vibeking preview] gemma window: {e}");
                                        tokio::time::sleep(PASS_INTERVAL).await;
                                        continue;
                                    }
                                };
                                let usable = !win_text.is_empty()
                                    && !is_preview_hallucination(&win_text)
                                    && !is_chat_refusal(&win_text);
                                // Paint committed-prefix + this window's text. The
                                // ChipBar prefix-diff keeps the stable prefix solid and
                                // only reveals the changing tail, so window churn doesn't
                                // flicker the whole line.
                                if usable
                                    && preview_state.chipbar_generation.load(Relaxed) == session
                                {
                                    let display = join_transcript(&committed, &win_text);
                                    let _ = preview_h.emit(
                                        "transcript:interim",
                                        serde_json::json!({
                                            "text": display,
                                            "session": session,
                                        }),
                                    );
                                }
                                let paused =
                                    tail_is_silent(&window, rate, channels, TAIL_SILENCE_MS);
                                if usable
                                    && (win_ms >= HARD_MAX_MS
                                        || (paused && win_ms >= COMMIT_PAUSE_MS))
                                {
                                    // Commit: fold the window into the stable prefix and
                                    // advance the cursor so the next window stays small.
                                    committed = join_transcript(&committed, &win_text);
                                    commit_cursor = win_end;
                                } else if win_ms >= HARD_MAX_MS {
                                    // Runaway-long window that stayed unusable (refusal /
                                    // hallucination) — drop it so passes don't grow
                                    // unbounded; the stop decode re-reads the tail anyway.
                                    commit_cursor = win_end;
                                }
                                tokio::time::sleep(PASS_INTERVAL).await;
                            }
                            // Hand the windowed transcript to the stop handler as the
                            // authoritative final for LONG clips (the whole-clip decode
                            // truncates at the model's ~30 s audio-encoder cap; the
                            // windowed `committed` prefix covers the whole recording).
                            // Only for OUR session — a superseding recording owns the
                            // slot. Transcribe the final uncommitted tail window and
                            // fold it in first so the ending isn't lost (mirrors how
                            // the Qwen3 streaming finish feeds its tail).
                            if preview_state.chipbar_generation.load(Relaxed) == session {
                                let (tail, _) =
                                    preview_state.audio_engine.read_samples_from(commit_cursor);
                                let (rate, channels) = preview_state.audio_engine.capture_meta();
                                if rate != 0 && !tail.is_empty() {
                                    let speech_len =
                                        trim_trailing_silence_len(&tail, rate, channels);
                                    let decode_tail = &tail[..speech_len];
                                    if has_speech(decode_tail, rate, channels) {
                                        let wav = encode_wav_bytes(decode_tail, rate, channels);
                                        let tail_input = stt::TranscribeInput {
                                            wav: &wav,
                                            api_key: "",
                                            language: Some(settings_snap.stt_language.as_str()),
                                            hotwords: &[],
                                            context_candidates: &[],
                                            model: settings_snap.model.as_deref(),
                                        };
                                        if let Ok(out) =
                                            stt::transcribe(stt::Provider::Gemma, tail_input).await
                                        {
                                            let t = out.text.trim();
                                            if !t.is_empty()
                                                && !is_chat_refusal(t)
                                                && !is_preview_hallucination(t)
                                            {
                                                committed = join_transcript(&committed, t);
                                            }
                                        }
                                    }
                                }
                                let final_opt = {
                                    let c = committed.trim();
                                    if c.is_empty() {
                                        None
                                    } else {
                                        Some(c.to_string())
                                    }
                                };
                                *preview_state.gemma_stream_final.lock() =
                                    Some((session, final_opt));
                            }
                            return;
                        }

                        // FluidAudio (Parakeet): frozen-head / live-tail preview.
                        // Each tick we re-decode only a short TRAILING window of the
                        // captured audio (last ~2 s) with the fast timestamped batch
                        // decoder. Tokens older than KEEP_SEC behind the live edge
                        // have settled (full right-context) → frozen into the head,
                        // never re-decoded again; the last KEEP_SEC re-decodes every
                        // tick so it self-corrects word-by-word. The head never
                        // re-words (no whole-sentence churn) and latency is ~one tick
                        // (no multi-second window wait). The authoritative final is
                        // still the stop handler's batch decode WITH bias.
                        if settings_snap.provider == stt::Provider::FluidAudio
                            && local_parakeet::engine_ready()
                        {
                            const SR: f32 = 16_000.0;
                            // Trailing audio kept re-decodable (self-correcting tail).
                            const KEEP_SEC: f32 = 1.2;
                            // Hold the freshest slice of audio back from the display
                            // so the shown last word has some right-context and
                            // doesn't flap between candidates ("forth"↔"fort").
                            const LAG_SEC: f32 = 0.4;
                            // Re-decode this much already-frozen audio for left
                            // context, so the first tail word isn't cut and decodes
                            // as cleanly as the head did.
                            const OVERLAP_SEC: f32 = 1.0;

                            let mut acc16k: Vec<f32> = Vec::new();
                            let mut cursor = 0usize;
                            let mut last = String::new();
                            let mut stitcher = TailStitcher::new();
                            loop {
                                if !preview_state.audio_engine.is_recording()
                                    || preview_state.chipbar_generation.load(Relaxed) != session
                                {
                                    break;
                                }
                                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                                let (chunk, next) =
                                    preview_state.audio_engine.read_samples_from(cursor);
                                cursor = next;
                                let (rate, channels) =
                                    preview_state.audio_engine.capture_meta();
                                if !chunk.is_empty() && rate != 0 {
                                    let mono: Vec<f32> = if channels <= 1 {
                                        chunk
                                    } else {
                                        let ch = channels as usize;
                                        chunk
                                            .chunks(ch)
                                            .map(|f| f.iter().sum::<f32>() / ch as f32)
                                            .collect()
                                    };
                                    acc16k.extend_from_slice(&audio_io::resample_to_16k(&mono, rate));
                                }
                                let total_sec = acc16k.len() as f32 / SR;
                                if total_sec < 0.3 {
                                    continue;
                                }
                                // Re-decode only [head - overlap, now]: bounded to
                                // ~(KEEP + OVERLAP) ≈ 2.2 s, so each decode is cheap.
                                let window_start_sec = (stitcher.head_time - OVERLAP_SEC).max(0.0);
                                let start = ((window_start_sec * SR) as usize).min(acc16k.len());
                                let slice = acc16k[start..].to_vec();
                                match local_parakeet::preview_transcribe_timed(slice).await {
                                    Ok(tokens) => {
                                        let display = stitcher.update(
                                            window_start_sec,
                                            &tokens,
                                            total_sec,
                                            KEEP_SEC,
                                            LAG_SEC,
                                        );
                                        if !display.is_empty()
                                            && display != last
                                            && preview_state.chipbar_generation.load(Relaxed)
                                                == session
                                        {
                                            last = display.clone();
                                            let _ = preview_h.emit(
                                                "transcript:interim",
                                                serde_json::json!({ "text": display, "session": session }),
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        log::error!("[vibeking preview] parakeet preview_transcribe_timed: {e}");
                                        break;
                                    }
                                }
                            }
                            return;
                        }

                        // Remaining engines (cloud STT, local Whisper) re-transcribe
                        // the whole buffer per pass over an HTTP
                        // round-trip — poll them gently. The loop also self-throttles
                        // (it awaits each pass before sleeping), so the interval is a
                        // floor, not a guaranteed cadence. (Gemma E4B is handled by
                        // its own windowed branch above.)
                        let interval = std::time::Duration::from_millis(700);
                        log::info!(
                            "[vibeking preview] live-preview loop start session={session} provider={:?}",
                            settings_snap.provider
                        );
                        let mut last = String::new();
                        // Brief initial gather so the first pass has real audio.
                        tokio::time::sleep(std::time::Duration::from_millis(450)).await;
                        loop {
                            if !preview_state.audio_engine.is_recording()
                                || preview_state.chipbar_generation.load(Relaxed) != session
                            {
                                break;
                            }
                            let (mut samples, _) = preview_state.audio_engine.read_samples_from(0);
                            let (rate, channels) = preview_state.audio_engine.capture_meta();
                            // Need ~0.3 s of audio before a pass is worthwhile.
                            let min_samples =
                                (rate as usize) * (channels.max(1) as usize) * 3 / 10;
                            if rate == 0 || samples.len() < min_samples {
                                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                                continue;
                            }
                            // Voice gate: skip the pass until the buffer carries real
                            // speech — Whisper/cloud STT hallucinate canned phrases on
                            // silent input, which would flash in the live card before
                            // the user has spoken (and it saves a model call per idle
                            // tick). Use the GAIN-INVARIANT detector (dynamic range,
                            // not an absolute RMS floor) — the same one the batch path
                            // uses — so a quiet-but-real mic (RØDE / laptop, peak
                            // ~0.04–0.09) isn't rejected as silence, which left the
                            // live preview permanently blank while the batch decoded
                            // fine. (The old `preview_rms < PREVIEW_MIN_RMS` floor sat
                            // ABOVE such a mic's whole-buffer RMS.)
                            if !crate::audio::capture_has_voice(&samples, rate, channels) {
                                tokio::time::sleep(interval).await;
                                continue;
                            }
                            // Lift a quiet take to a normal level before decoding so
                            // Whisper transcribes it as cleanly as the (already
                            // gain-normalized) batch pass. No-op for already-audible
                            // input; gated internally by the same voice check, so
                            // silence/hum is never amplified.
                            crate::audio::normalize_capture_gain(&mut samples, rate, channels);
                            let wav = encode_wav_bytes(&samples, rate, channels);
                            let input = stt::TranscribeInput {
                                wav: &wav,
                                api_key: settings_snap.api_key_for(settings_snap.provider),
                                language: Some(settings_snap.stt_language.as_str()),
                                hotwords: &[], // skip bias on the preview for speed
                                context_candidates: &[],
                                model: settings_snap.model.as_deref(),
                            };
                            match stt::transcribe(settings_snap.provider, input).await {
                                Ok(out) => {
                                    let text = out.text.trim().to_string();
                                    if !text.is_empty()
                                        && !is_preview_hallucination(&text)
                                        && !is_chat_refusal(&text)
                                        && text != last
                                        && preview_state.chipbar_generation.load(Relaxed)
                                            == session
                                    {
                                        last = text.clone();
                                        let _ = preview_h.emit(
                                            "transcript:interim",
                                            serde_json::json!({
                                                "text": text,
                                                "session": session,
                                            }),
                                        );
                                    }
                                }
                                // Best-effort: the authoritative pass runs on stop.
                                Err(e) => {
                                    log::error!("[vibeking preview] transcribe error: {e}");
                                }
                            }
                            // Sleep AFTER the pass, so the first paint is early.
                            tokio::time::sleep(interval).await;
                        }
                    });
                }

                // Screen-context biasing (macOS only). Race-safe via a generation
                // counter: late-arriving captures from older recordings are dropped.
                #[cfg(target_os = "macos")]
                {
                    let mode = state.settings.lock().screen_context_mode;
                    if mode != state::ScreenContextMode::Off {
                        let gen = state
                            .context_generation
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                            + 1;
                        state.pending_context.lock().take();
                        let context_state = state.clone();
                        let context_handle = h.clone();
                        tauri::async_runtime::spawn(async move {
                            match screen_context::capture(context_handle, mode).await {
                                Ok(Some(cc)) => {
                                    let candidates =
                                        proper_nouns::extract_candidates(&cc.text, 50);
                                    let still_current = context_state
                                        .context_generation
                                        .load(std::sync::atomic::Ordering::Relaxed)
                                        == gen;
                                    if still_current && !candidates.is_empty() {
                                        *context_state.pending_context.lock() = Some(candidates);
                                    }
                                }
                                Ok(None) => {}
                                Err(e) => {
                                    log::error!("[vibeking] screen_context capture error: {e:#}");
                                }
                            }
                        });
                    }
                }
            });

            // recording:stop → stop mic, transcribe, polish/translate, insert, hide chip bar
            let stop_handle = handle.clone();
            app.listen_any("recording:stop", move |_| {
                let h = stop_handle.clone();
                let state = h.state::<state::SharedState>().inner().clone();
                let gen = state
                    .chipbar_generation
                    .load(std::sync::atomic::Ordering::Relaxed);
                log::info!("[vibeking listener] recording:stop  gen={gen}");
                let mut clip = match state.audio_engine.stop_recording() {
                    Ok(c) => c,
                    Err(e) => {
                        log::error!("[vibeking] failed to finalize audio: {e}");
                        let _ = h.emit("recording:error", format!("audio: {e}"));
                        windows::hide_chipbar(&h);
                        return;
                    }
                };
                log::info!(
                    "[vibeking listener] recording:stop clip duration={} ms peak={:.4} gen={gen}",
                    clip.duration_ms, clip.peak,
                );

                // Skip transcribe entirely for clips that are obviously
                // unintended — too short to contain real speech, or
                // recorded with effectively zero signal. The most common
                // cause is a hotkey misfire: the user entered Toggle
                // mode via a quick press-release, then "started a new
                // recording" which actually stopped the empty toggle
                // one. Surfacing the resulting garbage transcript would
                // paste junk into the focused window. Just hide the
                // chip bar so the user can try again cleanly.
                const MIN_DURATION_MS: u64 = 250;
                const MIN_PEAK: f32 = 0.01;
                // True when we fell back to the untrimmed audio below. A recovered
                // clip is low-confidence (the trim thought it was silence), so the
                // transcribe path treats a hallucination/refusal result as "nothing"
                // rather than pasting filler like a Whisper "hello hello hello".
                let mut recovered = false;
                // Qwen3 produces its AUTHORITATIVE transcript from the live session
                // that ran during recording (held in the qwen3_stream_final slot),
                // NOT from this batch clip. A quiet-but-real take — low mic
                // gain, or just sitting back from a laptop mic — can peak below
                // MIN_PEAK (or trim away to 0 ms usable) while the live session
                // streamed a perfect transcript. The peak-based silent-clip gate
                // would discard that recording before the async path ever consults
                // the streamed final ("live transcription worked, then the whole
                // thing vanished on stop"). So for those engines skip the gate when a
                // substantial recording actually happened; gate on RAW length so a
                // genuine hotkey misfire (a tiny take) is still dropped. If streaming
                // truly produced nothing, the empty-transcript handler downstream
                // still hides the bar — nothing gets pasted.
                let provider = state.settings.lock().provider;
                // Engines that streamed a live transcript during recording: a real
                // (≥250 ms raw) take that the live session transcribed must not be
                // discarded by the peak/trim silent-clip gate just because it's
                // short or quiet. Qwen3 then uses its streamed final; Parakeet falls
                // through to the batch decode (which captures the short clip fine) —
                // either way the word the user saw live reaches the final.
                let streamed_recording = clip.raw_duration_ms >= MIN_DURATION_MS
                    && ((provider == stt::Provider::Qwen3 && local_qwen3::engine_ready())
                        || (provider == stt::Provider::FluidAudio
                            && local_parakeet::engine_ready()));
                if streamed_recording {
                    log::info!(
                        "[vibeking] {provider:?} streamed recording (raw {} ms, batch peak={:.4}) — skipping silent-clip gate, using live transcript",
                        clip.raw_duration_ms, clip.peak,
                    );
                }
                if !streamed_recording
                    && (clip.duration_ms < MIN_DURATION_MS || clip.peak < MIN_PEAK)
                {
                    // The TRIMMED clip looks unusable — but the leading-silence trim
                    // over-eats genuinely quiet speech (its 0.003 / 10 ms bar) and an
                    // absolute peak gate misjudges a low-gain mic (a laptop mic peaks
                    // at ~0.04). Trust the gain-invariant voice detector (the same one
                    // that gates capture normalization): if the RAW take actually
                    // carried speech, transcribe the (normalized) raw audio instead of
                    // discarding the recording. This is the "live preview transcribed
                    // it, then stop dropped everything as silent" bug. `recovered`
                    // marks it low-confidence so a hallucination/refusal is treated as
                    // nothing downstream.
                    let salvageable = clip.raw_has_voice || clip.peak >= MIN_PEAK;
                    if salvageable {
                        if let Some(raw_wav) = clip.recovery_wav.take() {
                            // The trim over-ate a substantial take — retry on the full
                            // (normalized) raw audio.
                            log::info!(
                                "[vibeking] trimmed clip empty ({} ms) — retrying on raw {} ms audio (voice={}, peak={:.4})",
                                clip.duration_ms, clip.raw_has_voice, clip.raw_duration_ms, clip.peak
                            );
                            clip.wav = raw_wav;
                            clip.duration_ms = clip.raw_duration_ms;
                            recovered = true;
                        } else if clip.duration_ms < MIN_DURATION_MS {
                            // Energy/voice present but too short and no raw fallback (a
                            // tiny take / hotkey misfire) → nothing to do.
                            log::info!(
                                "[vibeking] skipping transcribe — short clip ({} ms, peak={:.4}, raw={} ms)",
                                clip.duration_ms, clip.peak, clip.raw_duration_ms
                            );
                            windows::hide_chipbar(&h);
                            return;
                        } else {
                            // A real, long-enough take that's just quiet (peak below
                            // the gate) and wasn't over-trimmed — transcribe the
                            // trimmed clip as-is, low-confidence.
                            log::info!(
                                "[vibeking] quiet-but-voiced clip ({} ms, peak={:.4}) — transcribing as-is",
                                clip.duration_ms, clip.peak
                            );
                            recovered = true;
                        }
                    } else {
                        // Genuinely (near-)silent: NO voice AND low peak. Distinguish a
                        // hotkey misfire (tiny raw clip → hide quietly) from a real
                        // recording that captured silence (long raw clip but silent →
                        // the mic was dead/muted/wrong device).
                        log::info!(
                            "[vibeking] skipping transcribe — silent clip ({} ms, peak={:.4}, raw={} ms)",
                            clip.duration_ms, clip.peak, clip.raw_duration_ms
                        );
                        // No audio → just dismiss. Surfacing an error here was
                        // more annoying than helpful (a muted mic / misfire is
                        // obvious once nothing pastes); the user asked for a
                        // silent dismiss instead. With the always-on bar this
                        // returns to the idle bar; otherwise it hides.
                        windows::hide_chipbar(&h);
                        return;
                    }
                }

                let settings = state.settings.lock().clone();
                let context_candidates: Vec<String> =
                    state.pending_context.lock().take().unwrap_or_default();
                let candidate_preview = if context_candidates.is_empty() {
                    "[]".to_string()
                } else {
                    let head: Vec<&str> = context_candidates
                        .iter()
                        .take(10)
                        .map(String::as_str)
                        .collect();
                    let more = context_candidates.len().saturating_sub(head.len());
                    if more > 0 {
                        format!("[{}, +{} more]", head.join(", "), more)
                    } else {
                        format!("[{}]", head.join(", "))
                    }
                };
                log::info!(
                    "[vibeking] recording:stop  active_mode={:?}  provider={:?}  target={:?}  context_candidates({})={}",
                    settings.active_refinement_mode_id,
                    settings.provider,
                    settings.translate_target,
                    context_candidates.len(),
                    candidate_preview,
                );
                // Snapshot the chipbar generation at spawn time. The post-
                // transcript "result hold" sleep below uses this to detect
                // whether a new recording has started in the meantime — if
                // so, the new recording owns the chip bar and we must not
                // hide it from this stale cleanup path.
                let chipbar_gen = state
                    .chipbar_generation
                    .load(std::sync::atomic::Ordering::Relaxed);
                let chipbar_state = state.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = h.emit(
                        "transcription:start",
                        serde_json::json!({ "session": chipbar_gen }),
                    );

                    // For Qwen3 (and every other provider) the authoritative
                    // final transcript is the normal batch dispatch. Qwen3's
                    // optional live streaming preview during recording is
                    // separate (see the recording:start handler).
                    let input = stt::TranscribeInput {
                        wav: &clip.wav,
                        api_key: settings.api_key_for(settings.provider),
                        language: Some(settings.stt_language.as_str()),
                        hotwords: &settings.hotwords,
                        context_candidates: &context_candidates,
                        model: settings.model.as_deref(),
                    };
                    // Qwen3's authoritative final is the streaming session's
                    // accumulated transcript (handles arbitrarily long audio),
                    // NOT a batch re-transcribe (which overflows on long clips
                    // → "Qwen3 transcription failed"). Wait briefly for the
                    // live-preview task to finalize it; fall back to batch only
                    // if streaming wasn't active or produced nothing (short clip,
                    // engine not ready). All other providers go straight to batch.
                    // Gemma's whole-clip decode truncates at the model's ~30 s
                    // audio-encoder cap, so for clips past this threshold use the
                    // windowed preview's full-length transcript instead. Short
                    // clips keep the cleaner single-shot whole-clip decode.
                    const GEMMA_WINDOWED_FINAL_MS: u64 = 25_000;
                    let stt_result = if settings.provider == stt::Provider::Qwen3
                        && local_qwen3::engine_ready()
                    {
                        match await_qwen3_stream_final(&chipbar_state, chipbar_gen).await {
                            Some(text) => Ok(stt::TranscribeOutput {
                                text,
                                provider: stt::Provider::Qwen3,
                                model: local_qwen3::ENGINE_ID.to_string(),
                            }),
                            None => stt::transcribe(settings.provider, input).await,
                        }
                    } else if settings.provider == stt::Provider::Gemma
                        && gemma_server::engine_ready()
                        && (settings.gemma_streaming
                            || clip.duration_ms >= GEMMA_WINDOWED_FINAL_MS)
                    {
                        // Streaming mode: session/finish is the authoritative final
                        // for ALL clips (not truncated). When the live session produced
                        // NO final — the classic idle cold-start: the sidecar was
                        // idle-unloaded, its model reload outlasted a short first
                        // utterance, and the preview task aborted early — we don't give
                        // up. The captured audio survives in the buffer and the sidecar
                        // is warm by now, so recover_gemma_streaming re-runs the whole
                        // clip through a one-shot streaming session (same mode — no
                        // batch-server respawn / mode ping-pong). Windowed mode (flag
                        // off, long clip): keep the whole-clip decode as the fallback.
                        let gemma_model = settings
                            .model
                            .clone()
                            .unwrap_or_else(|| gemma_server::DEFAULT_MODEL.to_string());
                        match await_gemma_stream_final(&chipbar_state, chipbar_gen).await {
                            Some(text) => Ok(stt::TranscribeOutput {
                                text,
                                provider: stt::Provider::Gemma,
                                model: gemma_model,
                            }),
                            None if settings.gemma_streaming => {
                                // Instrumentation #3 — the exact silent end-state today:
                                // live streaming produced no final. Log it, then attempt
                                // the one-shot recovery and log whether it rescued the
                                // utterance (vs. the user truly said nothing).
                                let hint = if settings.stt_language == "auto" {
                                    None
                                } else {
                                    Some(settings.stt_language.as_str())
                                };
                                log::warn!(
                                    "[vibeking] gen={chipbar_gen} streaming final empty — \
                                     attempting one-shot recovery from captured clip"
                                );
                                let recovered = recover_gemma_streaming(
                                    &clip.wav,
                                    settings.model.as_deref(),
                                    hint,
                                )
                                .await;
                                log::info!(
                                    "[vibeking] gen={chipbar_gen} one-shot recovery {}",
                                    if recovered.is_some() {
                                        "succeeded"
                                    } else {
                                        "found nothing (no speech captured)"
                                    },
                                );
                                Ok(stt::TranscribeOutput {
                                    text: recovered.unwrap_or_default(),
                                    provider: stt::Provider::Gemma,
                                    model: gemma_model,
                                })
                            }
                            None => stt::transcribe(settings.provider, input).await,
                        }
                    } else {
                        stt::transcribe(settings.provider, input).await
                    };

                    // Long-audio safety net. Qwen3 (512-token decoder cache,
                    // ≈45-50 s of speech) and Gemma (~30 s audio encoder) cannot
                    // transcribe long clips — they error or return nothing. When
                    // that happens on a clip long enough to have tripped the cap,
                    // re-transcribe with local Whisper, which windows internally
                    // and handles arbitrarily long audio. Best-effort: only when
                    // Whisper's model is already on disk, so we never trigger a
                    // surprise multi-hundred-MB download mid-dictation. (Proper
                    // fix: window the Qwen3 streaming session — see local_qwen3.)
                    const LONG_AUDIO_FALLBACK_MS: u64 = 40_000;
                    let on_device_short_window_engine = matches!(
                        settings.provider,
                        stt::Provider::Qwen3 | stt::Provider::Gemma
                    );
                    let needs_fallback = match &stt_result {
                        Err(_) => true,
                        Ok(out) => {
                            let t = out.text.trim();
                            // Empty on a long clip → the engine choked on length.
                            (t.is_empty() && clip.duration_ms >= LONG_AUDIO_FALLBACK_MS)
                                // Or the chat-tuned model REPLIED to / REFUSED the
                                // audio instead of transcribing it (a known Gemma-12B
                                // failure mode, e.g. "I cannot fulfill this request —
                                // no audio file provided"). That text must never be
                                // pasted; re-transcribe with Whisper instead.
                                || is_chat_refusal(t)
                        }
                    };
                    let stt_result = if on_device_short_window_engine
                        && needs_fallback
                        && local_stt::model_exists(local_stt::DEFAULT_MODEL_ID)
                    {
                        log::info!(
                            "[vibeking] {:?} produced no transcript on a {} ms clip \
                             (likely the on-device long-audio cap) — falling back to Whisper",
                            settings.provider, clip.duration_ms,
                        );
                        let fallback_input = stt::TranscribeInput {
                            wav: &clip.wav,
                            api_key: "",
                            language: Some(settings.stt_language.as_str()),
                            hotwords: &settings.hotwords,
                            context_candidates: &context_candidates,
                            model: None, // Whisper default (DEFAULT_MODEL_ID)
                        };
                        match stt::transcribe(stt::Provider::Local, fallback_input).await {
                            Ok(out) => {
                                log::info!(
                                    "[vibeking] Whisper fallback OK ({} chars)",
                                    out.text.chars().count(),
                                );
                                Ok(out)
                            }
                            // Fall back failed too — surface the original error.
                            Err(e) => {
                                log::error!("[vibeking] Whisper fallback also failed: {e}");
                                stt_result
                            }
                        }
                    } else {
                        stt_result
                    };

                    match stt_result {
                        Ok(out)
                            if out.text.trim().is_empty()
                                || (recovered
                                    && (is_preview_hallucination(&out.text)
                                        || is_chat_refusal(&out.text))) =>
                        {
                            // Nothing usable was transcribed: an empty result, or a
                            // recovered (untrimmed) clip that the engine filled with a
                            // canned hallucination / chat refusal because it was
                            // actually silent. Don't emit/paste it or hold an empty
                            // chip bar — just hide it. Respect generation so we don't
                            // hide a newer recording's chip bar.
                            log::info!("[vibeking] empty/unusable transcript — hiding chip bar");
                            let current_gen = chipbar_state
                                .chipbar_generation
                                .load(std::sync::atomic::Ordering::Relaxed);
                            if current_gen == chipbar_gen {
                                windows::hide_chipbar(&h);
                            }
                        }
                        Ok(out) => {
                            let raw = out.text.clone();
                            // Surface the raw transcript so the chip bar can show
                            // it during the polish/translate phase. Emitted BEFORE
                            // the learned-dictionary substitution so the debug
                            // panel reflects exactly what STT produced.
                            let _ = h.emit(
                                "transcript:raw",
                                serde_json::json!({ "text": raw, "session": chipbar_gen }),
                            );

                            // Apply the user's learned correction dictionary
                            // (word-boundary regex substitution). Fast and
                            // deterministic — see corrections::apply_dictionary.
                            // The polish prompt also gets the dictionary as
                            // context for cases the substitution missed.
                            let dict_snapshot =
                                settings.correction_dictionary.clone();
                            let raw_for_polish = if dict_snapshot.is_empty() {
                                raw.clone()
                            } else {
                                corrections::apply_dictionary(&raw, &dict_snapshot)
                            };

                            let (final_text, polished, translated) =
                                run_refinement(&raw_for_polish, &settings, &h, chipbar_gen).await;

                            let event = TranscriptEvent {
                                raw: raw.clone(),
                                text: final_text.clone(),
                                provider: format!("{:?}", out.provider),
                                model: out.model,
                                duration_ms: clip.duration_ms,
                                translated,
                                polished,
                                session: chipbar_gen,
                            };
                            let _ = h.emit("transcript:complete", &event);

                            if let Err(e) = insert::write_text(h.clone(), final_text).await {
                                log::error!("[vibeking] insert failed: {e}");
                                let _ = h.emit("recording:error", format!("insert: {e}"));
                            }

                            // Hold the chip bar on the final state briefly so the
                            // user sees the result, then hide — but skip the
                            // hide if a new recording has started in the
                            // meantime (Parakeet's sub-second STT makes this
                            // race trivial to hit by rapid-firing the hotkey).
                            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                            let current_gen = chipbar_state
                                .chipbar_generation
                                .load(std::sync::atomic::Ordering::Relaxed);
                            if current_gen == chipbar_gen {
                                windows::hide_chipbar(&h);
                            }
                        }
                        Err(e) => {
                            let current_gen = chipbar_state
                                .chipbar_generation
                                .load(std::sync::atomic::Ordering::Relaxed);
                            if current_gen == chipbar_gen {
                                windows::hide_chipbar(&h);
                            }
                            log::error!("[vibeking] transcribe failed: {e}");
                            let _ = h.emit("recording:error", format!("transcribe: {e}"));
                        }
                    }
                });
            });

            // recording:cancel → drop the audio buffer, hide chip bar, drop any
            // pending screen-context candidates and bump generation so a still-
            // running capture is invalidated.
            let cancel_handle = handle.clone();
            app.listen_any("recording:cancel", move |_| {
                let h = cancel_handle.clone();
                let state = h.state::<state::SharedState>().inner().clone();
                state.audio_engine.cancel_recording();
                state
                    .chipbar_generation
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                #[cfg(target_os = "macos")]
                {
                    state
                        .context_generation
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let _ = state.pending_context.lock().take();
                }
                windows::hide_chipbar(&h);
            });

            // insert:complete → kick off the post-paste correction watcher.
            // No-op on non-macOS, and no-op if correction_learning_mode == Off.
            #[cfg(target_os = "macos")]
            {
                let insert_handle = handle.clone();
                app.listen_any("insert:complete", move |event| {
                    #[derive(serde::Deserialize)]
                    struct InsertCompletePayload {
                        pasted_text: String,
                    }
                    let payload: InsertCompletePayload = match serde_json::from_str(event.payload())
                    {
                        Ok(p) => p,
                        Err(e) => {
                            log::error!(
                                "[vibeking] insert:complete payload parse failed: {e}"
                            );
                            return;
                        }
                    };
                    correction_watcher::spawn_watcher(
                        insert_handle.clone(),
                        payload.pasted_text,
                        correction_watcher::DEFAULT_WATCH_WINDOW,
                    );
                });
            }

            hotkey::start(&handle);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            // Kill the Gemma MLX sidecar on app exit so the ~8 GB model server
            // doesn't outlive the app. (Graceful-quit only; a hard kill of the
            // app can't run this — see gemma_server hardening notes.)
            if let tauri::RunEvent::Exit = event {
                gemma_server::shutdown();
            }
        });
}

/// Live-preview voice-activity floor (RMS over the captured buffer). Below
/// this the buffer is effectively silence and we skip the preview transcribe
/// pass entirely — Whisper and Gemma both hallucinate canned text on silent
/// input ("thank you", "how are you doing today"), which then flashes in the
/// live card before the user has said anything. Real speech RMS is ~0.02–0.1;
/// a quiet room sits well under this. Tunable.
const PREVIEW_MIN_RMS: f32 = 0.006;

/// Root-mean-square amplitude of an interleaved f32 buffer. f64 accumulation so
/// long buffers don't lose precision. Used as the preview silence gate.
fn preview_rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum_sq / samples.len() as f64).sqrt() as f32
}

/// Backstop for the silence gate: even with some ambient energy, Whisper/Gemma
/// emit a small set of canned hallucinations on (near-)silence. Suppress them
/// from the **preview only** — the authoritative transcript on stop is
/// unaffected, so dropping a genuine lone "thank you" from the live card (rare)
/// is harmless. Match is on the whole normalized string, so real sentences that
/// merely contain these words are never touched.
fn is_preview_hallucination(text: &str) -> bool {
    let norm = text
        .trim()
        .trim_end_matches(|c: char| ".,!?;:。！？；：…\"'）) ".contains(c))
        .trim()
        .to_lowercase();
    const HALLUCINATIONS: &[&str] = &[
        // Whisper on silence (English)
        "thank you",
        "thank you for watching",
        "thanks for watching",
        "thank you very much",
        "please subscribe",
        "you",
        // Gemma (chat model) default replies to empty audio
        "how are you doing today",
        "how are you",
        "how can i help you",
        "how can i help you today",
        "hello, how can i help you",
        // Whisper on silence (Chinese — traditional + simplified)
        "謝謝",
        "謝謝大家",
        "謝謝觀看",
        "谢谢",
        "谢谢大家",
        "谢谢观看",
    ];
    HALLUCINATIONS.contains(&norm.as_str())
}

/// Markers that signal a chat-tuned audio model stopped transcribing and started
/// "talking" — refusals and meta-commentary that never occur in genuine
/// dictation. Used to reject a whole transcript ([`is_chat_refusal`]). Kept
/// deliberately broad on the stems ("i cannot fulfill", not "...this request")
/// so phrasing variants ("I cannot fulfill this") are caught.
const CHAT_DRIFT_MARKERS: &[&str] = &[
    "i cannot fulfill",
    "i can't fulfill",
    "i cannot fulfil", // single-l British spelling Gemma sometimes emits
    "i can't fulfil",
    "i cannot transcribe",
    "i can't transcribe",
    "i'm unable to",
    "i am unable to",
    "i cannot provide",
    "i can't provide",
    "i cannot complete",
    "i can't complete",
    "no audio file",
    "there is no audio",
    "no audio was provided",
    "please upload the audio",
    "please provide the audio",
    "please provide the",
    "i don't have access to the audio",
    "i do not have access to the audio",
    "as an ai",
    "i'm just a",
    "i am just a",
];

/// True when an instruction-tuned audio model REPLIED to (or refused) the speech
/// instead of transcribing it — a known Gemma-12B failure mode where it emits a
/// chat response like "I cannot fulfill this — no audio file provided" as the
/// "transcript". Such text must never be pasted; the stop handler routes a match
/// to the Whisper fallback. Markers don't occur in genuine dictation, so real
/// speech is never matched.
fn is_chat_refusal(text: &str) -> bool {
    let t = text.to_lowercase();
    CHAT_DRIFT_MARKERS.iter().any(|m| t.contains(m))
}

/// Wait for the Qwen3 live-preview task to hand off the streaming session's
/// final transcript for `session`, consuming it. `Some(text)` → use it as the
/// authoritative transcript; `None` → timed out or the session finished empty,
/// so fall back to a batch transcribe. The preview task only calls
/// `streaming_finish()` after the recording stops, so a brief wait is expected;
/// the deadline is generous to cover finalizing a long clip.
async fn await_qwen3_stream_final(state: &state::SharedState, session: u64) -> Option<String> {
    use std::time::{Duration, Instant};
    const MAX_WAIT: Duration = Duration::from_secs(15);
    const POLL: Duration = Duration::from_millis(50);
    let deadline = Instant::now() + MAX_WAIT;
    loop {
        {
            let mut slot = state.qwen3_stream_final.lock();
            match slot.as_ref() {
                // Our session's result is ready — consume and return it.
                Some((s, _)) if *s == session => return slot.take().unwrap().1,
                // A newer recording already overwrote our result — give up.
                Some((s, _)) if *s > session => return None,
                // Stale older entry, or nothing yet — keep waiting; our preview
                // task will overwrite the slot with our session's final.
                _ => {}
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Wait for the Gemma windowed-preview task to hand off `session`'s full-length
/// transcript, consuming it. `Some(text)` → use it as the authoritative final
/// (the whole-clip decode would truncate at the ~30 s cap); `None` → unusable or
/// the windowed branch never ran (engine not ready), so fall back to the
/// whole-clip decode. The preview task only writes this slot once, on stop, AFTER
/// folding in the final tail window — so the deadline must cover that tail decode,
/// which on slow Gemma + a long clip can take many seconds. The slot is NOT
/// pre-marked (doing so made this consume a placeholder `None` and skip the real
/// transcript). The common case still resolves quickly; only the rare
/// branch-never-ran case waits out the full deadline.
/// Last-ditch recovery for the Gemma streaming path. When the live preview
/// session produced no final — the idle cold-start case, where the sidecar was
/// idle-unloaded and its model reload finished only after the user already
/// stopped a short utterance — re-run the FULL captured clip through a fresh
/// one-shot streaming session. The sidecar is warm by now, so this is fast, and
/// it stays in streaming mode (no batch `mlx_vlm.server` respawn, which would
/// shutdown() the streaming sidecar and ping-pong modes across recordings).
///
/// Operates on the finalized clip WAV (the same audio the batch path would
/// transcribe) — NOT the live capture buffer, which `stop_recording` already
/// drained via `mem::take` to build this very WAV. Returns the transcript if
/// non-empty, else None (never panics the stop path).
async fn recover_gemma_streaming(
    wav: &[u8],
    model: Option<&str>,
    language: Option<&str>,
) -> Option<String> {
    // decode_wav_to_mono_f32 already downmixes to mono + resamples to 16 kHz —
    // exactly the shape gemma_session_feed expects.
    let pcm16k = match audio_io::decode_wav_to_mono_f32(wav) {
        Ok(p) if !p.is_empty() => p,
        Ok(_) => return None,
        Err(e) => {
            log::error!("[vibeking recover] decode clip wav: {e}");
            return None;
        }
    };

    let (port, sid) = match stt::gemma_session_start(model, language).await {
        Ok(v) => v,
        Err(e) => {
            log::error!("[vibeking recover] gemma session start: {e}");
            return None;
        }
    };
    if let Err(e) = stt::gemma_session_feed(port, &sid, &pcm16k).await {
        log::error!("[vibeking recover] gemma session feed: {e}");
        stt::gemma_session_cancel(port, &sid).await;
        return None;
    }
    match stt::gemma_session_finish(port, &sid).await {
        Ok(t) if !t.trim().is_empty() => {
            log::info!("[vibeking recover] recovered transcript via one-shot session");
            Some(t)
        }
        Ok(_) => None,
        Err(e) => {
            log::error!("[vibeking recover] gemma session finish: {e}");
            None
        }
    }
}

async fn await_gemma_stream_final(state: &state::SharedState, session: u64) -> Option<String> {
    use std::time::{Duration, Instant};
    const MAX_WAIT: Duration = Duration::from_secs(30);
    const POLL: Duration = Duration::from_millis(50);
    let deadline = Instant::now() + MAX_WAIT;
    loop {
        {
            let mut slot = state.gemma_stream_final.lock();
            match slot.as_ref() {
                Some((s, _)) if *s == session => return slot.take().unwrap().1,
                Some((s, _)) if *s > session => return None,
                _ => {}
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Join a committed transcript prefix with the next fragment. CJK scripts have
/// no inter-word spaces, Latin scripts do — so insert a single space only when
/// both sides of the seam are spaced-script word characters. Idempotent over
/// existing whitespace/punctuation at the seam.
fn join_transcript(committed: &str, next: &str) -> String {
    let a = committed.trim_end();
    let b = next.trim_start();
    if a.is_empty() {
        return b.to_string();
    }
    if b.is_empty() {
        return a.to_string();
    }
    let needs_space = wants_space_boundary(a.chars().next_back().unwrap())
        && wants_space_boundary(b.chars().next().unwrap());
    if needs_space {
        format!("{a} {b}")
    } else {
        format!("{a}{b}")
    }
}

/// Whether a char belongs to a space-separated script (Latin, Cyrillic, …) AND
/// is a word char — i.e. the seam beside it may need a space. CJK/Kana/Hangul
/// and any punctuation return false (those seams take no extra space).
fn wants_space_boundary(c: char) -> bool {
    if !c.is_alphanumeric() {
        return false;
    }
    let cjk = matches!(c as u32,
        0x3000..=0x303F   // CJK symbols & punctuation
        | 0x3040..=0x30FF // Hiragana + Katakana
        | 0x3400..=0x4DBF // CJK Ext-A
        | 0x4E00..=0x9FFF // CJK Unified Ideographs
        | 0xAC00..=0xD7AF // Hangul syllables
        | 0xF900..=0xFAFF // CJK compatibility ideographs
        | 0xFF00..=0xFFEF // fullwidth / halfwidth forms
    );
    !cjk
}

/// Frozen-head / live-tail stitcher for the Parakeet streaming preview.
///
/// Each tick the caller re-decodes a short TRAILING window of audio (the last
/// ~2 s) and feeds the resulting time-stamped tokens here. Tokens whose audio
/// has fully settled — older than `keep_sec` behind the live edge, so they had
/// full right-context when decoded — are frozen into `committed_raw` and never
/// revisited. The most recent `keep_sec` of audio stays the "tail": it is
/// re-decoded every tick, so it self-corrects word-by-word, but only there —
/// the head never re-words. This is the behaviour a windowed batch re-decode
/// (whole-buffer churn) and the SlidingWindow streamer (commit-once-per-window,
/// no self-correction) both fail to give.
///
/// `committed_raw` accumulates RAW SentencePiece pieces (with the `▁` word-mark)
/// so seams between frozen chunks keep their spacing; the `▁`→space conversion
/// happens once at render time.
struct TailStitcher {
    committed_raw: String,
    /// Audio time (s) the frozen head covers. Monotonic — only advances.
    head_time: f32,
}

impl TailStitcher {
    fn new() -> Self {
        Self {
            committed_raw: String::new(),
            head_time: 0.0,
        }
    }

    /// Feed the tokens of a fresh decode of `audio[window_start_sec .. ]`, where
    /// each token's `start`/`end` are relative to `window_start_sec`. `total_sec`
    /// is the full captured length so far. Three audio zones by end-time:
    ///   * older than `total - keep_sec` → FROZEN into the head (full context).
    ///   * `total - keep_sec .. total - lag_sec` → shown self-correcting tail.
    ///   * newer than `total - lag_sec` → HIDDEN bleeding edge (too fresh to be
    ///     stable; it flips between candidates each tick). Holding it back gives
    ///     every shown word a little right-context, so it doesn't visibly flap.
    /// Returns the text to display.
    fn update(
        &mut self,
        window_start_sec: f32,
        tokens: &[local_parakeet::PreviewToken],
        total_sec: f32,
        keep_sec: f32,
        lag_sec: f32,
    ) -> String {
        let commit_before = total_sec - keep_sec;
        let display_before = total_sec - lag_sec;
        let mut volatile_raw = String::new();
        for tok in tokens {
            let abs_start = window_start_sec + tok.start;
            let abs_end = window_start_sec + tok.end;
            // Tokens centred inside the already-frozen head are the re-decoded
            // overlap — skip them (the head owns that audio).
            if (abs_start + abs_end) * 0.5 <= self.head_time {
                continue;
            }
            if abs_end <= commit_before {
                // Settled: full right-context behind it → freeze permanently.
                self.committed_raw.push_str(&tok.token);
                if abs_end > self.head_time {
                    self.head_time = abs_end;
                }
            } else if abs_end <= display_before {
                // Recent but old enough to be stable → self-correcting tail.
                volatile_raw.push_str(&tok.token);
            }
            // else: bleeding edge, too fresh — hidden this tick.
        }
        Self::render(&self.committed_raw, &volatile_raw)
    }

    /// Reconstruct display text from the frozen head + current tail: concatenate
    /// raw pieces, turn `▁` into spaces, trim, and clean up Parakeet's onset/tail
    /// punctuation quirks. A preview decode has no VAD-calibration leading
    /// silence, so the model sometimes prepends a stray symbol ("∴", ". ", "。"…);
    /// we strip ALL leading non-word characters (stop at the first letter or
    /// digit — CJK ideographs count as letters, so real text is preserved), plus
    /// a single dangling trailing sentence mark ("fort." → "fort").
    fn render(committed_raw: &str, volatile_raw: &str) -> String {
        let mut s = String::with_capacity(committed_raw.len() + volatile_raw.len());
        s.push_str(committed_raw);
        s.push_str(volatile_raw);
        let text = s.replace('\u{2581}', " ");
        let trimmed = text
            .trim()
            .trim_start_matches(|c: char| !c.is_alphanumeric());
        const TRAIL_STRIP: &[char] = &[
            '.', ',', '!', '?', ';', ':', '。', '，', '、', '！', '？', '；', '：',
        ];
        let trimmed = match trimmed.chars().next_back() {
            Some(c) if TRAIL_STRIP.contains(&c) => {
                trimmed[..trimmed.len() - c.len_utf8()].trim_end()
            }
            _ => trimmed,
        };
        trimmed.to_string()
    }
}

/// True when the trailing `tail_ms` of an interleaved buffer sits below the
/// voice-activity floor — i.e. the speaker has paused. The windowed 12B preview
/// commits the live window at such a boundary so a word is never split.
fn tail_is_silent(samples: &[f32], rate: u32, channels: u16, tail_ms: u64) -> bool {
    if rate == 0 || samples.is_empty() {
        return false;
    }
    let frame = channels.max(1) as usize;
    let tail_len = ((rate as u64 * tail_ms / 1000) as usize * frame).max(frame);
    let start = samples.len().saturating_sub(tail_len);
    preview_rms(&samples[start..]) < PREVIEW_MIN_RMS
}

/// Voice-activity check robust to steady ambient noise — the gate before any
/// Gemma-12B audio pass. 12B is a chat LLM: fed near-silence it hallucinates a
/// conversational reply ("how can I help you today") instead of emitting
/// nothing, so a flat whole-buffer RMS floor (which a fan / mic auto-gain / room
/// hum can clear) isn't enough. Instead split into ~30 ms frames and require a
/// handful whose RMS clears a *speech-energy* bar: steady hum nudges every frame
/// up slightly but clears the bar in none, a stray click clears it in only one
/// or two, and real speech clears it in many. Returns true only on the last.
fn has_speech(samples: &[f32], rate: u32, channels: u16) -> bool {
    if rate == 0 || samples.is_empty() {
        return false;
    }
    const FRAME_MS: usize = 30;
    /// Speech frames sit ~0.02–0.1 RMS; quiet-room hum stays well under this.
    const VOICED_RMS: f32 = 0.02;
    /// ≥~120 ms of voiced frames, so a lone transient (click, key) can't trip it.
    const MIN_VOICED_FRAMES: usize = 4;
    let frame = (channels.max(1) as usize).max(1);
    let win = (rate as usize * FRAME_MS / 1000 * frame).max(frame);
    let mut voiced = 0usize;
    let mut i = 0usize;
    while i + win <= samples.len() {
        if preview_rms(&samples[i..i + win]) >= VOICED_RMS {
            voiced += 1;
            if voiced >= MIN_VOICED_FRAMES {
                return true;
            }
        }
        i += win;
    }
    false
}

/// Length of `samples` with trailing silence removed (plus a short keep-alive
/// margin after the last real signal). Feeding 12B a window that ends in silence
/// invites it to "run on" past the speech into confabulation/refusal — it reads
/// the pause as "your turn is over, now I reply" — so we cut the decode window to
/// the last speech before encoding. Returns the full length when the tail is
/// already speech or the whole buffer is silent (the caller's VAD gates that).
fn trim_trailing_silence_len(samples: &[f32], rate: u32, channels: u16) -> usize {
    if samples.is_empty() || rate == 0 {
        return samples.len();
    }
    const THRESHOLD: f32 = 0.003;
    let frame = channels.max(1) as usize;
    let margin = (rate as usize * frame) / 10; // ~100 ms after the last word
    match samples.iter().rposition(|s| s.abs() > THRESHOLD) {
        Some(last) => (last + 1 + margin).min(samples.len()),
        None => samples.len(),
    }
}

/// Encode interleaved f32 samples to a 16-bit PCM WAV blob, used by the live
/// preview to re-transcribe the in-progress recording. Native rate/channels —
/// the engines' `decode_wav_to_mono_f32` downmixes + resamples to 16 kHz.
/// Mirrors `audio::encode_wav` (which is private to that module).
fn encode_wav_bytes(samples: &[f32], sample_rate: u32, channels: u16) -> Vec<u8> {
    use std::io::Cursor;
    let spec = hound::WavSpec {
        channels: channels.max(1),
        sample_rate: if sample_rate == 0 {
            16_000
        } else {
            sample_rate
        },
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut buf = Cursor::new(Vec::with_capacity(samples.len() * 2 + 44));
    {
        let mut writer = match hound::WavWriter::new(&mut buf, spec) {
            Ok(w) => w,
            Err(_) => return Vec::new(),
        };
        for &s in samples {
            let _ = writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
        }
        let _ = writer.finalize();
    }
    buf.into_inner()
}

/// Runs the active refinement mode against `raw` and returns
/// `(final_text, polished, translated)`. The two booleans drive the chip
/// bar's "Vibing" vs "Translating" badge in the result phase. When the
/// active mode is `None` (Off), or the LLM provider isn't configured, or
/// the LLM call fails, falls back to the raw transcript with both flags
/// false — refinement is an enhancement, never blocks the insert path.
async fn run_refinement<R: tauri::Runtime>(
    raw: &str,
    settings: &state::Settings,
    handle: &tauri::AppHandle<R>,
    session: u64,
) -> (String, bool, bool) {
    let Some(active_id) = settings.active_refinement_mode_id.as_deref() else {
        return (raw.to_string(), false, false);
    };
    let Some(mode_def) = settings.refinement_modes.iter().find(|m| m.id == active_id) else {
        log::info!(
            "[vibeking] active_refinement_mode_id={active_id} not found in refinement_modes; skipping refinement"
        );
        return (raw.to_string(), false, false);
    };

    let configured = match settings.polish_provider {
        polish::PolishProvider::Anthropic => !settings.anthropic_key.is_empty(),
        _ => !settings.polish_base_url.is_empty() && !settings.polish_model.is_empty(),
    };
    if !configured {
        return (raw.to_string(), false, false);
    }

    let config = polish::PolishConfig {
        provider: settings.polish_provider,
        anthropic_key: &settings.anthropic_key,
        base_url: &settings.polish_base_url,
        model: &settings.polish_model,
        api_key: &settings.polish_api_key,
        prompt: &mode_def.prompt,
    };

    // The `translate` id is special-cased so the existing {target}
    // placeholder substitution still happens. Every other id (built-in or
    // user-created) runs as Polish mode — the prompt itself carries the
    // intent (Email, Notes, Code prompt, custom user modes, ...).
    let is_translate = active_id == "translate";

    if is_translate {
        let _ = handle.emit(
            "transcription:translating",
            serde_json::json!({ "raw": raw, "session": session }),
        );
        match polish::run(
            raw,
            &settings.hotwords,
            &settings.correction_dictionary,
            config.clone(),
            polish::Mode::Translate {
                target_language: &settings.translate_target,
            },
        )
        .await
        {
            // Never let refinement turn a real transcript into nothing — a
            // thinking/reasoning model can reply all-`<think>` (or get truncated
            // mid-thought), which `strip_thinking` collapses to "". Falling
            // through keeps the raw transcript so the user's words still land.
            Ok(t) if !t.trim().is_empty() => return (t, false, true),
            Ok(_) => log::warn!("[vibeking] translate returned empty — keeping raw transcript"),
            Err(e) => log::error!("[vibeking] translate failed: {e}"),
        }
    } else {
        let _ = handle.emit(
            "transcription:polishing",
            serde_json::json!({ "raw": raw, "session": session }),
        );
        match polish::run(
            raw,
            &settings.hotwords,
            &settings.correction_dictionary,
            config.clone(),
            polish::Mode::Polish,
        )
        .await
        {
            // See note above — an empty polish result must not erase the
            // transcript; fall through to keep the raw text.
            Ok(t) if !t.trim().is_empty() => return (t, true, false),
            Ok(_) => log::warn!(
                "[vibeking] refinement '{active_id}' returned empty — keeping raw transcript"
            ),
            Err(e) => log::error!("[vibeking] refinement '{active_id}' failed: {e}"),
        }
    }
    (raw.to_string(), false, false)
}

#[cfg(test)]
mod preview_join_tests {
    use super::{
        has_speech, is_chat_refusal, join_transcript, preview_rms, tail_is_silent,
        trim_trailing_silence_len, wants_space_boundary, TailStitcher, PREVIEW_MIN_RMS,
    };
    use crate::local_parakeet::PreviewToken;

    fn tok(token: &str, start: f32, end: f32) -> PreviewToken {
        PreviewToken {
            token: token.to_string(),
            start,
            end,
        }
    }

    #[test]
    fn freezes_settled_head_keeps_recent_tail() {
        // Whole utterance decoded in one window. With keep=1.2 s and a 4.0 s
        // total, the word ending at 3.5 s ("how") is still in the live tail,
        // while everything ending before 2.8 s is frozen.
        let mut s = TailStitcher::new();
        let tokens = [
            tok("▁hello", 0.0, 0.5),
            tok("▁world", 0.6, 1.1),
            tok("▁how", 3.0, 3.5),
        ];
        let display = s.update(0.0, &tokens, 4.0, 1.2, 0.0);
        assert_eq!(display, "hello world how");
        // "hello world" settled (ends ≤ 2.8); "how" (ends 3.5) is tail, not frozen.
        assert!(s.committed_raw.contains("hello"));
        assert!(s.committed_raw.contains("world"));
        assert!(!s.committed_raw.contains("how"));
    }

    #[test]
    fn frozen_head_survives_a_tail_reword() {
        // Tick 1: "I have to" — "to" is in the tail (recent). Tick 2: re-decode of
        // the trailing window revises it to "two" and adds "cats". The head ("I
        // have") stays; only the tail self-corrects.
        let mut s = TailStitcher::new();
        let d1 = s.update(
            0.0,
            &[
                tok("▁I", 0.0, 0.3),
                tok("▁have", 0.4, 0.8),
                tok("▁to", 0.9, 1.2),
            ],
            1.4,
            1.2,
            0.0,
        );
        assert_eq!(d1, "I have to");
        // Next tick: window re-decoded from head-overlap; "two" replaces "to".
        let d2 = s.update(
            0.0,
            &[
                tok("▁I", 0.0, 0.3),
                tok("▁have", 0.4, 0.8),
                tok("▁two", 0.9, 1.3),
                tok("▁cats", 1.4, 1.9),
            ],
            2.1,
            1.2,
            0.0,
        );
        assert_eq!(d2, "I have two cats");
        // "I have" froze (ended ≤ total-keep on tick 2); "to" never did.
        assert!(!s.committed_raw.contains("to"));
    }

    #[test]
    fn overlap_tokens_are_not_duplicated() {
        // Tick 1 freezes "hello" (ends 0.5, total 2.0, keep 1.2 → commit ≤ 0.8).
        let mut s = TailStitcher::new();
        let d1 = s.update(
            0.0,
            &[tok("▁hello", 0.0, 0.5), tok("▁there", 1.0, 1.5)],
            2.0,
            1.2,
            0.0,
        );
        assert_eq!(d1, "hello there");
        assert!((s.head_time - 0.5).abs() < 1e-3);
        // Tick 2 re-decodes from window_start 0.0 (head 0.5 - overlap 1.0 → 0.0),
        // so "hello" appears again in the tokens — it must NOT be re-appended.
        let d2 = s.update(
            0.0,
            &[
                tok("▁hello", 0.0, 0.5),
                tok("▁there", 1.0, 1.5),
                tok("▁friend", 1.6, 2.1),
            ],
            2.6,
            1.2,
            0.0,
        );
        assert_eq!(d2, "hello there friend");
        assert_eq!(s.committed_raw.matches("hello").count(), 1);
    }

    #[test]
    fn cjk_tokens_render_without_spaces() {
        let mut s = TailStitcher::new();
        let display = s.update(
            0.0,
            &[
                tok("今", 0.0, 0.3),
                tok("天", 0.4, 0.7),
                tok("好", 2.0, 2.3),
            ],
            3.0,
            1.2,
            0.0,
        );
        assert_eq!(display, "今天好");
    }

    #[test]
    fn lag_hides_bleeding_edge_and_strips_trailing_punct() {
        // total=3.0, keep=1.2 (commit ≤1.8), lag=0.4 (display ≤2.6). "going"
        // ends 2.4 → shown tail; "fort." ends 2.9 → bleeding edge, hidden. The
        // trailing "." that Parakeet dangles is also stripped if it slips in.
        let mut s = TailStitcher::new();
        let display = s.update(
            0.0,
            &[
                tok("▁we", 0.0, 0.4),
                tok("▁are", 0.5, 0.9),
                tok("▁going", 2.0, 2.4),
                tok("▁fort.", 2.5, 2.9),
            ],
            3.0,
            1.2,
            0.4,
        );
        assert_eq!(display, "we are going");
        // "fort." (the unstable freshest token) is held back, not shown.
        assert!(!display.contains("fort"));
    }

    #[test]
    fn strips_stray_leading_onset_symbol() {
        // No VAD-calibration silence in the preview decode → Parakeet prepends a
        // stray "∴" symbol token. It must not pollute the display.
        let mut s = TailStitcher::new();
        let display = s.update(
            0.0,
            &[tok("▁∴", 0.0, 0.1), tok("▁Yes", 0.2, 0.6)],
            0.6,
            1.2,
            0.0,
        );
        assert_eq!(display, "Yes");
    }

    #[test]
    fn latin_seam_gets_one_space() {
        assert_eq!(
            join_transcript("hello world", "this is"),
            "hello world this is"
        );
        // empty prefix → bare next; empty next → bare prefix
        assert_eq!(join_transcript("", "first words"), "first words");
        assert_eq!(join_transcript("already here", ""), "already here");
    }

    #[test]
    fn does_not_double_space_or_break_punctuation() {
        // existing whitespace at the seam is normalized to a single space
        assert_eq!(join_transcript("hello ", "  world"), "hello world");
        // punctuation on either side takes no extra space
        assert_eq!(join_transcript("end.", "Next"), "end.Next");
        assert_eq!(join_transcript("wait", ", really"), "wait, really");
    }

    #[test]
    fn cjk_seam_has_no_space() {
        // Simplified + Traditional: never insert a Latin space between glyphs.
        assert_eq!(join_transcript("你好", "世界"), "你好世界");
        assert_eq!(join_transcript("今天天氣", "很好"), "今天天氣很好");
        // mixed: CJK on one side of the seam suppresses the space
        assert_eq!(join_transcript("开发 app", "继续"), "开发 app继续");
    }

    #[test]
    fn space_boundary_classifies_scripts() {
        assert!(wants_space_boundary('a'));
        assert!(wants_space_boundary('Z'));
        assert!(wants_space_boundary('7'));
        assert!(!wants_space_boundary('你')); // CJK
        assert!(!wants_space_boundary('。')); // CJK punctuation
        assert!(!wants_space_boundary(',')); // ASCII punctuation
        assert!(!wants_space_boundary(' '));
    }

    #[test]
    fn tail_silence_detects_quiet_vs_loud_tail() {
        let rate = 16_000u32;
        // 1s of near-silence then 1s of loud tone → tail (last 450ms) is loud.
        let mut loud_tail = vec![0.0f32; rate as usize];
        loud_tail.extend(std::iter::repeat(0.3).take(rate as usize));
        assert!(!tail_is_silent(&loud_tail, rate, 1, 450));
        // loud start then a quiet tail → tail is silent.
        let mut quiet_tail = vec![0.3f32; rate as usize];
        quiet_tail.extend(std::iter::repeat(0.0).take(rate as usize));
        assert!(tail_is_silent(&quiet_tail, rate, 1, 450));
    }

    #[test]
    fn is_chat_refusal_catches_variants_without_request_suffix() {
        assert!(is_chat_refusal("I cannot fulfill this")); // no "request"
        assert!(is_chat_refusal("I can't fulfil this")); // single-l
        assert!(is_chat_refusal("Please provide the audio file"));
        assert!(!is_chat_refusal("I will fulfill my testing duties")); // genuine speech
    }

    #[test]
    fn trim_trailing_silence_cuts_quiet_tail_keeps_speech() {
        let rate = 16_000u32;
        // 0.5s speech then 1s silence → trimmed length ≈ speech + ~100ms margin.
        let mut s = vec![0.3f32; rate as usize / 2];
        s.extend(std::iter::repeat(0.0).take(rate as usize));
        let len = trim_trailing_silence_len(&s, rate, 1);
        assert!(len >= rate as usize / 2 && len < s.len());
        // All-speech → keep everything.
        let loud = vec![0.3f32; rate as usize];
        assert_eq!(trim_trailing_silence_len(&loud, rate, 1), loud.len());
    }

    #[test]
    fn has_speech_rejects_silence_and_steady_hum_accepts_speech() {
        let rate = 16_000u32;
        // Pure silence → no speech.
        assert!(!has_speech(&vec![0.0f32; rate as usize], rate, 1));
        // Steady low-level hum below the speech bar → no speech (the bug: this
        // used to clear a flat 0.006 RMS floor and trigger a 12B hallucination).
        let hum: Vec<f32> = (0..rate)
            .map(|i| 0.008 * ((i as f32) * 0.3).sin())
            .collect();
        assert!(!has_speech(&hum, rate, 1));
        // A lone loud transient (one ~30 ms click) is not enough voiced frames.
        let mut click = vec![0.0f32; rate as usize];
        for s in click.iter_mut().take((rate as usize * 30 / 1000) / 2) {
            *s = 0.5;
        }
        assert!(!has_speech(&click, rate, 1));
        // Sustained speech-level energy → speech.
        let speech: Vec<f32> = (0..rate).map(|i| 0.06 * ((i as f32) * 0.2).sin()).collect();
        assert!(has_speech(&speech, rate, 1));
    }

    // Regression: a quiet-but-real mic (e.g. RØDE VideoMic NTG, raw peak ~0.05–0.09,
    // whole-buffer RMS ~0.004) used to leave the live preview permanently BLANK,
    // because the universal preview loop gated on an ABSOLUTE RMS floor
    // (`preview_rms < PREVIEW_MIN_RMS`, 0.006) that sits ABOVE such a mic's RMS — so
    // every pass was skipped and `transcribe` was never called, while the batch
    // path (which gain-normalizes first) decoded fine. The fix gates on the
    // GAIN-INVARIANT `capture_has_voice` instead. This pins both halves so the
    // absolute floor can't sneak back in.
    #[test]
    fn quiet_mic_speech_passes_voice_gate_but_fails_old_rms_floor() {
        let rate = 16_000u32;
        // ~80 ms voiced bursts (quiet 0.011 sine) separated by ~120 ms near-silent
        // room tone → real speech dynamic range, but a whole-buffer RMS below the
        // old 0.006 floor, mirroring the logged capture (peak ~0.086, rms ~0.0044).
        let mut s = vec![0.0008f32; rate as usize];
        let burst = rate as usize * 80 / 1000;
        let gap = rate as usize * 120 / 1000;
        let mut i = 0usize;
        while i + burst <= s.len() {
            for k in 0..burst {
                s[i + k] = 0.011 * ((i + k) as f32 * 0.25).sin();
            }
            i += burst + gap;
        }
        // The OLD gate would reject this as "silence" → no preview pass ever runs.
        assert!(
            preview_rms(&s) < PREVIEW_MIN_RMS,
            "test signal must sit under the old floor it exposed (got {})",
            preview_rms(&s),
        );
        // The NEW (gain-invariant) gate correctly sees real speech → preview runs.
        assert!(
            crate::audio::capture_has_voice(&s, rate, 1),
            "gain-invariant gate must accept quiet-but-real speech",
        );
        // And it still rejects genuine silence (no false preview / hallucination).
        assert!(!crate::audio::capture_has_voice(&vec![0.0008f32; rate as usize], rate, 1));
    }
}
