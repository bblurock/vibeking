//! Microphone capture for Vibeking.
//!
//! Single long-lived audio thread that opens the cpal stream on first
//! recording and keeps it warm for a few seconds afterwards. Subsequent
//! recordings within the keepalive window reuse the running stream and
//! pay zero cold-start cost. After the idle timeout the stream closes
//! and the OS-level "this app is using your mic" indicator turns off, so
//! we're not always-listening when idle.
//!
//! cpal's Stream is `!Send` on macOS so the stream itself stays on the
//! audio thread; the rest of the app communicates with the thread via
//! an mpsc channel of commands.

use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{SampleFormat, WavSpec, WavWriter};
use parking_lot::Mutex;
use serde::Serialize;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::Emitter;

#[derive(Debug, Clone)]
pub struct RecordedClip {
    pub wav: Vec<u8>,
    pub duration_ms: u64,
    /// Peak absolute sample value in the clip (0.0–1.0). Surfaced so the
    /// caller can detect silent recordings — typically caused by a
    /// hotkey misfire (e.g. the user inadvertently entered Toggle mode
    /// via a fast press-release, then "started a new recording" which
    /// actually stopped the still-empty toggle one) — and skip the
    /// transcribe pipeline for them rather than pasting garbage.
    pub peak: f32,
    /// Recording length BEFORE leading-silence trim, in ms. Lets the caller
    /// tell a genuine hotkey misfire (tiny raw clip → hide quietly) apart from
    /// a real recording that captured silence (long raw clip but silent →
    /// surface a "check your mic" error instead of silently dropping it).
    pub raw_duration_ms: u64,
    /// The UNTRIMMED audio, populated only when leading-silence trimming ate
    /// almost the entire take (a substantial raw recording reduced to ~nothing).
    /// That pattern means either real-but-quiet speech the trim threshold
    /// over-ate, or a cold-start where only a tail was captured — so rather than
    /// discard the recording, the stop handler retries transcription on this raw
    /// clip "if available". `None` when the trimmed clip is already usable.
    pub recovery_wav: Option<Vec<u8>>,
    /// True when the gain-invariant voice detector found real speech in the RAW
    /// capture, regardless of how quiet it was. The stop handler keys its
    /// keep/drop decision on THIS rather than an absolute peak threshold, so a
    /// quiet-but-real take (low-gain laptop mic) the leading-silence trim
    /// over-ate is recovered instead of dropped as "silent".
    pub raw_has_voice: bool,
}

/// Outcome of resolving the user's preferred mic against cpal's current
/// device enumeration. When the saved device isn't connected (AirPods
/// disconnected, USB mic unplugged) we silently fall back to the system
/// default; the chip bar surfaces the swap so the user isn't recording
/// into a wrong device without knowing.
#[derive(Debug, Clone, Serialize)]
pub struct MicResolution {
    /// What the user picked in settings. `None` means they explicitly
    /// chose "System default" — no fallback is possible in that case.
    pub requested: Option<String>,
    /// The device actually opened. `None` if cpal had no input devices
    /// at all (no mic connected, permission denied, etc.).
    pub actual: Option<String>,
    /// True when `requested` was a specific device but it couldn't be
    /// found and we fell back to the system default.
    pub fell_back: bool,
}

/// Process-lifetime handle the rest of the app uses to drive recording.
///
/// Internally tracks the live audio thread's command channel. If the
/// thread has shut down (idle timeout, error), `start_recording` will
/// spawn a fresh one transparently. Cloning the `AudioEngine` value is
/// cheap — it's just an Arc<Mutex<...>>.
pub struct AudioEngine {
    cmd: Mutex<Option<mpsc::Sender<AudioCmd>>>,
    shared: Arc<SharedCapture>,
}

/// Capture state shared between the audio thread and live readers. The cpal
/// callback appends to `samples` while `recording` is set; the Qwen3 streaming
/// tap reads new samples incrementally via [`AudioEngine::read_samples_from`]
/// without disturbing the buffer the Stop handler later encodes to WAV.
struct SharedCapture {
    samples: Mutex<Vec<f32>>,
    recording: AtomicBool,
    /// Monotonic "stream is alive" heartbeat: the cpal callback bumps this once
    /// for every callback buffer that carried real audio (any sample above
    /// [`STREAM_ALIVE_FLOOR`] — i.e. above digital silence, which a working mic's
    /// ambient noise floor clears even when the user isn't speaking). It keeps
    /// advancing while the device delivers signal and *stops* only when the
    /// stream goes truly dead (muted / permission-denied / degraded → digital
    /// silence). The audio thread watches its rate of change to rebuild a dead
    /// stream mid-recording (repeatedly, not just once); `lib.rs` polls it to
    /// surface a "check your mic" warning. Crucially this is liveness, not speech
    /// — a silent pause keeps it ticking, so we never false-warn. Reset to 0 at
    /// the start of each recording.
    live_seq: AtomicU64,
    /// (sample_rate, channels) of the currently-open stream; (0, 0) if none.
    meta: Mutex<(u32, u16)>,
    /// Live-meter mode. When set (independent of `recording`), the cpal
    /// callback computes the per-buffer RMS and publishes it to `level` so
    /// the onboarding mic step can show a "speak to verify" meter without
    /// running the full recording/transcribe pipeline. Off by default.
    monitoring: AtomicBool,
    /// Latest per-buffer RMS (f32 bits, 0.0 when not monitoring). Read by the
    /// emit task in `start_mic_monitor` and pushed to the frontend as
    /// `audio:level`.
    level: AtomicU32,
}

impl SharedCapture {
    fn set_level(&self, v: f32) {
        self.level.store(v.to_bits(), Ordering::Relaxed);
    }
    fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

/// Cheap, cloneable read handle into the live meter. Handed to the
/// `start_mic_monitor` emit task so it can poll the level / monitoring flag
/// without holding the `AudioEngine` (which isn't `Clone`).
#[derive(Clone)]
pub struct MonitorTap(Arc<SharedCapture>);

impl MonitorTap {
    /// True while monitoring is active. The emit task loops on this.
    pub fn active(&self) -> bool {
        self.0.monitoring.load(Ordering::Relaxed)
    }
    /// Latest RMS (0.0–~1.0).
    pub fn level(&self) -> f32 {
        self.0.level()
    }
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self {
            cmd: Mutex::new(None),
            shared: Arc::new(SharedCapture {
                samples: Mutex::new(Vec::with_capacity(48_000 * 8)),
                recording: AtomicBool::new(false),
                live_seq: AtomicU64::new(0),
                meta: Mutex::new((0, 0)),
                monitoring: AtomicBool::new(false),
                level: AtomicU32::new(0),
            }),
        }
    }
}

