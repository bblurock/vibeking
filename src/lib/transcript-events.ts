import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { isTauri } from "./runtime";

export type TranscriptComplete = {
  raw: string;
  text: string;
  provider: string;
  model: string;
  duration_ms: number;
  translated: boolean;
  polished: boolean;
  session: number;
};

// Every transcript event from the backend includes the recording-session
// identifier (matches state.chipbar_generation at the time of transcribe
// spawn). The chip bar filters events whose `session` is older than the
// recording it's currently visualising — without this, the previous
// recording's late-arriving "polishing"/"complete" events would overwrite
// the new recording's "recording" state.
//
// `insert-complete` and `error` aren't gated: they come from paths that
// either fire after a global atomic gate already cleared (insert) or are
// always user-relevant immediately (error).
export type TranscriptEvent =
  | { type: "started"; session: number }
  | { type: "interim"; text: string; session: number }
  | { type: "raw"; text: string; session: number }
  | { type: "polishing"; raw: string; session: number }
  | { type: "translating"; raw: string; session: number }
  | { type: "complete"; payload: TranscriptComplete }
  | {
      type: "insert-complete";
      pasted_text: string;
      original_clipboard: string | null;
      ts_ms: number;
    }
  | { type: "error"; message: string };

const NOOP: UnlistenFn = () => {};

export async function subscribeTranscript(
  handler: (event: TranscriptEvent) => void,
): Promise<UnlistenFn> {
  if (!isTauri()) return NOOP;
  try {
    const offs = await Promise.all([
      listen<{ session: number }>("transcription:start", (e) =>
        handler({ type: "started", session: e.payload?.session ?? 0 }),
      ),
      listen<{ text: string; session: number }>("transcript:interim", (e) =>
        handler({
          type: "interim",
          text: e.payload?.text ?? "",
          session: e.payload?.session ?? 0,
        }),
      ),
      listen<{ text: string; session: number }>("transcript:raw", (e) =>
        handler({
          type: "raw",
          text: e.payload?.text ?? "",
          session: e.payload?.session ?? 0,
        }),
      ),
      listen<{ raw: string; session: number }>("transcription:polishing", (e) =>
        handler({
          type: "polishing",
          raw: e.payload?.raw ?? "",
          session: e.payload?.session ?? 0,
        }),
      ),
      listen<{ raw: string; session: number }>("transcription:translating", (e) =>
        handler({
          type: "translating",
          raw: e.payload?.raw ?? "",
          session: e.payload?.session ?? 0,
        }),
      ),
      listen<TranscriptComplete>("transcript:complete", (e) =>
        handler({ type: "complete", payload: e.payload }),
      ),
      listen<{
        pasted_text: string;
        original_clipboard: string | null;
        ts_ms: number;
      }>("insert:complete", (e) =>
        handler({
          type: "insert-complete",
          pasted_text: e.payload?.pasted_text ?? "",
          original_clipboard: e.payload?.original_clipboard ?? null,
          ts_ms: e.payload?.ts_ms ?? 0,
        }),
      ),
      listen<string>("recording:error", (e) =>
        handler({ type: "error", message: e.payload ?? "Unknown error" }),
      ),
    ]);
    return () => offs.forEach((off) => off());
  } catch (e) {
    console.warn("[vibeking] subscribeTranscript failed:", e);
    return NOOP;
  }
}
