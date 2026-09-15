import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

// Live mic-level plumbing for the onboarding mic step. The Rust audio engine
// (audio.rs) opens a dedicated "monitor" stream that computes per-buffer RMS
// and emits `audio:level` at ~25 Hz — independent of the recording/transcribe
// pipeline. Mirrors the listen/invoke shape used in lib/local-model.ts.

export type AudioLevel = { rms: number };

/** Start (or re-target) the live meter on `device` ("" / undefined = system default). */
export async function startMicMonitor(device?: string): Promise<void> {
  // The Rust command takes `device: Option<String>`; "" means system default,
  // which we pass through as null so cpal resolves the default input.
  await invoke<void>("start_mic_monitor", {
    device: device && device.length > 0 ? device : null,
  });
}

/** Stop the live meter; the warm stream closes shortly after (mic indicator off). */
export async function stopMicMonitor(): Promise<void> {
  await invoke<void>("stop_mic_monitor");
}

/** Subscribe to live RMS ticks. Returns an unlisten fn. */
export async function subscribeAudioLevel(
  cb: (level: AudioLevel) => void,
): Promise<UnlistenFn> {
  return await listen<AudioLevel>("audio:level", (e) => cb(e.payload));
}
