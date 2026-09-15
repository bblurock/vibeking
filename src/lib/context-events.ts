import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { isTauri } from "./runtime";

export type CaptureSource = "ax" | "ocr";

export interface ContextCapturedEvent {
  candidates: string[];
  source: CaptureSource;
  bundle_id: string | null;
  app_name: string | null;
  latency_ms: number;
  raw_text_len: number;
  /** Client-side decoration — when this renderer received the event. */
  capturedAt: number;
}

const NOOP: UnlistenFn = () => {};

/**
 * Subscribe to `context:captured` events emitted by the Rust capture
 * pipeline. Returns an unlisten function. Safe to call outside Tauri
 * (returns a no-op).
 */
export function subscribeContextCaptured(
  handler: (event: ContextCapturedEvent) => void,
): () => void {
  if (!isTauri()) return NOOP;
  let unlisten: UnlistenFn | null = null;
  let cancelled = false;
  listen<Omit<ContextCapturedEvent, "capturedAt">>("context:captured", (e) => {
    handler({ ...e.payload, capturedAt: Date.now() });
  })
    .then((off) => {
      if (cancelled) off();
      else unlisten = off;
    })
    .catch((err) => {
      console.warn("[vibeking] subscribeContextCaptured failed:", err);
    });
  return () => {
    cancelled = true;
    unlisten?.();
  };
}
