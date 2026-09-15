import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { AlertTriangle, X } from "lucide-react";
import { isTauri } from "@/lib/runtime";

type ErrorEntry = {
  id: number;
  message: string;
  at: number;
};

const HIDE_AFTER_MS = 10_000;

export function ErrorBanner() {
  const [errors, setErrors] = useState<ErrorEntry[]>([]);

  useEffect(() => {
    if (!isTauri()) return;
    let off: (() => void) | undefined;
    const idSrc = { current: 0 };
    void listen<string>("recording:error", (e) => {
      const id = ++idSrc.current;
      const msg = (e.payload || "Unknown error").toString();
      setErrors((cur) => [...cur, { id, message: msg, at: Date.now() }]);
      window.setTimeout(() => {
        setErrors((cur) => cur.filter((x) => x.id !== id));
      }, HIDE_AFTER_MS);
    })
      .then((o) => (off = o))
      .catch(() => {});
    return () => off?.();
  }, []);

  if (errors.length === 0) return null;

  return (
    <div className="fixed bottom-5 right-5 z-50 flex flex-col gap-2 max-w-sm pointer-events-none animate-vibeking-fade-up">
      {errors.map((e) => (
        <Toast
          key={e.id}
          message={e.message}
          onDismiss={() =>
            setErrors((cur) => cur.filter((x) => x.id !== e.id))
          }
        />
      ))}
    </div>
  );
}

function Toast({
  message,
  onDismiss,
}: {
  message: string;
  onDismiss: () => void;
}) {
  const summary = summarize(message);
  return (
    <div
      role="alert"
      className="pointer-events-auto flex items-start gap-2.5 rounded-lg border border-[var(--vk-danger-border-3)] bg-[var(--vk-surface)] px-3 py-2.5 shadow-[0_10px_30px_var(--vk-shadow-10),0_2px_4px_var(--vk-shadow-04)]"
    >
      <AlertTriangle className="size-4 shrink-0 text-[var(--vk-danger-2)] mt-px" />
      <div className="min-w-0 flex-1">
        <div className="text-[12.5px] font-semibold text-[var(--vk-text-2)]">
          {summary.title}
        </div>
        {summary.detail ? (
          <div className="mt-0.5 text-[11.5px] text-[var(--vk-text-6)] leading-snug break-words">
            {summary.detail}
          </div>
        ) : null}
        {summary.hint ? (
          <div className="mt-1 text-[11px] text-[var(--vk-text-8)] leading-snug">
            {summary.hint}
          </div>
        ) : null}
      </div>
      <button
        onClick={onDismiss}
        aria-label="Dismiss"
        className="size-5 inline-flex items-center justify-center rounded text-[var(--vk-text-8)] hover:bg-[var(--vk-surface-3)] hover:text-[var(--vk-text-2)] transition-colors -mr-1"
      >
        <X className="size-3.5" />
      </button>
    </div>
  );
}

function summarize(message: string): {
  title: string;
  detail?: string;
  hint?: string;
} {
  if (message.startsWith("transcribe:")) {
    const inner = message.slice("transcribe:".length).trim();
    if (inner.includes("401") || inner.toLowerCase().includes("unauthorized")) {
      return {
        title: "STT auth failed",
        detail: inner,
        hint: "Check the API Key for your STT provider in Recognition.",
      };
    }
    if (inner.includes("missing API key")) {
      return {
        title: "STT API key missing",
        detail: inner,
        hint: "Open Recognition and paste a key for the selected provider.",
      };
    }
    return { title: "Transcription failed", detail: inner };
  }
  if (message.startsWith("polish:") || message.startsWith("translate:")) {
    const inner = message.replace(/^(polish|translate):\s*/, "").trim();
    if (inner.includes("401") || inner.toLowerCase().includes("unauthorized")) {
      return {
        title: "Polish endpoint requires a key",
        detail: inner,
        hint: "Open Enhancement and paste the API Key, or start the local server without auth.",
      };
    }
    if (inner.includes("failed to reach")) {
      return {
        title: "Polish endpoint unreachable",
        detail: inner,
        hint: "Confirm the Base URL is correct and the local server is running.",
      };
    }
    if (inner.includes("missing Anthropic API key")) {
      return {
        title: "Anthropic key missing",
        hint: "Open Enhancement → API Key.",
      };
    }
    return { title: "Polish failed", detail: inner };
  }
  if (message.startsWith("audio:")) {
    return {
      title: "Microphone error",
      detail: message.slice("audio:".length).trim(),
      hint: "Check System Settings → Privacy → Microphone.",
    };
  }
  if (message.startsWith("insert:")) {
    return {
      title: "Couldn't insert text",
      detail: message.slice("insert:".length).trim(),
      hint: "Check Accessibility permission for Vibeking.",
    };
  }
  return { title: "Error", detail: message };
}