enum AudioCmd {
    Start {
        device_name: Option<String>,
        ready_tx: mpsc::Sender<Result<MicResolution, String>>,
    },
    Stop {
        clip_tx: mpsc::Sender<Result<RecordedClip>>,
    },
    Cancel,
    /// Open (or reuse) a stream on `device_name` purely to publish live RMS
    /// levels for the onboarding mic meter. Does NOT set `recording`, so the
    /// samples buffer is never filled and the transcribe pipeline is untouched.
    StartMonitor {
        device_name: Option<String>,
    },
    /// Stop publishing levels. The stream is left warm; the idle watchdog
    /// closes it after `IDLE_TIMEOUT`, turning the mic indicator off.
    StopMonitor,
}

/// How long the cpal stream stays open after the last recording ends.
/// During this window the mic indicator stays on, but a new recording
/// starts with zero cold-start cost. After the timeout, the stream
/// closes and the indicator turns off.
const IDLE_TIMEOUT: Duration = Duration::from_secs(5);

/// A sample magnitude above this counts as "the capture pipeline is alive" for
/// the dead-stream watchdog. Deliberately set BELOW a room's ambient noise floor
/// (and far below speech) so a working mic keeps the liveness heartbeat ticking
/// even while the user is silent/thinking — only a genuinely dead stream
/// (digital silence: exact 0.0 / denormals, as a muted, permission-denied, or
/// degraded macOS audio unit delivers) ever stalls it. This is liveness, NOT
/// speech detection: we must never flag "check your mic" just because someone
/// paused. (Leading-silence trimming + the silent-clip gate use their own,
/// higher thresholds — see `leading_silence_index` / `SILENT_PEAK`.)
const STREAM_ALIVE_FLOOR: f32 = 0.0003;

impl AudioEngine {
    pub fn start_recording(&self, device_name: Option<String>) -> Result<MicResolution> {
        let cmd_tx = self.ensure_thread()?;
        let (ready_tx, ready_rx) = mpsc::channel();
        if cmd_tx
            .send(AudioCmd::Start {
                device_name: device_name.clone(),
                ready_tx,
            })
            .is_err()
        {
            // Thread shut down between ensure_thread and send; respawn once.
            self.clear_thread();
            let cmd_tx = self.ensure_thread()?;
            let (ready_tx, ready_rx) = mpsc::channel();
            cmd_tx
                .send(AudioCmd::Start {
                    device_name,
                    ready_tx,
                })
                .map_err(|e| anyhow!("audio thread send failed: {e}"))?;
            return finalize_start(ready_rx);
        }
        finalize_start(ready_rx)
    }

    pub fn stop_recording(&self) -> Result<RecordedClip> {
        let cmd_tx = self
            .cmd
            .lock()
            .clone()
            .ok_or_else(|| anyhow!("no audio thread running"))?;
        let (clip_tx, clip_rx) = mpsc::channel();
        cmd_tx
            .send(AudioCmd::Stop { clip_tx })
            .map_err(|e| anyhow!("audio thread send failed: {e}"))?;
        clip_rx
            .recv()
            .map_err(|e| anyhow!("audio thread dropped before clip: {e}"))?
    }

    pub fn cancel_recording(&self) {
        if let Some(cmd_tx) = self.cmd.lock().clone() {
            let _ = cmd_tx.send(AudioCmd::Cancel);
        }
    }

    /// Begin publishing live mic RMS for the onboarding meter. Sets the
    /// monitoring flag synchronously (so the cpal callback starts computing
    /// levels the moment the stream is open) and asks the audio thread to open
    /// the requested device. Idempotent: calling again with a different device
    /// reopens on it. Independent of `start_recording` — no buffer fills, no
    /// chip bar, no transcribe.
    pub fn start_monitor(&self, device_name: Option<String>) -> Result<()> {
        self.shared.monitoring.store(true, Ordering::Relaxed);
        let cmd_tx = self.ensure_thread()?;
        if cmd_tx
            .send(AudioCmd::StartMonitor {
                device_name: device_name.clone(),
            })
            .is_err()
        {
            // Thread shut down between ensure_thread and send; respawn once.
            self.clear_thread();
            let cmd_tx = self.ensure_thread()?;
            cmd_tx
                .send(AudioCmd::StartMonitor { device_name })
                .map_err(|e| anyhow!("audio thread send failed: {e}"))?;
        }
        Ok(())
    }

    /// Stop publishing live levels and zero the meter. The stream is left warm
    /// and closed by the idle watchdog shortly after.
    pub fn stop_monitor(&self) {
        self.shared.monitoring.store(false, Ordering::Relaxed);
        self.shared.set_level(0.0);
        if let Some(cmd_tx) = self.cmd.lock().clone() {
            let _ = cmd_tx.send(AudioCmd::StopMonitor);
        }
    }

    /// True while the live meter is active.
    pub fn is_monitoring(&self) -> bool {
        self.shared.monitoring.load(Ordering::Relaxed)
    }

    /// A cheap clone handle the emit task uses to poll the live level.
    pub fn monitor_tap(&self) -> MonitorTap {
        MonitorTap(self.shared.clone())
    }

    /// True while a recording is in progress (cpal is appending samples).
    /// The Qwen3 streaming tap loops on this to know when to stop feeding.
    pub fn is_recording(&self) -> bool {
        self.shared.recording.load(Ordering::Relaxed)
    }

    /// "Stream is alive" heartbeat — advances each time the mic delivers a
    /// callback buffer carrying real audio (above the noise floor) during the
    /// current recording (reset to 0 at Start). A `lib.rs` watcher polls this: if
    /// it stops advancing while recording, the capture pipeline (not merely the
    /// speaker) has gone silent — a dead/muted device — and the user is warned.
    /// A normal speech pause keeps it ticking via ambient noise, so a thinking
    /// user is never warned. See [`SharedCapture::live_seq`].
    pub fn live_seq(&self) -> u64 {
        self.shared.live_seq.load(Ordering::Relaxed)
    }

    /// (sample_rate, channels) of the live stream; (0, 0) if none open.
    pub fn capture_meta(&self) -> (u32, u16) {
        *self.shared.meta.lock()
    }

