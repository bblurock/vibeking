import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Engine =
  | "whisper"
  | "parakeet"
  | "qwen3"
  | "gemma";

export type ModelStatus = {
  id: string;
  exists: boolean;
  path: string;
  engine: Engine | string;
  /** True while the Rust engine is mid-download/compile. Survives frontend
   * reloads (the in-memory in-flight tracker below doesn't), so the row UI
   * can rehydrate "Preparing…" state after a Cmd+R. Whisper engine always
   * returns false here — it uses the model:progress stream instead. */
  preparing: boolean;
};

export type ModelProgress = {
  id: string;
  engine: Engine | string;
  downloaded: number;
  total: number | null;
};

export type ModelComplete = {
  id: string;
  engine: Engine | string;
};

export type EngineOpts = {
  engine?: Engine;
  modelId?: string;
};

// Backward-compat: a bare call (no args) → whisper, matching Phase 1's
// default on the Rust side. Pass `{ engine: "parakeet" }` for Parakeet.
export async function getModelStatus(opts?: EngineOpts): Promise<ModelStatus> {
  return await invoke<ModelStatus>("model_status", {
    modelId: opts?.modelId,
    engine: opts?.engine,
  });
}

export async function downloadModel(opts?: EngineOpts): Promise<string> {
  const engine: Engine = opts?.engine ?? "whisper";
  await ensureInFlightInit();
  markInFlight(engine, true);
  try {
    return await invoke<string>("download_model", {
      modelId: opts?.modelId,
      engine: opts?.engine,
    });
  } catch (e) {
    markInFlight(engine, false);
    throw e;
  }
}

// In-flight preparation tracking (module-level singleton).
//
// Background: Phase 1 emits exactly one `(0, None)` progress tick when
// prepare starts, then nothing until `model:complete`. Any component
// that mounts AFTER the tick has no way of knowing a prep is in flight
// via the event stream alone — which led to the bug where the
// ParakeetEngineRow on Home showed "Prepare engine" while the banner
// already showed "Preparing…" because onboarding had fired the prep
// before Home mounted.
//
// Fix: track in-flight engines here. `downloadModel` marks the engine
// in-flight synchronously; the module's complete-listener clears it.
// Components subscribe via `useIsPreparing(engine)` and always see the
// same answer regardless of mount order.
const inFlight = new Set<string>();
const inFlightListeners = new Set<() => void>();

function emitInFlightChange() {
  for (const l of inFlightListeners) l();
}

function markInFlight(engine: string, value: boolean) {
  const changed = value ? !inFlight.has(engine) : inFlight.delete(engine);
  if (value) inFlight.add(engine);
  if (changed) emitInFlightChange();
}

let inFlightInit: Promise<void> | null = null;
async function ensureInFlightInit(): Promise<void> {
  if (inFlightInit) return inFlightInit;
  inFlightInit = (async () => {
    await listen<ModelComplete>("model:complete", (e) => {
      markInFlight(e.payload.engine, false);
    });
  })();
  return inFlightInit;
}

export function isPreparing(engine: Engine): boolean {
  return inFlight.has(engine);
}

export function subscribeIsPreparing(cb: () => void): () => void {
  inFlightListeners.add(cb);
  return () => {
    inFlightListeners.delete(cb);
  };
}

export function useIsPreparing(engine: Engine): boolean {
  const [value, setValue] = useState<boolean>(() => isPreparing(engine));
  useEffect(() => {
    void ensureInFlightInit();
    const update = () => setValue(isPreparing(engine));
    update();
    return subscribeIsPreparing(update);
  }, [engine]);
  return value;
}

export async function deleteModel(opts?: EngineOpts): Promise<void> {
  await invoke<void>("delete_model", {
    modelId: opts?.modelId,
    engine: opts?.engine,
  });
}

export async function subscribeModelProgress(
  cb: (p: ModelProgress) => void,
): Promise<UnlistenFn> {
  return await listen<ModelProgress>("model:progress", (e) => cb(e.payload));
}

// Phase 1 broadcasts a structured payload `{ id, engine }` (not a bare
// string), so each engine's UI can filter on its own events without
// cross-talk. The legacy string-payload shape is no longer emitted.
export async function subscribeModelComplete(
  cb: (p: ModelComplete) => void,
): Promise<UnlistenFn> {
  return await listen<ModelComplete>("model:complete", (e) => cb(e.payload));
}

// ---- Model warming (loading a local model into memory on switch / app open) ----
// Distinct from download/prep above: the engine is already installed, we're just
// paying the cold-start load (sidecar spawn + multi-GB model into RAM). The chip
// bar shows a "Warming…" HUD between `model:warming` and `model:warmed`. Payload
// `provider` is the kebab wire id ("qwen3", "gemma", "fluid-audio", …) from the
// Rust `stt::Provider` enum.
export type WarmEvent = { provider: string };
export type WarmFailedEvent = { provider: string; message: string };

export async function subscribeModelWarming(
  cb: (p: WarmEvent) => void,
): Promise<UnlistenFn> {
  return await listen<WarmEvent>("model:warming", (e) => cb(e.payload));
}

export async function subscribeModelWarmed(
  cb: (p: WarmEvent) => void,
): Promise<UnlistenFn> {
  return await listen<WarmEvent>("model:warmed", (e) => cb(e.payload));
}

export async function subscribeModelWarmFailed(
  cb: (p: WarmFailedEvent) => void,
): Promise<UnlistenFn> {
  return await listen<WarmFailedEvent>("model:warm-failed", (e) => cb(e.payload));
}

/// The provider currently being warmed (or null). Queried by the chip bar on
/// mount so an app-open warm that fired before the chipbar listener was ready
/// still shows the HUD. Returns the kebab provider id, e.g. "qwen3".
export async function currentWarming(): Promise<string | null> {
  return await invoke<string | null>("current_warming");
}
