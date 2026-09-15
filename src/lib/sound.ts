import { listen } from "@tauri-apps/api/event";
import { isTauri } from "./runtime";
import { getSettings } from "./settings";

const SOUND_URL = "/sounds/complete.wav";

let started = false;
let unlisten: (() => void) | null = null;
let element: HTMLAudioElement | null = null;
let soundFx = true;

function audio(): HTMLAudioElement {
  if (!element) {
    element = new Audio(SOUND_URL);
    element.preload = "auto";
    element.volume = 0.6;
  }
  return element;
}

function play() {
  if (!soundFx) return;
  const el = audio();
  try {
    el.currentTime = 0;
    void el.play().catch(() => {});
  } catch {
    // ignore — audio playback failures shouldn't crash the app
  }
}

export function startSoundListener() {
  if (started) return;
  if (!isTauri()) return;
  started = true;

  void getSettings()
    .then((s) => (soundFx = s.soundFx))
    .catch(() => {});

  void listen<unknown>("transcript:complete", () => play())
    .then((off) => (unlisten = off))
    .catch((err) =>
      console.warn("[vibeking] sound listener failed:", err),
    );
}

export function stopSoundListener() {
  unlisten?.();
  unlisten = null;
  started = false;
}

export function setSoundFx(on: boolean) {
  soundFx = on;
}