    /// Read capture samples produced since `cursor` (an index into the raw,
    /// interleaved, native-rate buffer). Returns `(new_samples, new_cursor)`.
    /// Read-only: does not drain the buffer the Stop handler encodes to WAV,
    /// so the live streaming preview and the authoritative final transcript
    /// see the same audio.
    pub fn read_samples_from(&self, cursor: usize) -> (Vec<f32>, usize) {
        let guard = self.shared.samples.lock();
        let len = guard.len();
        if cursor >= len {
            return (Vec::new(), len);
        }
        (guard[cursor..].to_vec(), len)
    }

    fn ensure_thread(&self) -> Result<mpsc::Sender<AudioCmd>> {
        let mut guard = self.cmd.lock();
        if let Some(tx) = guard.as_ref() {
            return Ok(tx.clone());
        }
        let (cmd_tx, cmd_rx) = mpsc::channel::<AudioCmd>();
        let shared = self.shared.clone();
        std::thread::Builder::new()
            .name("vibeking-audio".into())
            .spawn(move || {
                run_audio_thread(cmd_rx, shared);
            })
            .map_err(|e| anyhow!("spawn audio thread: {e}"))?;
        *guard = Some(cmd_tx.clone());
        Ok(cmd_tx)
    }

    fn clear_thread(&self) {
        *self.cmd.lock() = None;
    }
}

fn finalize_start(
    ready_rx: mpsc::Receiver<Result<MicResolution, String>>,
) -> Result<MicResolution> {
    match ready_rx.recv() {
        Ok(Ok(res)) => Ok(res),
        Ok(Err(msg)) => Err(anyhow!("{msg}")),
        Err(_) => Err(anyhow!("audio thread terminated before ready")),
    }
}

/// List available input device names. Errors are surfaced as an empty
/// Vec — the UI falls back to the "System default" sentinel and capture
/// continues to work.
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let Ok(devices) = host.input_devices() else {
        return Vec::new();
    };
    devices.filter_map(|d| d.name().ok()).collect()
}

fn input_device_cache() -> &'static Arc<crate::input_device_cache::InputDeviceCache> {
    static CACHE: std::sync::OnceLock<Arc<crate::input_device_cache::InputDeviceCache>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| Arc::new(crate::input_device_cache::InputDeviceCache::default()))
}

pub fn cached_input_devices() -> Vec<String> {
    input_device_cache().snapshot()
}

pub fn refresh_input_devices(app: &tauri::AppHandle<tauri::Wry>) {
    let app = app.clone();
    if let Err(error) = input_device_cache().refresh(list_input_devices, move |devices| {
        let _ = app.emit("audio:input-devices-changed", devices);
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || crate::tray::maybe_rebuild(&handle));
    }) {
        log::error!("[vibeking] could not start microphone discovery: {error}");
    }
}

#[tauri::command]
pub fn input_devices(app: tauri::AppHandle<tauri::Wry>) -> Vec<String> {
    refresh_input_devices(&app);
    cached_input_devices()
}

/// Live mic-level event payload (`audio:level`). `rms` is the raw per-buffer
/// root-mean-square (≈0.0 silence → ~0.2 for normal speech); the frontend
/// maps it to bar heights so the visual gain stays a UI concern.
#[derive(Clone, Serialize)]
struct AudioLevel {
    rms: f32,
}

