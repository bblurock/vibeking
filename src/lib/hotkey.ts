import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { isTauri } from "./runtime";
import type { ActiveRefinementMode } from "./settings";

export type RecordingMode = "push-to-talk" | "toggle";

export type MicResolution = {
  requested: string | null;
  actual: string | null;
  fellBack: boolean;
};

export type RecordingEvent =
  | { type: "start" }
  | { type: "stop" }
  | { type: "cancel" }
  | { type: "mode"; mode: RecordingMode }
  | { type: "mic-resolved"; resolution: MicResolution };

const NOOP: UnlistenFn = () => {};

export async function subscribeRecording(
  handler: (event: RecordingEvent) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return NOOP;
  try {
    const unlisteners: UnlistenFn[] = await Promise.all([
      listen("recording:start", () => handler({ type: "start" })),
      listen("recording:stop", () => handler({ type: "stop" })),
      listen("recording:cancel", () => handler({ type: "cancel" })),
      listen<RecordingMode>("mode:changed", (e) =>
        handler({ type: "mode", mode: e.payload }),
      ),
      // Rust emits this AFTER recording:start once it resolves the cpal
      // device. Rename the snake_case field from serde so the rest of the
      // frontend keeps the camelCase convention.
      listen<{ requested: string | null; actual: string | null; fell_back: boolean }>(
        "recording:mic-resolved",
        (e) =>
          handler({
            type: "mic-resolved",
            resolution: {
              requested: e.payload.requested,
              actual: e.payload.actual,
              fellBack: e.payload.fell_back,
            },
          }),
      ),
    ]);
    return () => {
      for (const off of unlisteners) off();
    };
  } catch (e) {
    console.warn("[vibeking] subscribeRecording failed:", e);
    return NOOP;
  }
}

/// Click-to-record from the always-on idle bar. Toggles the shared recording
/// state in Rust, which emits the same `recording:start` / `recording:stop`
/// events as the global hotkey — so a click behaves exactly like a tap-toggle
/// of the hotkey (and Esc still cancels it).
export async function toggleRecordingFromUi(): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("ui_toggle_recording");
  } catch (e) {
    console.warn("[vibeking] toggleRecordingFromUi failed:", e);
  }
}

/// Subscribe to refinement-mode changes emitted by Rust whenever the user
/// cycles via Shift+Tab or picks a mode in Settings. Payload `mode: null`
/// represents the "Off" position in the cycle.
export async function subscribeRefinementMode(
  handler: (payload: ActiveRefinementMode) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return NOOP;
  try {
    return await listen<ActiveRefinementMode>(
      "refinement:active-changed",
      (e) => handler(e.payload),
    );
  } catch (e) {
    console.warn("[vibeking] subscribeRefinementMode failed:", e);
    return NOOP;
  }
}
