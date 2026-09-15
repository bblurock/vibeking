//! Gemma 4 audio STT via an app-managed MLX sidecar (macOS / Apple Silicon).
//!
//! Gemma's audio modality has no Rust/CoreML path — it only runs through Python
//! MLX (`mlx-vlm`). Rather than bundle a fragile frozen interpreter, vibeking
//! manages a Python virtualenv and spawns `mlx_vlm.server` (an OpenAI-compatible
//! HTTP server with `input_audio` support), then POSTs audio to it from
//! [`crate::stt`]. This mirrors how the app already drives local LLM servers for
//! refinement (`polish.rs` → Ollama / mlx-lm).
//!
//! Lifecycle: `setup()` creates the venv + installs mlx-vlm (one-time);
//! `ensure_running()` spawns the server (downloading the ~8 GB model on first
//! run) and waits for health; `shutdown()` kills it. The app calls `shutdown()`
//! on exit so no orphaned server leaks the model in RAM.
//!
//! NOTE (hardening follow-ups): uses the system `python3` for the venv — a
//! bundled `uv` would remove that dependency. The server is killed as a direct
//! child; if a future mlx-vlm spawns workers, switch to a process-group kill.
//! The server CLI form, port, and health path are constants below so they're
//! trivial to adjust once verified against the installed mlx-vlm version.

#[cfg(not(target_os = "macos"))]
pub use noop::*;

#[cfg(target_os = "macos")]
pub use platform::*;

/// Identifier used by the model-management UI / Tauri commands.
pub const ENGINE_ID: &str = "gemma-4";

/// Default MLX repo for the Gemma audio model. Overridable via settings.
/// 4-bit quant (~4 GB resident) rather than bf16 (~16 GB) — the keep-warm
/// server holds this in memory, so the quant matters a lot for footprint.
pub const DEFAULT_MODEL: &str = "mlx-community/gemma-4-E4B-it-4bit";

#[cfg(not(target_os = "macos"))]
mod noop {
    use anyhow::{anyhow, Result};