/// Start the onboarding mic meter on `device` (None = system default). Opens
/// the stream and spawns a ~25 Hz task that emits `audio:level` until
/// `stop_mic_monitor` is called. Safe to call repeatedly to switch devices —
/// only one emit task runs at a time.
#[tauri::command]
pub fn start_mic_monitor<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, crate::state::SharedState>,
    device: Option<String>,
) -> Result<(), String> {
    let engine = &state.audio_engine;
    let was_active = engine.is_monitoring();
    engine.start_monitor(device).map_err(|e| e.to_string())?;
    // Only one emitter — re-calls (device switches) reuse the running task.
    // Plain OS thread (not the async runtime) so we don't depend on a tokio
    // time context; `AppHandle::emit` is safe to call from any thread.
    if !was_active {
        let tap = engine.monitor_tap();
        std::thread::Builder::new()
            .name("vibeking-mic-meter".into())
            .spawn(move || {
                while tap.active() {
                    let _ = app.emit("audio:level", AudioLevel { rms: tap.level() });
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
                // One final zero so the meter visibly settles when monitoring ends.
                let _ = app.emit("audio:level", AudioLevel { rms: 0.0 });
            })
            .ok();
    }
    Ok(())
}

/// Stop the onboarding mic meter and let the warm stream close.
#[tauri::command]
pub fn stop_mic_monitor(state: tauri::State<'_, crate::state::SharedState>) {
    state.audio_engine.stop_monitor();
}

/// Returns the resolved cpal device plus a flag indicating whether we
/// had to fall back to the system default. Callers use the flag to drive
/// the chip bar's "(unavailable)" hint so the user knows the recording
/// is going into a different device than they configured.
fn pick_input_device(host: &cpal::Host, name: Option<&str>) -> Option<(cpal::Device, bool)> {
    if let Some(name) = name.map(str::trim).filter(|s| !s.is_empty()) {
        if let Ok(devices) = host.input_devices() {
            for d in devices {
                if d.name().map(|n| n == name).unwrap_or(false) {
                    return Some((d, false));
                }
            }
        }
        log::info!("[vibeking] requested input device '{name}' not found; using default");
        return host.default_input_device().map(|d| (d, true));
    }
    host.default_input_device().map(|d| (d, false))
}

struct OpenStream {
    _stream: cpal::Stream,
    sample_rate: u32,
    channels: u16,
    /// The name the caller requested when opening this stream. Used by
    /// the warm-stream reuse check (`needs_reopen`) — if the user
    /// switches their mic preference, we have to reopen even if the
    /// resolved device happens to be the same.
    requested_name: Option<String>,
    /// Resolution outcome reported back to the chip bar on every Start,
    /// including warm reuses. Stored here so we don't have to re-walk
    /// cpal's device list on every keypress.
    resolution: MicResolution,
}

fn run_audio_thread(cmd_rx: mpsc::Receiver<AudioCmd>, shared: Arc<SharedCapture>) {
    let mut open: Option<OpenStream> = None;
    let mut last_activity = Instant::now();
    // Dead-stream watchdog state. We track the audio "heartbeat" (`live_seq`)
    // across poll ticks: while a recording is active, if the heartbeat stops
    // advancing for SILENCE_REBUILD the warm stream has gone dead (delivers only
    // silent callbacks — the intermittent "recorded 42 s, captured nothing" bug)
    // and we rebuild it on the same device. Unlike the old one-shot latch this
    // re-heals as many times as needed (capped per take) and catches a stream
    // that dies *after* the first audible sample, not just a cold one.
    let mut last_live_seq: u64 = 0;
    let mut last_live_at = Instant::now();
    let mut rebuilds_this_recording: u32 = 0;
    const SILENCE_REBUILD: Duration = Duration::from_millis(2500);
    const MAX_REBUILDS_PER_RECORDING: u32 = 3;

    loop {
        // Short poll cadence so the idle watchdog can close the stream
        // promptly once IDLE_TIMEOUT lapses.
        match cmd_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(AudioCmd::Start {
                device_name,
                ready_tx,
            }) => {
                // Reopen if no stream OR the requested device differs
                // from the currently-warm one.
                let needs_reopen = match &open {
                    None => true,
                    Some(s) => s.requested_name != device_name,
                };
                if needs_reopen {
                    open = None;
                    match setup_stream(device_name.as_deref(), shared.clone()) {
                        Ok(o) => open = Some(o),
                        Err(e) => {
                            let _ = ready_tx.send(Err(e.to_string()));
                            continue;
                        }
                    }
                }

                // Publish stream meta so the streaming tap can resample the
                // raw native-rate buffer to Qwen3's 16 kHz mono.
                if let Some(o) = &open {
                    *shared.meta.lock() = (o.sample_rate, o.channels);
                }

                // Clear the buffer THEN flip recording on. Any samples
                // produced by cpal between now and the Stop are kept
                // as-is in the raw buffer; leading silence (Bluetooth
                // SCO cold-start padding, mostly) gets trimmed in the
                // Stop handler before WAV encoding.
                shared.samples.lock().clear();
                shared.live_seq.store(0, Ordering::Relaxed);
                shared.recording.store(true, Ordering::Relaxed);
                // Arm the dead-stream watchdog for this recording.
                last_live_seq = 0;
                last_live_at = Instant::now();
                rebuilds_this_recording = 0;

                // Don't block on the first callback. AirPods (and other
                // Bluetooth mics) deliver 200-400 ms of silence-padded
                // buffers while the SCO link establishes; waiting for
                // them gives us a "ready" signal that's exactly aligned
                // with the moment the first useless buffer arrives, AND
                // lags the chip bar appearance for the same window.
                // Returning immediately lets the chip bar show right
                // away; the Stop-time trim recovers the same outcome
                // for the captured audio.
                last_activity = Instant::now();
                let resolution = open
                    .as_ref()
                    .map(|s| s.resolution.clone())
                    .unwrap_or_else(|| MicResolution {
                        requested: device_name.clone(),
                        actual: None,
                        fell_back: false,
                    });
                let _ = ready_tx.send(Ok(resolution));
            }
            Ok(AudioCmd::Stop { clip_tx }) => {
                shared.recording.store(false, Ordering::Relaxed);
                // Diagnostic: did the device deliver ANY audible sample this take?
                // "NO audio" on a recording where the user spoke = a dead stream.
                log::info!(
                    "[vibeking audio] stream delivered {} audio during this recording",
                    if shared.live_seq.load(Ordering::Relaxed) > 0 {
                        "live"
                    } else {
                        "NO (dead stream)"
                    }
                );
                last_activity = Instant::now();
                let (sample_rate, channels) = match &open {
                    Some(o) => (o.sample_rate, o.channels),
                    None => {
                        let _ = clip_tx.send(Err(anyhow!("no active stream on stop")));
                        continue;
                    }
                };
                let mut raw_samples = std::mem::take(&mut *shared.samples.lock());
                let raw_len = raw_samples.len();
                let raw_duration_ms = sample_to_ms(raw_len, sample_rate, channels);

                // Gain-normalize a quiet-but-real capture so EVERY downstream
                // consumer — the leading-silence trim, the silent-clip gate, and the
                // batch engines (cloud / Gemma 4 / Parakeet / Whisper) — sees a
                // normally-leveled signal. Some mics (or a user sitting back from a
                // laptop mic) deliver speech that peaks at ~0.004 (−48 dBFS), well
                // below the trim (0.003) and gate (0.01) thresholds, so real speech
                // was being trimmed to nothing and dropped as "silent". Normalization
                // is gated by a GAIN-INVARIANT voice check (dynamic range, not
                // absolute level), so genuine silence or steady hum is never
                // amplified into a hallucinated transcript. No-op for normal levels.
                if let Some(norm) = normalize_capture_gain(&mut raw_samples, sample_rate, channels)
                {
                    log::info!(
                        "[vibeking audio] quiet mic: normalized gain {:.1}x (peak {:.4} → {:.4})",
                        norm.gain,
                        norm.orig_peak,
                        norm.new_peak,
                    );
                }

                // Did the RAW capture actually contain speech? Gain-invariant
                // (dynamic-range) check — the same detector that gates the
                // normalization above, so it's true for a whisper-quiet mic too.
                // The keep/drop and stream-reopen decisions below key on this
                // instead of an absolute peak threshold a low-gain mic can't clear.
                let raw_has_voice = capture_has_voice(&raw_samples, sample_rate, channels);

                // Find (but don't yet apply) the leading-silence cut. We keep the
                // raw buffer intact so we can fall back to it if the trim turns out
                // to have eaten almost everything (quiet speech / cold-start tail).
                let trim_idx = leading_silence_index(&raw_samples, sample_rate, channels);
                let kept = &raw_samples[trim_idx..];
                let peak = kept.iter().fold(0.0_f32, |a, b| a.max(b.abs()));
                let sum_sq: f64 = kept.iter().map(|s| (*s as f64) * (*s as f64)).sum();
                let rms = if kept.is_empty() {
                    0.0
                } else {
                    (sum_sq / kept.len() as f64).sqrt()
                };
                let usable_ms = sample_to_ms(kept.len(), sample_rate, channels);
                let substantial = raw_len >= sample_rate as usize * channels as usize; // ≥ ~1 s

                if trim_idx > 0 {
                    log::info!(
                        "[vibeking audio] trimmed {} leading silent samples (~{} ms of cold-start padding)",
                        trim_idx,
                        sample_to_ms(trim_idx, sample_rate, channels),
                    );
                }
                log::info!(
                    "[vibeking audio] captured {}/{} samples ({} ch @ {} Hz) · peak={:.4} rms={:.4}",
                    kept.len(),
                    raw_len,
                    channels,
                    sample_rate,
                    peak,
                    rms,
                );

                // If the trim ate almost the entire take but the raw recording had
                // real length, preserve the UNTRIMMED audio so the stop handler can
                // retry on it instead of discarding the recording — the trim's
                // 0.003 / 10 ms-sustained threshold can over-eat genuinely quiet
                // speech (peak ~0.014 in the cold-start trace). Only encoded in this
                // failure case, to avoid double-encoding every clip.
                let recovery_wav = if trim_idx > 0 && substantial && usable_ms < 200 {
                    encode_wav(&raw_samples, sample_rate, channels).ok()
                } else {
                    None
                };

                let clip = encode_wav(kept, sample_rate, channels).map(|wav| RecordedClip {
                    wav,
                    duration_ms: usable_ms,
                    peak,
                    raw_duration_ms,
                    recovery_wav,
                    raw_has_voice,
                });
                let _ = clip_tx.send(clip);

                // A substantial recording that came back (near-)silent, OR that
                // trimmed away to almost nothing, means the stream wasn't really
                // capturing — a dead warm stream (macOS reuse hazard) or a cold
                // reopen that delivered silence for the whole take. Drop it so the
                // NEXT recording reopens a fresh stream instead of losing words
                // again. Peak alone wasn't enough: a lone ~0.014 blip at the tail
                // clears a 0.01 peak gate while the take is silent (the cold-start
                // trace), so we also catch "substantial raw → ~0 ms usable".
                //
                // BUT: a quiet-but-voiced take the leading-silence trim over-ate is
                // NOT a dead stream — the mic delivered real speech (the live
                // preview transcribed it). Reopening here would needlessly cold-start
                // the next recording. So only declare the stream dead when there was
                // genuinely no voice.
                const SILENT_PEAK: f32 = 0.01;
                if substantial && !raw_has_voice && (peak < SILENT_PEAK || usable_ms < 200) {
                    log::info!(
                        "[vibeking audio] {raw_duration_ms} ms recording → {usable_ms} ms usable (peak={peak:.4}) — dropping stream to force a fresh reopen"
                    );
                    open = None;
                }
            }
            Ok(AudioCmd::Cancel) => {
                shared.recording.store(false, Ordering::Relaxed);
                shared.samples.lock().clear();
                last_activity = Instant::now();
            }
            Ok(AudioCmd::StartMonitor { device_name }) => {
                // Reuse the warm stream if it's already on the right device,
                // otherwise (re)open. Same device-match logic as Start.
                let needs_reopen = match &open {
                    None => true,
                    Some(s) => s.requested_name != device_name,
                };
                if needs_reopen {
                    open = None;
                    match setup_stream(device_name.as_deref(), shared.clone()) {
                        Ok(o) => {
                            *shared.meta.lock() = (o.sample_rate, o.channels);
                            open = Some(o);
                        }
                        Err(e) => {
                            log::error!("[vibeking audio] monitor stream open failed: {e}");
                        }
                    }
                }
                last_activity = Instant::now();
            }
            Ok(AudioCmd::StopMonitor) => {
                // Flag already cleared by `stop_monitor`; just reset the idle
                // timer so the warm stream closes on the normal schedule.
                last_activity = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Dead-stream watchdog: while a recording is in progress, track the
                // audio heartbeat. If it keeps advancing the mic is live; if it
                // stalls for SILENCE_REBUILD the warm stream went dead (delivers
                // silent callbacks — the intermittent "no audio captured" bug), so
                // rebuild it on the same device. Re-heals up to MAX_REBUILDS times
                // per take, catching a stream that dies mid-recording, not only a
                // cold one. The samples buffer is preserved across the rebuild, so a
                // genuine speech pause loses nothing if this fires during one.
                if shared.recording.load(Ordering::Relaxed) {
                    let seq = shared.live_seq.load(Ordering::Relaxed);
                    if seq != last_live_seq {
                        last_live_seq = seq;
                        last_live_at = Instant::now();
                    } else if last_live_at.elapsed() > SILENCE_REBUILD
                        && rebuilds_this_recording < MAX_REBUILDS_PER_RECORDING
                    {
                        let dev = open.as_ref().and_then(|o| o.requested_name.clone());
                        log::info!(
                            "[vibeking audio] no audio for {}ms into recording — stream dead, rebuilding mid-recording (attempt {}/{})",
                            last_live_at.elapsed().as_millis(),
                            rebuilds_this_recording + 1,
                            MAX_REBUILDS_PER_RECORDING,
                        );
                        open = None;
                        match setup_stream(dev.as_deref(), shared.clone()) {
                            Ok(o) => {
                                *shared.meta.lock() = (o.sample_rate, o.channels);
                                open = Some(o);
                            }
                            Err(e) => {
                                log::error!("[vibeking audio] mid-recording reopen failed: {e}")
                            }
                        }
                        rebuilds_this_recording += 1;
                        // Give the fresh stream a full window before judging it dead.
                        last_live_at = Instant::now();
                    }
                }
                if open.is_some()
                    && !shared.recording.load(Ordering::Relaxed)
                    && !shared.monitoring.load(Ordering::Relaxed)
                    && last_activity.elapsed() > IDLE_TIMEOUT
                {
                    log::info!("[vibeking audio] idle {IDLE_TIMEOUT:?}, closing stream");
                    drop(open);
                    return; // next start_recording call will respawn the thread
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return;
            }
        }
    }
}

fn setup_stream(device_name: Option<&str>, shared: Arc<SharedCapture>) -> Result<OpenStream> {
    let host = cpal::default_host();
    let (device, fell_back) = pick_input_device(&host, device_name)
        .ok_or_else(|| anyhow!("no input device available"))?;
    let actual_name = device.name().ok();
    let config = device
        .default_input_config()
        .map_err(|e| anyhow!("default input config: {e}"))?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels();
    let sample_format = config.sample_format();
    let stream = build_stream(&device, &config, shared)?;
    stream.play().map_err(|e| anyhow!("cpal play: {e}"))?;
    log::info!(
        "[vibeking audio] opened stream: device={:?} (requested={:?}, fell_back={}) {} ch @ {} Hz, fmt={:?}",
        actual_name, device_name, fell_back, channels, sample_rate, sample_format,
    );
    let requested_name = device_name.map(str::to_string);
    let resolution = MicResolution {
        requested: requested_name.clone(),
        actual: actual_name,
        fell_back,
    };
    Ok(OpenStream {
        _stream: stream,
        sample_rate,
        channels,
        requested_name,
        resolution,
    })
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    shared: Arc<SharedCapture>,
) -> Result<cpal::Stream> {
    let stream_config: cpal::StreamConfig = config.clone().into();
    let err_fn = |err| log::error!("[vibeking] audio stream error: {err}");

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => {
            let shared = shared.clone();
            device.build_input_stream(
                &stream_config,
                move |data: &[f32], _| {
                    if shared.monitoring.load(Ordering::Relaxed) && !data.is_empty() {
                        let ss: f64 = data.iter().map(|s| (*s as f64) * (*s as f64)).sum();
                        shared.set_level((ss / data.len() as f64).sqrt() as f32);
                    }
                    if shared.recording.load(Ordering::Relaxed) {
                        if data.iter().any(|s| s.abs() > STREAM_ALIVE_FLOOR) {
                            shared.live_seq.fetch_add(1, Ordering::Relaxed);
                        }
                        shared.samples.lock().extend_from_slice(data);
                    }
                },
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::I16 => {
            let shared = shared.clone();
            device.build_input_stream(
                &stream_config,
                move |data: &[i16], _| {
                    if shared.monitoring.load(Ordering::Relaxed) && !data.is_empty() {
                        let ss: f64 = data
                            .iter()
                            .map(|&s| {
                                let f = s as f64 / i16::MAX as f64;
                                f * f
                            })
                            .sum();
                        shared.set_level((ss / data.len() as f64).sqrt() as f32);
                    }
                    if !shared.recording.load(Ordering::Relaxed) {
                        return;
                    }
                    let mut guard = shared.samples.lock();
                    guard.reserve(data.len());
                    let mut audible = false;
                    for &s in data {
                        let f = s as f32 / i16::MAX as f32;
                        if f.abs() > STREAM_ALIVE_FLOOR {
                            audible = true;
                        }
                        guard.push(f);
                    }
                    drop(guard);
                    if audible {
                        shared.live_seq.fetch_add(1, Ordering::Relaxed);
                    }
                },
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::U16 => {
            let shared = shared.clone();
            device.build_input_stream(
                &stream_config,
                move |data: &[u16], _| {
                    if shared.monitoring.load(Ordering::Relaxed) && !data.is_empty() {
                        let ss: f64 = data
                            .iter()
                            .map(|&s| {
                                let f = (s as f64 - 32768.0) / 32768.0;
                                f * f
                            })
                            .sum();
                        shared.set_level((ss / data.len() as f64).sqrt() as f32);
                    }
                    if !shared.recording.load(Ordering::Relaxed) {
                        return;
                    }
                    let mut guard = shared.samples.lock();
                    guard.reserve(data.len());
                    let mut audible = false;
                    for &s in data {
                        let f = (s as f32 - 32768.0) / 32768.0;
                        if f.abs() > STREAM_ALIVE_FLOOR {
                            audible = true;
                        }
                        guard.push(f);
                    }
                    drop(guard);
                    if audible {
                        shared.live_seq.fetch_add(1, Ordering::Relaxed);
                    }
                },
                err_fn,
                None,
            )?
        }
        fmt => return Err(anyhow!("unsupported sample format: {fmt:?}")),
    };

    Ok(stream)
}

/// Index of the first real audio in the buffer — everything before it is
/// leading silence to drop. The primary motivation is Bluetooth mic cold-start
/// padding (AirPods and most SCO/HFP devices deliver 200-400 ms of zero-padded
/// buffers while the link establishes) which we don't want in the WAV passed to
/// STT.
///
/// Returns the sample index to cut at (0 = nothing to trim). Works on FRAME
/// energy (RMS over ~10 ms windows), not per-sample levels: audio oscillates, so
/// individual samples dip toward zero every cycle — a per-sample
/// "continuously above 0.003" rule resets constantly and, for a quiet mic
/// (speech peaking ~0.04), never latches until some loud burst late in the take,
/// trimming away the real speech. Frame RMS averages over the cycle and is
/// stable.
///
/// The onset threshold is RELATIVE to the clip's own noise floor (with a tiny
/// absolute backstop), so it's gain-invariant — a whisper-quiet mic and a loud
/// one both work — mirroring [`capture_has_voice`]. Requires a couple of
/// consecutive elevated frames so a lone click/crackle doesn't unlatch it. If no
/// onset qualifies it returns 0 (trim nothing) — the safe failure: keeping the
/// whole take beats over-eating it, and the stop handler's recovery path handles
/// a quiet untrimmed clip. Pure (no mutation) so the caller can keep the
/// untrimmed buffer for a recovery retry.
fn leading_silence_index(samples: &[f32], sample_rate: u32, channels: u16) -> usize {
    if samples.is_empty() || sample_rate == 0 || channels == 0 {
        return 0;
    }
    const FRAME_MS: usize = 10;
    /// Onset must stand at least this far above the clip's own floor (gain-free).
    const RISE_FACTOR: f32 = 3.0;
    /// Absolute backstop so a faint-noise floor can't make the onset bar trivial,
    /// and so genuine near-silence (BT cold-start padding) is still trimmed.
    const MIN_ONSET: f32 = 0.004;
    /// Frames of sustained elevated energy required → rejects a lone click.
    const SUSTAIN_FRAMES: usize = 2;

    let ch = channels.max(1) as usize;
    let frame = (sample_rate as usize * FRAME_MS / 1000 * ch).max(ch);

    // Per-frame RMS + each frame's start sample index.
    let mut rms: Vec<f32> = Vec::with_capacity(samples.len() / frame + 1);
    let mut i = 0;
    while i + frame <= samples.len() {
        let slice = &samples[i..i + frame];
        let sum_sq: f64 = slice.iter().map(|s| (*s as f64) * (*s as f64)).sum();
        rms.push((sum_sq / slice.len() as f64).sqrt() as f32);
        i += frame;
    }
    // Too short to meaningfully trim — keep it all.
    if rms.len() <= SUSTAIN_FRAMES {
        return 0;
    }

    // Noise floor = a low percentile of frame energy; onset bar sits above it.
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let floor = sorted[(((sorted.len() - 1) as f32) * 0.2).round() as usize].max(1e-9);
    let onset = (floor * RISE_FACTOR).max(MIN_ONSET);

    // First frame that begins a run of SUSTAIN_FRAMES consecutive elevated frames.
    for f in 0..rms.len().saturating_sub(SUSTAIN_FRAMES - 1) {
        if rms[f..f + SUSTAIN_FRAMES].iter().all(|&r| r > onset) {
            return f * frame;
        }
    }
    0
}

/// Outcome of a gain-normalization pass. `None` from [`normalize_capture_gain`]
/// means nothing was changed (already a healthy level, or no voice to amplify).
pub(crate) struct GainNorm {
    gain: f32,
    orig_peak: f32,
    new_peak: f32,
}

/// Bring a quiet-but-real capture up to a consistent working level, in place.
///
/// Mics and setups vary by 30+ dB: a user sitting back from a laptop mic can
/// deliver speech that peaks at ~0.004 while another peaks at 0.3. Absolute
/// thresholds downstream (trim 0.003, silent-gate 0.01, VAD 0.02) then wrongly
/// treat the quiet take as silence and discard a real recording. Scaling the
/// whole clip to a target peak fixes every consumer at once and is what the ASR
/// engines effectively do internally anyway.
///
/// Crucially this only fires when [`capture_has_voice`] confirms real speech via
/// a GAIN-INVARIANT test (dynamic range), so pure silence, denormal noise, or a
/// steady hum is never amplified into a hallucinated transcript. Returns `None`
/// when the clip is already at a healthy level or has no voice to raise.
pub(crate) fn normalize_capture_gain(
    samples: &mut [f32],
    sample_rate: u32,
    channels: u16,
) -> Option<GainNorm> {
    /// Leave anything at or above this peak alone — it's already audible.
    const NORMALIZE_BELOW: f32 = 0.08;
    /// Scale a quiet take so its peak lands here (a comfortable −12 dBFS).
    const TARGET_PEAK: f32 = 0.25;
    /// Never amplify by more than this — caps how hard we lift near-silence even
    /// after the voice gate, so any residual noise stays bounded.
    const MAX_GAIN: f32 = 64.0;

    let orig_peak = samples.iter().fold(0.0_f32, |a, b| a.max(b.abs()));
    if orig_peak <= 0.0 || orig_peak >= NORMALIZE_BELOW {
        return None;
    }
    if !capture_has_voice(samples, sample_rate, channels) {
        return None;
    }
    let gain = (TARGET_PEAK / orig_peak).min(MAX_GAIN);
    if gain <= 1.0 {
        return None;
    }
    for s in samples.iter_mut() {
        *s = (*s * gain).clamp(-1.0, 1.0);
    }
    Some(GainNorm {
        gain,
        orig_peak,
        new_peak: (orig_peak * gain).min(1.0),
    })
}

/// Gain-INVARIANT speech detector: is there real voice in this clip regardless of
/// how loud it was captured? Compares a high percentile of per-frame RMS (the
/// "speech level") to a low percentile (the "noise floor"). Speech is peaky — it
/// alternates voiced bursts with pauses, so the ratio is large — while silence
/// and steady hum are flat (ratio ≈ 1). Because it keys on the RATIO it works for
/// a whisper-quiet mic and a loud one alike; a tiny absolute floor only rejects
/// true digital silence / denormals. Used to gate [`normalize_capture_gain`] so
/// we never amplify non-speech.
pub(crate) fn capture_has_voice(samples: &[f32], sample_rate: u32, channels: u16) -> bool {
    if sample_rate == 0 || samples.is_empty() {
        return false;
    }
    const FRAME_MS: usize = 20;
    /// Below this the "loud" frames are still essentially digital silence.
    const ABS_SPEECH_FLOOR: f32 = 0.0006;
    /// Speech peaks must stand at least this far above the clip's own floor.
    const DYN_RANGE_MIN: f32 = 3.0;

    let ch = channels.max(1) as usize;
    let win = (sample_rate as usize * FRAME_MS / 1000 * ch).max(ch);
    let mut frames: Vec<f32> = Vec::with_capacity(samples.len() / win + 1);
    let mut i = 0;
    while i + win <= samples.len() {
        let slice = &samples[i..i + win];
        let sum_sq: f64 = slice.iter().map(|s| (*s as f64) * (*s as f64)).sum();
        frames.push((sum_sq / slice.len() as f64).sqrt() as f32);
        i += win;
    }
    if frames.len() < 3 {
        return false;
    }
    frames.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = |p: f32| -> f32 {
        let idx = (((frames.len() - 1) as f32) * p).round() as usize;
        frames[idx]
    };
    let floor = pct(0.2).max(1e-9);
    let speech = pct(0.9);
    speech > ABS_SPEECH_FLOOR && (speech / floor) >= DYN_RANGE_MIN
}

fn sample_to_ms(samples: usize, sample_rate: u32, channels: u16) -> u64 {
    if sample_rate == 0 || channels == 0 {
        return 0;
    }
    (samples as u64 * 1000) / (sample_rate as u64 * channels as u64)
}

fn encode_wav(samples: &[f32], sample_rate: u32, channels: u16) -> Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::with_capacity(samples.len() * 2 + 44));
    let spec = WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    {
        let mut writer = WavWriter::new(&mut buf, spec)?;
        for &s in samples {
            let amplitude = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            writer.write_sample(amplitude)?;
        }
        writer.finalize()?;
    }
    Ok(buf.into_inner())
}

#[cfg(test)]
mod trim_tests {
    use super::leading_silence_index;

    const RATE: u32 = 48_000;

    #[test]
    fn no_trim_when_speech_starts_immediately() {
        let s = vec![0.5f32; RATE as usize]; // 1s of loud audio from sample 0
        assert_eq!(leading_silence_index(&s, RATE, 1), 0);
    }

    #[test]
    fn trims_leading_silence_up_to_sustained_speech() {
        let pad = RATE as usize / 2; // 0.5s of silence
        let mut s = vec![0.0f32; pad];
        s.extend(std::iter::repeat(0.5).take(RATE as usize)); // then 1s of speech
        assert_eq!(leading_silence_index(&s, RATE, 1), pad);
    }

    #[test]
    fn lone_click_does_not_unlatch_trim() {
        // A single 1ms blip in otherwise-silent audio is below the 10ms sustained
        // requirement, so it must NOT be treated as speech onset.
        let mut s = vec![0.0f32; RATE as usize];
        for v in s.iter_mut().take(RATE as usize / 1000) {
            *v = 0.8; // ~1ms click
        }
        // No sustained run → index stays at the click start (best-effort), and the
        // caller's recovery path keeps the raw audio. The key property: it does not
        // find a *valid* 10ms speech onset, so usable content is ~0 → recovery.
        let idx = leading_silence_index(&s, RATE, 1);
        // Either 0 (click start, run never completed) — never a false "speech" cut.
        assert!(idx <= RATE as usize / 1000);
    }

    /// A `freq`-Hz sine at peak `amp` for `secs` — oscillating audio that dips
    /// through zero every cycle, unlike the constant-amplitude blocks above (so it
    /// exercises the failure the old per-sample scan had on real waveforms).
    fn sine(amp: f32, freq: f32, secs: f32) -> Vec<f32> {
        let n = (RATE as f32 * secs) as usize;
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / RATE as f32).sin())
            .collect()
    }

    #[test]
    fn keeps_quiet_oscillating_speech_after_silence() {
        // Regression for the over-trim bug: a quiet mic (peak ~0.04 sine) after
        // 0.5s of silence. The old per-sample / absolute-0.003 scan reset on every
        // zero-crossing and only latched at a late loud burst, trimming the speech
        // away. Frame energy + a floor-relative onset must cut at the boundary.
        let pad = RATE as usize / 2;
        let mut s = vec![0.0f32; pad];
        s.extend(sine(0.04, 200.0, 1.0));
        let frame = (RATE as usize) * 10 / 1000;
        let idx = leading_silence_index(&s, RATE, 1);
        assert!(
            idx >= pad.saturating_sub(frame) && idx <= pad + frame,
            "should cut at the silence/speech boundary (~{pad}), got {idx}"
        );
        // The whole ~1s of quiet speech must survive (the old code ate it).
        assert!(
            s.len() - idx >= RATE as usize,
            "quiet speech must be kept, kept {}",
            s.len() - idx
        );
    }

    #[test]
    fn keeps_quiet_oscillating_speech_from_start() {
        // Quiet sine from sample 0, no leading silence → nothing to trim.
        let s = sine(0.04, 200.0, 1.0);
        assert_eq!(leading_silence_index(&s, RATE, 1), 0);
    }
}

