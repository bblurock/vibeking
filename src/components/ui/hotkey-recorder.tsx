import { useEffect, useRef, useState } from "react";
import { cn } from "@/lib/utils";
import { isTauri } from "@/lib/runtime";
import { invoke } from "@tauri-apps/api/core";

const MOD_ONLY_COMMIT_DELAY_MS = 400;

async function setCaptureMode(on: boolean) {
  if (!isTauri()) return;
  try {
    await invoke("set_hotkey_capture", { on });
  } catch {
    // best-effort; the native listener will still work without it
  }
}

type Props = {
  value: string;
  onChange: (v: string) => void;
  className?: string;
};

const MOD_FROM_CODE: Record<string, string> = {
  ControlLeft: "left-control",
  ControlRight: "right-control",
  ShiftLeft: "left-shift",
  ShiftRight: "right-shift",
  AltLeft: "left-option",
  AltRight: "right-option",
  MetaLeft: "left-command",
  MetaRight: "right-command",
};

const MOD_GLYPH: Record<string, string> = {
  "left-control": "⌃L",
  "right-control": "⌃R",
  "left-shift": "⇧L",
  "right-shift": "⇧R",
  "left-option": "⌥L",
  "right-option": "⌥R",
  "left-command": "⌘L",
  "right-command": "⌘R",
};

const PUNCT_FROM_CODE: Record<string, string> = {
  Slash: "slash",
  Minus: "minus",
  Equal: "equal",
  BracketLeft: "bracket-left",
  BracketRight: "bracket-right",
  Semicolon: "semicolon",
  Quote: "quote",
  Comma: "comma",
  Period: "period",
  Backslash: "backslash",
  ArrowUp: "arrow-up",
  ArrowDown: "arrow-down",
  ArrowLeft: "arrow-left",
  ArrowRight: "arrow-right",
};

const KEY_DISPLAY: Record<string, string> = {
  slash: "/",
  minus: "-",
  equal: "=",
  "bracket-left": "[",
  "bracket-right": "]",
  semicolon: ";",
  quote: "'",
  comma: ",",
  period: ".",
  backslash: "\\",
  backquote: "`",
  space: "␣",
  enter: "⏎",
  tab: "⇥",
  "arrow-up": "↑",
  "arrow-down": "↓",
  "arrow-left": "←",
  "arrow-right": "→",
};

function keyFromCode(code: string): string | null {
  if (code.startsWith("Key")) return code.slice(3).toLowerCase();
  if (code.startsWith("Digit")) return code.slice(5);
  if (PUNCT_FROM_CODE[code]) return PUNCT_FROM_CODE[code];
  if (code === "Space") return "space";
  if (code === "Enter") return "enter";
  if (code === "Tab") return "tab";
  if (code === "Backquote") return "backquote";
  if (code.startsWith("F") && /^F\d{1,2}$/.test(code)) return code.toLowerCase();
  return null;
}

function modOrder(m: string): number {
  return [
    "left-control",
    "right-control",
    "left-shift",
    "right-shift",
    "left-option",
    "right-option",
    "left-command",
    "right-command",
  ].indexOf(m);
}

// Compact glyph rendering of a hotkey spec, e.g.
// "right-control+right-shift+slash" → "⌃R + ⇧R + /". Exported so the always-on
// idle bar can show custom combos the same way (its old raw-string fallback
// overflowed the fixed-width bar — see ChipBar IdleBarContent).
export function formatHotkey(spec: string): string {
  if (!spec) return "(none)";
  const parts = spec.split("+");
  return parts
    .map((p) => MOD_GLYPH[p] ?? KEY_DISPLAY[p] ?? p.toUpperCase())
    .join(" + ");
}

export function HotkeyRecorder({ value, onChange, className }: Props) {
  const [recording, setRecording] = useState(false);
  const [draftMods, setDraftMods] = useState<string[]>([]);
  const draftRef = useRef<string[]>([]);
  draftRef.current = draftMods;
  const btnRef = useRef<HTMLButtonElement>(null);
  const commitTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (!recording) {
      void setCaptureMode(false);
      return;
    }
    void setCaptureMode(true);

    const clearCommitTimer = () => {
      if (commitTimerRef.current) {
        clearTimeout(commitTimerRef.current);
        commitTimerRef.current = null;
      }
    };

    const finish = (spec: string) => {
      clearCommitTimer();
      onChange(spec);
      setRecording(false);
      setDraftMods([]);
    };

    const onDown = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      clearCommitTimer();

      if (e.code === "Escape") {
        setRecording(false);
        setDraftMods([]);
        return;
      }

      const mod = MOD_FROM_CODE[e.code];
      if (mod) {
        setDraftMods((prev) =>
          prev.includes(mod) ? prev : [...prev, mod].sort((a, b) => modOrder(a) - modOrder(b)),
        );
        return;
      }

      const key = keyFromCode(e.code);
      if (key) {
        const mods = draftRef.current;
        const spec = mods.length > 0 ? [...mods, key].join("+") : key;
        finish(spec);
      }
    };

    const onUp = (e: KeyboardEvent) => {
      const mod = MOD_FROM_CODE[e.code];
      if (!mod) return;
      // Modifier released. Defer the modifier-only commit so the user has a
      // chance to press a main key (or another modifier) without us locking
      // in the spec immediately.
      const stillAny = e.ctrlKey || e.shiftKey || e.altKey || e.metaKey;
      if (stillAny || draftRef.current.length === 0) return;
      clearCommitTimer();
      commitTimerRef.current = setTimeout(() => {
        const mods = draftRef.current;
        if (mods.length === 0) return;
        finish(mods.join("+"));
      }, MOD_ONLY_COMMIT_DELAY_MS);
    };

    window.addEventListener("keydown", onDown, true);
    window.addEventListener("keyup", onUp, true);
    return () => {
      window.removeEventListener("keydown", onDown, true);
      window.removeEventListener("keyup", onUp, true);
      clearCommitTimer();
      void setCaptureMode(false);
    };
  }, [recording, onChange]);

  const display = recording
    ? draftMods.length > 0
      ? draftMods.map((m) => MOD_GLYPH[m] ?? m).join(" + ") + " + …"
      : "Press a key combo · Esc to cancel"
    : formatHotkey(value);

  return (
    <button
      ref={btnRef}
      onClick={() => setRecording(true)}
      className={cn(
        "inline-flex items-center gap-2 h-7 px-3 rounded-md border text-[12px] font-medium transition-all min-w-[150px] justify-center",
        recording
          ? "border-[var(--vk-accent-4)] bg-[var(--vk-accent-soft-bg-3)] text-[var(--vk-accent-6)] shadow-[0_0_0_3px_var(--vk-chip-glass-shadow-3)]"
          : "border-[var(--vk-border-strong)] bg-[var(--vk-surface)] text-[var(--vk-text-2)] hover:bg-[var(--vk-canvas-3)] hover:border-[var(--vk-border-strong-2)]",
        className,
      )}
      title={recording ? "Press a key combination, or Esc to cancel" : "Click to record"}
    >
      {recording ? (
        <span className="size-1.5 rounded-full bg-[var(--vk-accent-2)] animate-vibeking-pulse" />
      ) : null}
      <span className="font-mono tabular-nums">{display}</span>
    </button>
  );
}
