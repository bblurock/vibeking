import { listen } from "@tauri-apps/api/event";
import { isTauri } from "./runtime";
import { patchSettings } from "./use-settings";
import { isProvider } from "./settings";

let started = false;
const unlisteners: Array<() => void> = [];

export function startTrayListener() {
  if (started) return;
  if (!isTauri()) return;
  started = true;

  void listen<string>("tray:set-stt-language", (e) => {
    const lang = e.payload;
    if (!lang) return;
    void patchSettings({ sttLanguage: lang });
  })
    .then((off) => unlisteners.push(off))
    .catch((err) =>
      console.warn("[vibeking] tray language listener failed:", err),
    );

  void listen<string>("tray:set-provider", (e) => {
    // Trust the Rust side's validated id, but narrow against the canonical
    // provider list (covers ALL engines, incl. qwen3/gemma/fluid-audio — the
    // hand-maintained allowlist that used to live here dropped those, so a tray
    // switch to a local engine never reached the app UI or the store).
    const value = e.payload;
    if (!isProvider(value)) return;
    void patchSettings({ provider: value });
  })
    .then((off) => unlisteners.push(off))
    .catch((err) =>
      console.warn("[vibeking] tray provider listener failed:", err),
    );

  // Rust emits this from the refinement submenu click handler. `__off__`
  // is the sentinel for the "Off" entry. Everything else is a mode id
  // straight from the user's settings (built-in or custom). The Rust
  // side already validated and broadcast `refinement:active-changed`
  // ahead of this — this listener just persists the same write through
  // the canonical TS save path so the JSON store doesn't drift.
  void listen<string>("tray:set-refinement-mode", (e) => {
    const value = e.payload;
    if (!value) return;
    const id = value === "__off__" ? null : value;
    void patchSettings({ activeRefinementModeId: id });
  })
    .then((off) => unlisteners.push(off))
    .catch((err) =>
      console.warn("[vibeking] tray refinement listener failed:", err),
    );

  // Rust emits this from the microphone submenu click handler. The
  // payload is the literal `settings.micDevice` value to persist —
  // empty string means "System default" (matches the existing schema
  // where `mic_device: ""` is the documented sentinel).
  void listen<string>("tray:set-mic-device", (e) => {
    // Payload is always a string from the Rust side; allow undefined
    // for defensive parsing. Empty string IS valid (system default) so
    // we don't early-return on falsy.
    if (typeof e.payload !== "string") return;
    void patchSettings({ micDevice: e.payload });
  })
    .then((off) => unlisteners.push(off))
    .catch((err) =>
      console.warn("[vibeking] tray mic listener failed:", err),
    );

  // Rust emits this from the "Always-on bar" tray toggle. The Rust side has
  // already flipped its settings cache + reconciled the window; this persists
  // the same boolean through the canonical TS save path so the JSON store and
  // the Home settings switch stay in sync.
  void listen<boolean>("tray:set-persistent-bar", (e) => {
    if (typeof e.payload !== "boolean") return;
    void patchSettings({ persistentBar: e.payload });
  })
    .then((off) => unlisteners.push(off))
    .catch((err) =>
      console.warn("[vibeking] tray persistent-bar listener failed:", err),
    );
}

export function stopTrayListener() {
  while (unlisteners.length) {
    const off = unlisteners.pop();
    off?.();
  }
  started = false;
}