#[cfg(test)]
mod gain_tests {
    use super::{capture_has_voice, normalize_capture_gain};

    const RATE: u32 = 48_000;

    /// 1 s signal that alternates 100 ms "voiced" blocks at `amp` with 100 ms
    /// "pause" blocks at `floor` — i.e. high dynamic range, like real speech.
    fn peaky(amp: f32, floor: f32) -> Vec<f32> {
        let block = RATE as usize / 10; // 100 ms
        let mut v = Vec::with_capacity(RATE as usize);
        for k in 0..10 {
            let level = if k % 2 == 0 { amp } else { floor };
            v.extend(std::iter::repeat(level).take(block));
        }
        v
    }

    fn peak(s: &[f32]) -> f32 {
        s.iter().fold(0.0_f32, |a, b| a.max(b.abs()))
    }

    #[test]
    fn voice_check_is_gain_invariant() {
        // The SAME peaky structure reads as voice whether captured loud or
        // whisper-quiet — that's the whole point (ratio, not absolute level).
        assert!(
            capture_has_voice(&peaky(0.30, 0.03), RATE, 1),
            "loud speech"
        );
        assert!(
            capture_has_voice(&peaky(0.004, 0.0005), RATE, 1),
            "quiet speech"
        );
    }

    #[test]
    fn voice_check_rejects_silence_and_steady_hum() {
        assert!(
            !capture_has_voice(&vec![0.0; RATE as usize], RATE, 1),
            "silence"
        );
        // A constant low level (hum) is flat → low dynamic range → not voice,
        // even though its absolute level clears any tiny floor.
        assert!(
            !capture_has_voice(&vec![0.02; RATE as usize], RATE, 1),
            "steady hum"
        );
    }