    pub fn engine_ready() -> bool {
        false
    }
    pub fn is_preparing() -> bool {
        false
    }
    pub fn is_running() -> bool {
        false
    }
    pub fn idle_secs() -> Option<u64> {
        None
    }
    pub fn set_idle_policy(_idle_secs: u64) {}
    pub async fn setup(
        _on_progress: impl Fn(u64, Option<u64>) + Send + Sync + 'static,
    ) -> Result<()> {
        Err(anyhow!("Gemma engine only available on macOS"))
    }
    pub async fn ensure_running(_model: &str) -> Result<u16> {
        Err(anyhow!("Gemma engine only available on macOS"))
    }
    pub async fn ensure_streaming(_model: &str) -> Result<u16> {
        Err(anyhow!("Gemma engine only available on macOS"))
    }
    pub fn shutdown() {}
    pub fn clear_engine() -> Result<()> {
        Err(anyhow!("Gemma engine only available on macOS"))
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use anyhow::{anyhow, Context, Result};
    use parking_lot::Mutex;

    /// Fixed loopback port for the sidecar. High + uncommon to avoid clashes;
    /// if it's ever taken, the spawn's health check fails and surfaces cleanly.
    const GEMMA_PORT: u16 = 51247;
    /// How the server is launched, relative to the venv's python.
    const SERVER_MODULE: &str = "mlx_vlm.server";
    /// Health endpoint polled until the model is loaded and serving.
    const HEALTH_PATH: &str = "/v1/models";

    // --- EXPERIMENTAL streaming sidecar (Settings.gemma_streaming) ----------
    // The stateful KV-cache-reuse server (resources/gemma_stream_server.py). When the flag
    // is on, we spawn this script instead of `mlx_vlm.server` and drive it via
    // the session API. Scripts are embedded and written into the venv at spawn
    // time so Python's script-dir import resolves `gemma_session`.
    const STREAM_SERVER_PY: &str = "gemma_stream_server.py";
    const STREAM_SESSION_PY: &str = "gemma_session.py";
    const STREAM_HEALTH_PATH: &str = "/health";
    const STREAM_SERVER_SRC: &str =
        include_str!("../resources/gemma_stream_server.py");
    const STREAM_SESSION_SRC: &str = include_str!("../resources/gemma_session.py");

    /// Write the streaming sidecar scripts into the venv dir (idempotent;
    /// rewritten each spawn so an app upgrade refreshes them).
    fn deploy_stream_scripts(venv: &std::path::Path) -> Result<()> {
        std::fs::write(venv.join(STREAM_SERVER_PY), STREAM_SERVER_SRC)
            .context("write gemma_stream_server.py")?;
        std::fs::write(venv.join(STREAM_SESSION_PY), STREAM_SESSION_SRC)
            .context("write gemma_session.py")?;
        Ok(())
    }
    /// Generous: first run downloads the model + loads onto the ANE.
    const HEALTH_TIMEOUT_SECS: u64 = 1200;
    /// Idle window (seconds) after which the server is unloaded to free its
    /// multi-GB resident footprint; the next transcription re-spawns it (cold
    /// load). Configurable from settings via `set_idle_policy` — `0` means
    /// "never unload" (the user's Keep-loaded toggle). Default 15 min.
    static IDLE_SHUTDOWN_SECS: AtomicU64 = AtomicU64::new(900);

    /// Apply the user's Gemma memory policy. `idle_secs == 0` keeps the sidecar
    /// resident (never unload); otherwise it unloads after that many idle
    /// seconds. Read live by the idle watcher each tick, so a change takes
    /// effect without restarting the watcher.
    pub fn set_idle_policy(idle_secs: u64) {
        IDLE_SHUTDOWN_SECS.store(idle_secs, Ordering::Relaxed);
    }

    struct RunningServer {
        child: Child,
        port: u16,
        model: String,
        /// Which sidecar this is — the streaming session server vs. the stateless
        /// `mlx_vlm.server`. A reuse must match the requested mode.
        streaming: bool,
        /// True only AFTER the spawn's `/health` poll passed — i.e. the model is
        /// loaded and the HTTP server is actually listening. A just-spawned child
        /// is alive but not yet ready (the MLX model load takes seconds after an
        /// idle-unload); reusing it then makes callers POST to a port that isn't
        /// listening, which is exactly the cold-start "session/start connection
        /// refused" that lost the first recording. See `reuse_if_healthy`.
        ready: bool,
    }

    static SERVER: Mutex<Option<RunningServer>> = Mutex::new(None);
    static PREPARING: AtomicBool = AtomicBool::new(false);
    /// Last time the server was used (touched on every `ensure_running`). The
    /// idle watcher unloads the server once this exceeds `IDLE_SHUTDOWN_SECS`.
    static LAST_USED: Mutex<Option<std::time::Instant>> = Mutex::new(None);

    fn touch() {
        *LAST_USED.lock() = Some(std::time::Instant::now());
    }

    /// Seconds since the server was last used this session, or `None` if never
    /// used yet. Lets the record-start diagnostic record "cold after N s idle"
    /// as a fact (the idle watcher unloads after `IDLE_SHUTDOWN_SECS`).
    pub fn idle_secs() -> Option<u64> {
        LAST_USED.lock().as_ref().map(|t| t.elapsed().as_secs())
    }

    fn data_dir() -> Option<PathBuf> {
        dirs::data_local_dir().map(|d| d.join("Vibeking"))
    }
    fn venv_dir() -> Option<PathBuf> {
        data_dir().map(|d| d.join("gemma-venv"))
    }
    fn venv_python() -> Option<PathBuf> {
        venv_dir().map(|d| d.join("bin/python3"))
    }

    /// The Python version uv manages for us. Pinned so every install matches the
    /// locked `gemma-requirements.txt` (which requires >=3.10; mlx-vlm 0.6.0).
    const MANAGED_PYTHON: &str = "3.11";

    /// Locked, hashed dependency set — embedded so a shipped build is fully
    /// self-describing (same pattern as the streaming scripts above). Generated
    /// from the known-good env via `uv pip compile` (see scripts / plans).
    const GEMMA_REQUIREMENTS_SRC: &str = include_str!("../resources/gemma-requirements.txt");

    /// Model prefetch script — downloads the ~8 GB model into the contained HF
    /// cache during setup (so "Ready" is honest), emitting `PROGRESS d t` lines.
    const GEMMA_PREFETCH_SRC: &str = include_str!("../resources/gemma_prefetch.py");

    /// uv's managed-Python and cache dirs, kept INSIDE our data dir so the whole
    /// Python toolchain is contained and removed with the app — never touching
    /// the user's system Python or `~/.local/share/uv`.
    fn uv_python_dir() -> Option<PathBuf> {
        data_dir().map(|d| d.join("uv/python"))
    }
    fn uv_cache_dir() -> Option<PathBuf> {
        data_dir().map(|d| d.join("uv/cache"))
    }
    /// Hugging Face cache, contained in our data dir so the ~8 GB model lives
    /// with the app (removable, and discoverable by the sidecar). Both `setup()`
    /// (prefetch) and the runtime sidecar spawn point `HF_HOME` here.
    fn hf_cache_dir() -> Option<PathBuf> {
        data_dir().map(|d| d.join("hf-cache"))
    }

    /// Path to the bundled `uv` binary. In a release `.app`, Tauri places
    /// externalBin next to the main executable in `Contents/MacOS/`; in dev it
    /// lives in the source tree where `scripts/fetch-uv.sh` puts it.
    fn uv_bin() -> PathBuf {
        if cfg!(debug_assertions) {
            PathBuf::from(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/binaries/uv-aarch64-apple-darwin"
            ))
        } else {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join("uv")))
                .unwrap_or_else(|| PathBuf::from("uv"))
        }
    }

    /// Readiness marker. Bumped to a uv-scheme name so installs from the old
    /// "system python3 + unpinned pip" path (which could be Python 3.9 with an
    /// incompatible mlx-vlm) are treated as not-ready and rebuilt by `setup()`.
    fn installed_marker() -> Option<PathBuf> {
        venv_dir().map(|d| d.join(".deps-ready-uv1"))
    }
    /// The legacy marker; its presence (without the new one) signals a pre-uv
    /// venv that must be wiped and rebuilt.
    fn legacy_marker() -> Option<PathBuf> {
        venv_dir().map(|d| d.join(".mlx-vlm-installed"))
    }

    /// "Ready" = the venv exists AND the model is downloaded — the marker is
    /// written by `setup()` only after the model prefetch succeeds, so this is
    /// honest (no "Ready" while the 8 GB model is still downloading). The server
    /// need not be up. A pre-uv install (legacy marker only) is NOT ready.
    pub fn engine_ready() -> bool {
        match (venv_python(), installed_marker()) {
            (Some(py), Some(marker)) => py.exists() && marker.exists(),
            _ => false,
        }
    }

    pub fn is_preparing() -> bool {
        PREPARING.load(Ordering::Relaxed)
    }

    pub fn is_running() -> bool {
        SERVER.lock().is_some()
    }

    struct PreparingGuard;
    impl Drop for PreparingGuard {
        fn drop(&mut self) {
            PREPARING.store(false, Ordering::Relaxed);
        }
    }

    /// Build the venv with a bundled, managed Python + pinned deps via `uv`.
    /// Idempotent. `on_progress` is coarse: 1/3 managed Python, 2/3 venv,
    /// 3/3 locked deps installed.
    ///
    /// This deliberately does NOT use the user's system `python3` or `pip`: that
    /// was the source of field breakage (Python 3.9 vs 3.11, and an unpinned
    /// mlx-vlm that dropped `mlx_vlm.models.cache`). uv downloads its own
    /// CPython and installs the exact locked set in `gemma-requirements.txt`.
    pub async fn setup(
        on_progress: impl Fn(u64, Option<u64>) + Send + Sync + 'static,
    ) -> Result<()> {
        PREPARING.store(true, Ordering::Relaxed);
        let _guard = PreparingGuard;

        let venv = venv_dir().ok_or_else(|| anyhow!("no data dir"))?;
        let venv_py = venv_python().ok_or_else(|| anyhow!("no data dir"))?;
        let marker = installed_marker().ok_or_else(|| anyhow!("no data dir"))?;
        let legacy = legacy_marker().ok_or_else(|| anyhow!("no data dir"))?;
        let py_dir = uv_python_dir().ok_or_else(|| anyhow!("no data dir"))?;
        let cache_dir = uv_cache_dir().ok_or_else(|| anyhow!("no data dir"))?;
        let hf_cache = hf_cache_dir().ok_or_else(|| anyhow!("no data dir"))?;
        let uv = uv_bin();
        if !uv.exists() {
            return Err(anyhow!(
                "bundled uv not found at {} (run scripts/fetch-uv.sh)",
                uv.display()
            ));
        }

        // Venv build is fast and has no byte total — report indeterminate
        // (`None`) so the bar doesn't jump backward when the model's byte-% starts.
        on_progress(0, None);
        tauri::async_runtime::spawn_blocking(move || -> Result<()> {
            use std::io::{BufRead, BufReader};
            std::fs::create_dir_all(venv.parent().unwrap()).ok();
            std::fs::create_dir_all(&py_dir).ok();
            std::fs::create_dir_all(&cache_dir).ok();
            std::fs::create_dir_all(&hf_cache).ok();

            // Migration: a pre-uv venv (legacy marker, or no new marker) may be
            // Python 3.9 with an incompatible mlx-vlm. Wipe it and rebuild clean.
            if venv.exists() && (!marker.exists() || legacy.exists()) {
                log::info!("[gemma setup] removing pre-uv venv to rebuild with managed Python");
                std::fs::remove_dir_all(&venv)
                    .with_context(|| format!("remove stale venv {}", venv.display()))?;
            }

            // Run uv with its managed-Python + cache dirs contained in our data
            // dir, so nothing touches the user's system Python or ~/.local.
            let run = |args: &[&str], what: &str| -> Result<()> {
                let out = Command::new(&uv)
                    .args(args)
                    .env("UV_PYTHON_INSTALL_DIR", &py_dir)
                    .env("UV_CACHE_DIR", &cache_dir)
                    // Never silently fall back to a system interpreter.
                    .env("UV_PYTHON_DOWNLOADS", "automatic")
                    .env("UV_NO_CONFIG", "1")
                    .output()
                    .with_context(|| format!("run uv {what}"))?;
                if !out.status.success() {
                    log::error!(
                        "[gemma setup] uv {what} failed: {}",
                        String::from_utf8_lossy(&out.stderr)
                    );
                    return Err(anyhow!("uv {what} failed ({})", out.status));
                }
                Ok(())
            };

            // 1/3: ensure the managed CPython exists. `--no-bin` keeps uv from
            // writing a `python3.11` shim into the user's ~/.local/bin — we only
            // want the interpreter inside our contained data dir.
            run(&["python", "install", "--no-bin", MANAGED_PYTHON], "python install")?;

            // Create the venv from that managed Python.
            run(
                &[
                    "venv",
                    "--python",
                    MANAGED_PYTHON,
                    venv.to_str().ok_or_else(|| anyhow!("venv path not utf-8"))?,
                ],
                "venv",
            )?;

            // Install the locked, hashed deps into the venv. Write the embedded
            // lock to a file uv can read.
            let req_path = venv.join("gemma-requirements.txt");
            std::fs::write(&req_path, GEMMA_REQUIREMENTS_SRC)
                .with_context(|| format!("write {}", req_path.display()))?;
            run(
                &[
                    "pip",
                    "install",
                    "--python",
                    venv_py.to_str().ok_or_else(|| anyhow!("py path not utf-8"))?,
                    "-r",
                    req_path.to_str().ok_or_else(|| anyhow!("req path not utf-8"))?,
                ],
                "pip install",
            )?;

            // Download the ~8 GB model NOW (not lazily on first record) so the
            // "Ready" badge is honest. Stream the script's `PROGRESS d t` lines
            // straight into the byte-% progress callback.
            let prefetch_py = venv.join("gemma_prefetch.py");
            std::fs::write(&prefetch_py, GEMMA_PREFETCH_SRC)
                .with_context(|| format!("write {}", prefetch_py.display()))?;
            let mut child = Command::new(&venv_py)
                .arg(&prefetch_py)
                .arg(super::DEFAULT_MODEL)
                .env("HF_HOME", &hf_cache)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context("spawn gemma_prefetch.py")?;
            if let Some(err) = child.stderr.take() {
                drain_into_log("prefetch", err);
            }
            if let Some(out) = child.stdout.take() {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    if let Some(rest) = line.strip_prefix("PROGRESS ") {
                        let mut it = rest.split_whitespace();
                        if let (Some(d), Some(t)) = (it.next(), it.next()) {
                            if let (Ok(d), Ok(t)) = (d.parse::<u64>(), t.parse::<u64>()) {
                                on_progress(d, Some(t));
                            }
                        }
                    }
                }
            }
            let status = child.wait().context("wait gemma_prefetch.py")?;
            if !status.success() {
                return Err(anyhow!("model prefetch failed ({status})"));
            }

            // Mark ready ONLY now — venv AND model are both present, so
            // `engine_ready()` is honest. Drop the legacy marker if present.
            std::fs::write(&marker, format!("mlx-vlm via uv, python {MANAGED_PYTHON}\n").as_bytes())
                .ok();
            let _ = std::fs::remove_file(&legacy);
            on_progress(1, Some(1));
            Ok(())
        })
        .await
        .map_err(|e| anyhow!("uv setup join: {e}"))??;

        Ok(())
    }

    /// A healthy server already serving `model`, if any. Releases the (non-Send)
    /// SERVER guard before awaiting health so the future stays Send.
    async fn reuse_if_healthy(model: &str, streaming: bool) -> Option<u16> {
        // Reuse if OUR tracked child for this model+mode is still ALIVE. We
        // spawned it and confirmed health at spawn time. The single-threaded MLX
        // server can be briefly busy with an in-flight decode, during which a
        // /health ping would queue and TIME OUT — the old code then concluded
        // "dead" and killed + reloaded the 8 GB model on every recording, which
        // was the multi-second live-preview delay. Process-alive means it's
        // serving (a queued request just waits its turn), so don't reload. A
        // dead/exited child → None → (re)spawn. No await: holds the guard only
        // for the synchronous liveness probe.
        let mut guard = SERVER.lock();
        let s = guard.as_mut()?;
        if s.model != model || s.streaming != streaming {
            return None;
        }
        // Not yet health-confirmed (still loading the model after a cold spawn)
        // → don't hand out the port; the caller waits on the spawn gate until the
        // spawner flips `ready` so its session/start hits a listening server.
        if !s.ready {
            return None;
        }
        match s.child.try_wait() {
            Ok(None) => Some(s.port), // still running
            _ => None,                // exited, or can't tell → respawn
        }
    }

    /// Process-wide gate serializing server (re)spawns — see `ensure_running`.
    fn spawn_gate() -> &'static tokio::sync::Mutex<()> {
        static GATE: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
        GATE.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    /// Pump a sidecar output stream into the app log, line by line. The thread
    /// exits on EOF (sidecar exit/kill), so it never outlives the child. This is
    /// the only window into why Gemma "silently doesn't transcribe" on a user's
    /// machine — the sidecar was previously spawned with `Stdio::null()`.
    fn drain_into_log(tag: &'static str, stream: impl std::io::Read + Send + 'static) {
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                log::info!("[gemma sidecar {tag}] {line}");
            }
        });
    }

    /// Ensure the stateless `mlx_vlm.server` is up for `model`; return its port.
    pub async fn ensure_running(model: &str) -> Result<u16> {
        ensure_running_mode(model, false).await
    }

    /// Ensure the EXPERIMENTAL streaming session sidecar is up for `model`.
    /// Spawned only when `Settings.gemma_streaming` is on. Mutually exclusive
    /// with the stateless server (same port, same single-server lifecycle).
    pub async fn ensure_streaming(model: &str) -> Result<u16> {
        ensure_running_mode(model, true).await
    }

    /// Ensure a server in the requested mode is up for `model` and return its
    /// port. Spawns it if not running (or if the model / mode changed), then
    /// polls health until ready.
    async fn ensure_running_mode(model: &str, streaming: bool) -> Result<u16> {
        touch(); // record usage so the idle watcher doesn't unload mid-session

        // Fast path: a healthy server already serving this model+mode — no gate.
        if let Some(port) = reuse_if_healthy(model, streaming).await {
            return Ok(port);
        }

        // Serialize (re)spawns. Without this gate, a second caller that arrives
        // while the first is still loading the model (health not yet OK) would
        // fall through to shutdown() + respawn and KILL the in-flight load. This
        // races in practice: after the idle-unload, both the record-start prewarm
        // and the live-preview loop call ensure_running during a recording. Gate
        // so exactly one spawns; everyone else waits here, then reuses the
        // freshly-healthy server below.
        let _gate = spawn_gate().lock().await;

        // Re-check under the gate — a caller we queued behind may have just
        // spawned and healthed the server while we waited.
        if let Some(port) = reuse_if_healthy(model, streaming).await {
            return Ok(port);
        }

        // Tear down any stale/mismatched server (incl. a wrong-mode one), then
        // (re)spawn. Also free the port from an ORPHAN we don't own (a sidecar
        // left by a crashed/force-quit prior session) so our spawn can bind —
        // otherwise it silently fails to bind and the health poll hangs (or, for
        // the streaming mode, an orphaned mlx_vlm.server answers /health).
        shutdown();
        free_stale_port(GEMMA_PORT);

        let py = venv_python()
            .filter(|p| p.exists())
            .ok_or_else(|| anyhow!("Gemma not set up — create the venv first (Set up engine)"))?;
        let venv = venv_dir().ok_or_else(|| anyhow!("no data dir"))?;
        let mut cmd = Command::new(&py);
        // Point the sidecar at the contained HF cache where setup() prefetched
        // the model, so it loads from disk instead of re-downloading to
        // ~/.cache/huggingface.
        if let Some(hf) = hf_cache_dir() {
            cmd.env("HF_HOME", hf);
        }
        if streaming {
            deploy_stream_scripts(&venv)?;
            cmd.arg(venv.join(STREAM_SERVER_PY))
                .current_dir(&venv) // so `import gemma_session` resolves
                .args(["--model", model])
                .args(["--host", "127.0.0.1"])
                .args(["--port", &GEMMA_PORT.to_string()]);
        } else {
            cmd.args(["-m", SERVER_MODULE])
                .args(["--model", model])
                .args(["--host", "127.0.0.1"])
                .args(["--port", &GEMMA_PORT.to_string()]);
        }
        let mut child = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context(if streaming {
                "spawn gemma_stream_server.py"
            } else {
                "spawn mlx_vlm.server"
            })?;
        // Tee the sidecar's output into the app log instead of /dev/null — its
        // stderr is the single most useful diagnostic when Gemma fails on a
        // user's machine (model load OOM, missing dep, port bind failure).
        if let Some(out) = child.stdout.take() {
            drain_into_log("stdout", out);
        }
        if let Some(err) = child.stderr.take() {
            drain_into_log("stderr", err);
        }

        *SERVER.lock() = Some(RunningServer {
            child,
            port: GEMMA_PORT,
            model: model.to_string(),
            streaming,
            ready: false, // flipped true below once /health passes
        });

        // Poll health until the model finishes downloading + loading.
        let deadline = std::time::Duration::from_secs(HEALTH_TIMEOUT_SECS);
        let start = tauri::async_runtime::spawn(async move {
            let begin = std::time::Instant::now();
            loop {
                if health_ok(GEMMA_PORT, streaming).await {
                    return true;
                }
                if begin.elapsed() > deadline {
                    return false;
                }
                tokio::time::sleep(std::time::Duration::from_millis(750)).await;
            }
        })
        .await
        .map_err(|e| anyhow!("health join: {e}"))?;

        if start {
            // Health confirmed — mark ready so concurrent callers waiting on the
            // gate (the live-preview session AND the stop-time recovery) now get a
            // port that's actually listening.
            if let Some(s) = SERVER.lock().as_mut() {
                s.ready = true;
            }
            spawn_idle_watcher();
            Ok(GEMMA_PORT)
        } else {
            shutdown();
            Err(anyhow!("Gemma server failed to become healthy in time"))
        }
    }

    /// One watcher per server lifetime: unloads the server once it's gone
    /// `IDLE_SHUTDOWN_SECS` without a transcription, freeing its resident RAM.
    fn spawn_idle_watcher() {
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                if !is_running() {
                    return; // server already gone (manual stop / model switch)
                }
                let idle = LAST_USED
                    .lock()
                    .as_ref()
                    .map(|t| t.elapsed())
                    .unwrap_or_default();
                let limit = IDLE_SHUTDOWN_SECS.load(Ordering::Relaxed);
                // `0` = Keep-loaded: never unload (just keep polling so a later
                // policy change back to a finite timeout still takes effect).
                if limit > 0 && idle >= std::time::Duration::from_secs(limit) {
                    log::info!("[vibeking gemma] idle {limit}s — unloading server to free memory");
                    shutdown();
                    return;
                }
            }
        });
    }

    async fn health_ok(port: u16, streaming: bool) -> bool {
        let path = if streaming {
            STREAM_HEALTH_PATH
        } else {
            HEALTH_PATH
        };
        let url = format!("http://127.0.0.1:{port}{path}");
        let res = reqwest::Client::new()
            .get(&url)
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await;
        match res {
            Ok(r) if r.status().is_success() => {
                if streaming {
                    // CRITICAL: mlx_vlm.server ALSO answers GET /health (with
                    // `{"status":"healthy",...}`), so a 200 alone can't tell our
                    // streaming sidecar apart from a stale stateless server on the
                    // same port. Require our distinctive `{"ok":true}` marker, or a
                    // lingering mlx_vlm.server masquerades as healthy and every
                    // /session/* call 404s.
                    r.text()
                        .await
                        .map(|b| b.contains("\"ok\""))
                        .unwrap_or(false)
                } else {
                    true
                }
            }
            _ => false,
        }
    }

    /// Kill the running server (direct child). Called on app exit and before a
    /// model switch.
    pub fn shutdown() {
        if let Some(mut s) = SERVER.lock().take() {
            let _ = s.child.kill();
            let _ = s.child.wait();
        }
    }

    /// Kill any process STILL listening on `port` that we don't own — a stale
    /// sidecar orphaned by a crashed or force-quit previous session. `shutdown()`
    /// only reaps the child we spawned; an orphan from a prior run keeps the port
    /// bound, so our fresh spawn can't bind and (worse) `mlx_vlm.server`'s
    /// `/health` would masquerade as healthy. Best-effort, macOS-only (lsof).
    fn free_stale_port(port: u16) {
        let Ok(out) = Command::new("/usr/sbin/lsof")
            .args(["-ti", &format!("tcp:{port}"), "-sTCP:LISTEN"])
            .output()
        else {
            return;
        };
        for pid in String::from_utf8_lossy(&out.stdout).split_whitespace() {
            log::info!("[vibeking gemma] freeing port {port}: killing stale sidecar pid {pid}");
            let _ = Command::new("/bin/kill").arg("-9").arg(pid).status();
        }
    }

    /// Stop the server and remove the venv (forces a fresh setup next time).
    /// Leaves the HuggingFace model cache alone (shared, user-managed).
    pub fn clear_engine() -> Result<()> {
        shutdown();
        if let Some(venv) = venv_dir() {
            if venv.exists() {
                std::fs::remove_dir_all(&venv)
                    .with_context(|| format!("remove {}", venv.display()))?;
            }
        }
        // Also free the ~8 GB model so "delete" reclaims the space it took.
        if let Some(hf) = hf_cache_dir() {
            if hf.exists() {
                std::fs::remove_dir_all(&hf)
                    .with_context(|| format!("remove {}", hf.display()))?;
            }
        }
        Ok(())
    }
}