    #[test]
    fn normalizes_quiet_speech_up_to_target() {
        let mut s = peaky(0.004, 0.0005); // −48 dBFS speech
        let norm = normalize_capture_gain(&mut s, RATE, 1).expect("should normalize");
        assert!(norm.gain > 1.0);
        // Lifted into a healthy band (target 0.25), so the downstream trim/gate
        // thresholds (0.003 / 0.01) now see real speech.
        assert!(peak(&s) > 0.1, "post-norm peak={}", peak(&s));
        assert!(norm.new_peak <= 1.0);
    }

    #[test]
    fn leaves_healthy_levels_untouched() {
        let mut s = peaky(0.30, 0.03);
        assert!(normalize_capture_gain(&mut s, RATE, 1).is_none());
        assert!((peak(&s) - 0.30).abs() < 1e-6, "must be byte-identical");
    }

    #[test]
    fn never_amplifies_silence_or_hum() {
        // Pure silence: nothing to lift.
        let mut silence = vec![0.0; RATE as usize];
        assert!(normalize_capture_gain(&mut silence, RATE, 1).is_none());
        // Quiet steady hum: below the normalize ceiling, but no VOICE → left as-is
        // so it can't be amplified into a hallucinated transcript.
        let mut hum = vec![0.01; RATE as usize];
        assert!(normalize_capture_gain(&mut hum, RATE, 1).is_none());
        assert!(peak(&hum) < 0.02, "hum must not be amplified");
    }

    #[test]
    fn voice_check_survives_loud_transient_then_quiet_speech() {
        // The cold-start failure mode: a brief loud click/transient (which sets the
        // buffer PEAK, so the absolute-peak silent gate is misled) followed by
        // genuinely quiet speech. The gain-invariant detector must STILL report
        // voice so the stop handler recovers the take (RecordedClip.raw_has_voice)
        // instead of dropping a real recording as "silent".
        let mut v = peaky(0.004, 0.0005); // −48 dBFS quiet speech
        for x in v.iter_mut().take(RATE as usize / 1000) {
            *x = 0.4; // ~1 ms transient, far above the speech level
        }
        assert!(peak(&v) > 0.3, "the transient sets a high peak");
        assert!(
            capture_has_voice(&v, RATE, 1),
            "quiet speech behind a transient must still read as voice"
        );
    }
}
