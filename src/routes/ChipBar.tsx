import { useEffect, useLayoutEffect, useRef, useState } from "react";
import {
  Mic,
  MicOff,
  Sparkles,
  Languages,
  Volume2,
  CheckCircle2,
  AlertTriangle,
  Pencil,
  Check,
  X,
  Settings2,
  ChevronDown,
  Square,
  Loader2,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  subscribeRecording,
  subscribeRefinementMode,
  toggleRecordingFromUi,
} from "@/lib/hotkey";
import { subscribeTranscript } from "@/lib/transcript-events";
import {
  subscribeModelWarming,
  subscribeModelWarmed,
  subscribeModelWarmFailed,
  currentWarming,
} from "@/lib/local-model";
import { useSettings } from "@/lib/use-settings";
import {
  RECORD_HOTKEYS,
  type CorrectionEntry,
  type CorrectionLearningMode,
  type RefinementMode,
  type Settings,
} from "@/lib/settings";
import { isTauri } from "@/lib/runtime";
import { diffWords } from "@/lib/text-diff";
import { BrandChip } from "@/components/BrandMark";
import { Kbd } from "@/components/ui/kbd";
import { formatHotkey } from "@/components/ui/hotkey-recorder";

type MicFallback = {
  requested: string;
  actual: string | null;
};

type Phase =
  | { kind: "idle" }
  // A local model is being loaded into memory (sidecar spawn + multi-GB load)
  // after a model switch or app open — show a spinner pill so the user knows
  // the next dictation just needs a moment. `provider` is the kebab wire id.
  | { kind: "warming"; provider: string }
  | {
      kind: "recording";
      mode: "push-to-talk" | "toggle" | null;
      // Populated when the requested mic wasn't found and we fell back
      // to the system default. `null` means either no fallback fired, or
      // the user explicitly chose "System default".
      micFallback: MicFallback | null;
      // Live streaming transcript (Qwen3 Chinese dictation). `null` until the
      // first partial arrives; only ever set when the local engine + Chinese
      // path is active. Other engines leave the pill as-is.
      interim: string | null;
      // True while a cold Gemma sidecar is loading after an idle-unload — show
      // a "Warming…" hint so the user understands the (brief) absence of live
      // preview. The stop-time recovery still delivers the final transcript.
      warming?: boolean;
    }
  | { kind: "recognizing"; raw: string | null }
  | { kind: "polishing"; raw: string }
  | { kind: "translating"; raw: string }
  | { kind: "done"; raw: string; final: string; translated: boolean; polished: boolean }
  | { kind: "error"; message: string }
  | { kind: "correction-prompt-ask"; from: string; to: string; pastedText: string; secondsLeft: number }
  // Auto mode accumulates corrections that fire within the same paste's
  // success-toast window, so a rapid sequence of edits surfaces as one
  // readable toast ("Learned 2: claw → Claude, hermes → Hermes") instead
  // of flashing through individual ones the user can't track.
  | { kind: "correction-success-auto"; corrections: { from: string; to: string }[] };

type Accent = { bg: string; fg: string; muted: string };

const ACCENTS: Record<string, Accent> = {
  warming: {
    bg: "var(--vk-info-soft-bg)",
    fg: "var(--vk-info-fg-2)",
    muted: "var(--vk-info-fg-3)",
  },
  recording: {
    bg: "var(--vk-accent-soft-bg-4)",
    fg: "var(--vk-accent-2)",
    muted: "var(--vk-accent-3)",
  },
  recognizing: {
    bg: "var(--vk-info-soft-bg)",
    fg: "var(--vk-info-fg-2)",
    muted: "var(--vk-info-fg-3)",
  },
  polishing: {
    bg: "var(--vk-purple-soft-bg)",
    fg: "var(--vk-purple)",
    muted: "var(--vk-purple-3)",
  },
  translating: {
    bg: "var(--vk-success-soft-bg)",
    fg: "var(--vk-success-3)",
    muted: "var(--vk-success-3)",
  },
  done: {
    bg: "var(--vk-success-soft-bg)",
    fg: "var(--vk-success-2)",
    muted: "var(--vk-success-4)",
  },
  error: {
    bg: "var(--vk-danger-soft-bg-2)",
    fg: "var(--vk-danger)",
    muted: "var(--vk-danger-7)",
  },
  "correction-prompt-ask": {
    bg: "var(--vk-purple-soft-bg)",
    fg: "var(--vk-purple-2)",
    muted: "var(--vk-purple-4)",
  },
  "correction-success-auto": {
    bg: "var(--vk-success-soft-bg)",
    fg: "var(--vk-success-2)",
    muted: "var(--vk-success-4)",
  },
};

type CorrectionDetectedPayload = {
  from: string;
  to: string;
  pasted_text: string;
  from_offset: number;
  mode: CorrectionLearningMode;
};

// Human-facing names for the warmable local engines, keyed by their kebab
// provider id (the `stt::Provider` wire form sent on model:warming events).
const PROVIDER_LABELS: Record<string, string> = {
  qwen3: "Qwen3",
  "fluid-audio": "Parakeet",
  gemma: "Gemma",
};
function providerLabel(provider: string): string {
  return PROVIDER_LABELS[provider] ?? provider;
}

const ASK_COUNTDOWN_SECONDS = 8;
const AUTO_TOAST_MS = 2000;

export function ChipBar() {
  const { t } = useTranslation();
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });
  const [settings, updateSettings] = useSettings();
  // Idle-bar refinement-mode picker open state. Lifted here (out of the bar
  // content) so the popover can render as a sibling of the morphing shell —
  // otherwise the shell's `overflow: hidden` (needed for the width morph) would
  // clip the upward-opening menu.
  const [menuOpen, setMenuOpen] = useState(false);

  // Keep a live ref to settings so the correction event handler always sees
  // the latest dictionary, even if it was registered with an older snapshot.
  const settingsRef = useRef(settings);
  useEffect(() => {
    settingsRef.current = settings;
  }, [settings]);

  // Live ref to the current phase so async event handlers (warming, audio
  // health) can branch on "am I idle / recording right now?" without being
  // re-subscribed on every phase change.
  const phaseRef = useRef(phase);
  useEffect(() => {
    phaseRef.current = phase;
  }, [phase]);

  // Mic capture health during the current recording. Driven by the Rust
  // `recording:audio-health` watcher, which only fires when the stream is
  // genuinely dead (a thinking pause keeps it `live`). `micPermission` lets us
  // show the precise reason — denied permission vs. a dead/muted device. Reset
  // to healthy on each start.
  const [audioHealth, setAudioHealth] = useState<{
    live: boolean;
    micPermission: boolean;
  }>({ live: true, micPermission: true });

  useEffect(() => {
    document.documentElement.classList.add("chipbar");
    return () => document.documentElement.classList.remove("chipbar");
  }, []);

  // Active refinement mode for the recording-pill badge. Seeded from
  // settings, then driven live by `refinement:active-changed` so the
  // user sees the Shift+Tab cycle land within ~1 frame of pressing.
  // `null` is the "Off" position in the cycle.
  const initialActiveMode: RefinementMode | null =
    settings.refinementModes.find(
      (m) => m.id === settings.activeRefinementModeId,
    ) ?? null;
  const [activeMode, setActiveMode] = useState<RefinementMode | null>(
    initialActiveMode,
  );

  useEffect(() => {
    // Resync the local active mode when settings load lazily — the initial
    // state is computed from a possibly-stale cached snapshot.
    const next =
      settings.refinementModes.find(
        (m) => m.id === settings.activeRefinementModeId,
      ) ?? null;
    setActiveMode(next);
  }, [settings.refinementModes, settings.activeRefinementModeId]);

  useEffect(() => {
    let off: (() => void) | undefined;
    void subscribeRefinementMode((payload) => {
      setActiveMode(payload.mode);
    }).then((o) => (off = o));
    return () => off?.();
  }, []);

  // Session counter — increments on every recording:start. Transcript
  // events from the backend include the session they belong to; any
  // event whose session is older than what we've seen is dropped on the
  // floor, so the previous recording's polish/complete events can't
  // overwrite the new recording's "recording" / "recognizing" phase.
  // Parakeet's sub-second STT exposed this race trivially — the polish
  // step of recording N would still be running when the user fired
  // recording N+1.
  const sessionRef = useRef(0);

  useEffect(() => {
    // `listen` is async, so without this guard a StrictMode (dev) mount→
    // unmount→mount leaks the first subscription: cleanup runs before the
    // promise resolves, leaving a duplicate listener. That made every
    // recording increment sessionRef TWICE, racing it ahead of the backend
    // generation so the stale-session filter dropped every transcript event
    // (raw / interim / complete). The guard unsubscribes late-resolving
    // listeners that belong to an already-cleaned-up effect.
    let cancelled = false;
    let off1: (() => void) | undefined;
    let off2: (() => void) | undefined;

    void subscribeRecording((e) => {
      switch (e.type) {
        case "start":
          sessionRef.current += 1;
          setAudioHealth({ live: true, micPermission: true });
          // Carry a pre-recording "warming" state (a model:warming that landed
          // just before recording:start, e.g. the Gemma cold-start prewarm) into
          // the recording phase so the "Warming…" hint isn't lost to the race.
          setPhase((p) => ({
            kind: "recording",
            mode: null,
            micFallback: null,
            interim: null,
            warming: p.kind === "warming",
          }));
          break;
        case "stop":
          // Carry the last live text into the recognizing phase so it stays
          // visible through processing (no blank flash), until the final
          // transcript:raw replaces it.
          setPhase((p) => ({
            kind: "recognizing",
            raw: p.kind === "recording" ? p.interim : null,
          }));
          break;
        case "cancel":
          sessionRef.current += 1;
          setPhase({ kind: "idle" });
          break;
        case "mode":
          // The backend emits mode:changed AFTER recording:start once it
          // can classify a press: quick tap → "toggle", long hold → no
          // emit (just left as the default recording state). We only
          // surface the toggle label — push-to-talk is the implicit
          // default and shouldn't need its own banner.
          setPhase((p) =>
            p.kind === "recording" ? { ...p, mode: e.mode } : p,
          );
          break;
        case "mic-resolved":
          // Backend fires this right after the cpal stream opens. Only
          // surface a hint when the user's saved device wasn't found
          // (fellBack = true) — the common case where requested === actual
          // is silent. If we've already advanced past recording (e.g. very
          // fast quick-tap → stop), the badge is moot, drop it.
          if (!e.resolution.fellBack) break;
          setPhase((p) =>
            p.kind === "recording"
              ? {
                  ...p,
                  micFallback: {
                    requested: e.resolution.requested ?? "",
                    actual: e.resolution.actual,
                  },
                }
              : p,
          );
          break;
      }
    }).then((o) => {
      if (cancelled) o();
      else off1 = o;
    });

    // Minimum visible time for the polish/translate "Vibing" phases.
    // Local mlx-lm returns in ~5-30 ms, which is faster than any human
    // can perceive a state change — without a floor the user sees the
    // chip go recognizing → done and misses the refinement step
    // entirely. 350 ms is a comfortable read time without feeling laggy.
    const POLISH_MIN_DWELL_MS = 350;
    let polishEnteredAt: number | null = null;
    let pendingComplete: ReturnType<typeof setTimeout> | null = null;

    void subscribeTranscript((e) => {
      // Stale-session filter. Events lacking a session field (legacy
      // insert-complete, error) fall through.
      const evSession =
        e.type === "started" ||
        e.type === "interim" ||
        e.type === "raw" ||
        e.type === "polishing" ||
        e.type === "translating"
          ? e.session
          : e.type === "complete"
            ? e.payload.session
            : null;
      if (evSession !== null && evSession < sessionRef.current) {
        return;
      }

      // A new recording's events should invalidate any deferred
      // "complete" we were holding back for dwell — that one was for
      // the previous recording and is no longer interesting.
      if (
        pendingComplete &&
        (e.type === "started" || e.type === "raw")
      ) {
        clearTimeout(pendingComplete);
        pendingComplete = null;
        polishEnteredAt = null;
      }

      switch (e.type) {
        case "started":
          setPhase((p) =>
            p.kind === "recognizing" ? p : { kind: "recognizing", raw: null },
          );
          break;
        case "interim":
          // Live streaming partial. During recording it's the preview card;
          // after stop (kind "recognizing") it's the streamed final tokens
          // (Gemma 12B), surfaced as the recognizing text so the result flows
          // in live instead of popping in as a blob.
          setPhase((p) => {
            if (p.kind === "recording") return { ...p, interim: e.text };
            if (p.kind === "recognizing") return { kind: "recognizing", raw: e.text };
            return p;
          });
          break;
        case "raw":
          setPhase({ kind: "recognizing", raw: e.text });
          break;
        case "polishing":
          polishEnteredAt = Date.now();
          setPhase({ kind: "polishing", raw: e.raw });
          break;
        case "translating":
          polishEnteredAt = Date.now();
          setPhase({ kind: "translating", raw: e.raw });
          break;
        case "complete": {
          const applyComplete = () => {
            polishEnteredAt = null;
            pendingComplete = null;
            setPhase({
              kind: "done",
              raw: e.payload.raw,
              final: e.payload.text,
              polished: e.payload.polished,
              translated: e.payload.translated,
            });
          };
          const elapsed =
            polishEnteredAt !== null ? Date.now() - polishEnteredAt : Infinity;
          if (elapsed < POLISH_MIN_DWELL_MS) {
            if (pendingComplete) clearTimeout(pendingComplete);
            pendingComplete = setTimeout(
              applyComplete,
              POLISH_MIN_DWELL_MS - elapsed,
            );
          } else {
            applyComplete();
          }
          break;
        }
        case "error":
          if (pendingComplete) {
            clearTimeout(pendingComplete);
            pendingComplete = null;
          }
          setPhase({ kind: "error", message: e.message });
          break;
      }
    }).then((o) => {
      if (cancelled) o();
      else off2 = o;
    });

    return () => {
      cancelled = true;
      off1?.();
      off2?.();
      if (pendingComplete) clearTimeout(pendingComplete);
    };
  }, []);

  // Persist a correction to the live settings cache. Reads from settingsRef
  // so concurrent updates elsewhere aren't clobbered.
  async function persistCorrection(from: string, to: string) {
    if (!from || !to || from === to) return;
    const entry: CorrectionEntry = {
      from,
      to,
      learnedAtMs: Date.now(),
    };
    const current = settingsRef.current.correctionDictionary ?? [];
    console.log(
      "[vibeking chipbar] persistCorrection",
      { from, to, newDictSize: current.length + 1 },
    );
    await updateSettings({ correctionDictionary: [...current, entry] });
  }

  // Pending auto-keep timeout, decoupled from React phase state. Without
  // this, a new `recording:start` overwrites phase=correction-prompt-ask
  // before the countdown useEffect can call persistCorrection, and the
  // learned entry is lost. The timeout below survives phase changes and
  // only gets cancelled when the user explicitly clicks Keep / Discard.
  const pendingAutoKeepRef = useRef<{
    timeoutId: number;
    from: string;
    to: string;
  } | null>(null);

  function cancelPendingAutoKeep() {
    const pending = pendingAutoKeepRef.current;
    if (pending) {
      window.clearTimeout(pending.timeoutId);
      pendingAutoKeepRef.current = null;
    }
  }

  function schedulePendingAutoKeep(from: string, to: string) {
    cancelPendingAutoKeep();
    const timeoutId = window.setTimeout(() => {
      pendingAutoKeepRef.current = null;
      void persistCorrection(from, to);
      // The auto-keep timer is the sole owner of the dismiss action for the
      // ask-mode prompt. The visual countdown only updates the label.
      setPhase((p) => (p.kind === "correction-prompt-ask" ? { kind: "idle" } : p));
    }, ASK_COUNTDOWN_SECONDS * 1000);
    pendingAutoKeepRef.current = { timeoutId, from, to };
  }

  // Subscribe to correction:detected from the T11 watcher. Branches on
  // payload.mode to drive Ask vs Auto UX.
  useEffect(() => {
    if (!isTauri()) return;
    let off: UnlistenFn | undefined;
    let cancelled = false;

    void listen<CorrectionDetectedPayload>("correction:detected", (e) => {
      const p = e.payload;
      if (!p) return;
      // Defensive: detector shouldn't emit in Off mode, but ignore if it does.
      if (p.mode === "off") return;
      // Defensive: never store a no-op swap.
      if (!p.from || !p.to || p.from === p.to) return;

      if (p.mode === "auto") {
        void persistCorrection(p.from, p.to);
        setPhase((prev) => {
          const newEntry = { from: p.from, to: p.to };
          if (prev.kind === "correction-success-auto") {
            return {
              kind: "correction-success-auto",
              corrections: [...prev.corrections, newEntry],
            };
          }
          return { kind: "correction-success-auto", corrections: [newEntry] };
        });
        return;
      }

      // Ask mode — schedule the auto-keep persist OUTSIDE the React render
      // cycle (survives phase changes from a new recording etc.), and show
      // the prompt for the user to optionally Keep/Edit/Discard.
      //
      // If a previous prompt is still pending (user didn't act on it before
      // a new correction fired in the same 20s window), commit it now. The
      // user's non-action is treated as implicit Keep — same contract as the
      // 8s auto-keep timeout. Without this, sequential corrections in one
      // paste would silently lose every correction except the last.
      const previousPending = pendingAutoKeepRef.current;
      if (previousPending) {
        void persistCorrection(previousPending.from, previousPending.to);
      }
      schedulePendingAutoKeep(p.from, p.to);
      setPhase({
        kind: "correction-prompt-ask",
        from: p.from,
        to: p.to,
        pastedText: p.pasted_text,
        secondsLeft: ASK_COUNTDOWN_SECONDS,
      });
    })
      .then((o) => {
        if (cancelled) {
          o();
        } else {
          off = o;
        }
      })
      .catch((err) => {
        console.warn("[vibeking] correction:detected listen failed:", err);
      });

    return () => {
      cancelled = true;
      off?.();
    };
    // persistCorrection closes over updateSettings + settingsRef (a ref),
    // both stable across renders — no dep changes needed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Ask-mode countdown. Purely visual — decrements secondsLeft for the
  // chip's "(Ns)" label. Dismissal is owned by schedulePendingAutoKeep
  // (the ref-based timer) which also handles persistence; setting
  // secondsLeft to 0 (e.g. when the user clicks Edit) just hides the
  // label, it does NOT dismiss the chip on its own.
  useEffect(() => {
    if (phase.kind !== "correction-prompt-ask") return;
    const id = window.setInterval(() => {
      setPhase((p) => {
        if (p.kind !== "correction-prompt-ask") return p;
        if (p.secondsLeft <= 0) {
          window.clearInterval(id);
          return p;
        }
        return { ...p, secondsLeft: p.secondsLeft - 1 };
      });
    }, 1000);
    return () => window.clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase.kind]);

  // Auto-mode success toast auto-dismiss. Keyed on corrections.length so a
  // new entry appended to the batch refreshes the 2s timer — the user gets
  // a fresh window to read the updated toast instead of losing it because
  // the previous timer fired mid-update.
  const autoCorrectionsCount =
    phase.kind === "correction-success-auto" ? phase.corrections.length : 0;
  useEffect(() => {
    if (phase.kind !== "correction-success-auto") return;
    const id = window.setTimeout(() => {
      setPhase({ kind: "idle" });
    }, AUTO_TOAST_MS);
    return () => window.clearTimeout(id);
  }, [phase.kind, autoCorrectionsCount]);

  // When a correction phase transitions back to idle, hide the chipbar
  // window. The Rust side shows it when correction:detected fires; the JS
  // side hides it when the user resolves the prompt (or the auto toast
  // finishes). Recording-flow phases are hidden from Rust, not from here.
  const previousPhaseKindRef = useRef(phase.kind);
  useEffect(() => {
    const previous = previousPhaseKindRef.current;
    const wasCorrection =
      previous === "correction-prompt-ask" ||
      previous === "correction-success-auto";
    if (phase.kind === "idle" && wasCorrection && isTauri()) {
      void invoke("chipbar_hide").catch(() => {});
    }
    previousPhaseKindRef.current = phase.kind;
  }, [phase.kind]);

  // Persistent idle bar fallback. When `recording:stop` (or an error / silent
  // clip) would normally hide the chip bar, Rust instead resizes to the idle
  // footprint and emits `chipbar:reset` so we drop the lingering done/error
  // phase and re-render the idle bar.
  useEffect(() => {
    if (!isTauri()) return;
    let off: UnlistenFn | undefined;
    let cancelled = false;
    void listen("chipbar:reset", () => {
      setPhase({ kind: "idle" });
    })
      .then((o) => {
        if (cancelled) o();
        else off = o;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      off?.();
    };
  }, []);

  // Mic liveness during recording. The Rust watcher emits recording:audio-health
  // { live, micPermission } only when the stream is genuinely dead (a thinking
  // pause keeps it `live`, so no false alarms). We only render the warning while
  // recording, so stale events between takes are harmless.
  useEffect(() => {
    if (!isTauri()) return;
    let off: UnlistenFn | undefined;
    let cancelled = false;
    void listen<{ live: boolean; micPermission?: boolean }>(
      "recording:audio-health",
      (e) => {
        setAudioHealth({
          live: e.payload.live,
          micPermission: e.payload.micPermission ?? true,
        });
      },
    )
      .then((o) => {
        if (cancelled) o();
        else off = o;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      off?.();
    };
  }, []);

  // Model warming HUD. Show a spinner pill while a local model loads into memory
  // (switch / app open), driven by the Rust warm orchestrator. Never interrupt a
  // recording or result card — warming only takes over from the idle state.
  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    const offs: UnlistenFn[] = [];
    const track = (p: Promise<UnlistenFn>) =>
      void p
        .then((o) => {
          if (cancelled) o();
          else offs.push(o);
        })
        .catch(() => {});

    track(
      subscribeModelWarming(({ provider }) => {
        const cur = phaseRef.current;
        // A Gemma cold-start warm fires DURING recording — surface it as a hint
        // on the recording chip instead of the standalone idle warming HUD.
        if (cur.kind === "recording") {
          setPhase({ ...cur, warming: true });
          return;
        }
        if (cur.kind !== "idle") return;
        setPhase({ kind: "warming", provider });
        // The window may be hidden (always-on bar off); make sure it's visible
        // so the HUD actually shows.
        void invoke("chipbar_show").catch(() => {});
      }),
    );
    track(
      subscribeModelWarmed(() => {
        const cur = phaseRef.current;
        if (cur.kind === "recording") {
          setPhase({ ...cur, warming: false });
          return;
        }
        if (cur.kind !== "warming") return;
        setPhase({ kind: "idle" });
        // Falls back to the idle bar (always-on) or hides the window.
        void invoke("chipbar_hide").catch(() => {});
      }),
    );
    track(
      subscribeModelWarmFailed(({ provider }) => {
        if (phaseRef.current.kind !== "warming") return;
        setPhase({
          kind: "error",
          message: t("chipbar.warmFailed", { model: providerLabel(provider) }),
        });
      }),
    );

    // Catch an app-open warm whose event fired before this listener mounted.
    void currentWarming()
      .then((provider) => {
        if (cancelled || !provider) return;
        if (phaseRef.current.kind !== "idle") return;
        setPhase({ kind: "warming", provider });
        void invoke("chipbar_show").catch(() => {});
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      offs.forEach((o) => o());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Dismiss the chip bar: clear the phase and tell Rust to hide (or, with the
  // always-on bar, fall back to the idle bar). Used by the error state — both
  // an auto-timeout and a click — so a failed dictation doesn't sit on screen
  // until the next recording.
  function dismissChip() {
    setPhase({ kind: "idle" });
    if (isTauri()) void invoke("chipbar_hide").catch(() => {});
  }

  // Errors used to linger with no way out except starting another recording.
  // Auto-dismiss after a readable pause; the card is also click-to-dismiss.
  useEffect(() => {
    if (phase.kind !== "error") return;
    const id = window.setTimeout(dismissChip, 4500);
    return () => window.clearTimeout(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase.kind]);

  // Grow the always-on bar's window only when the content actually needs the
  // height — the mode picker, a live transcript, or a result/error card. The
  // idle ↔ recording-compact morph keeps the compact window, so it happens
  // entirely in HTML with no window resize (no flicker, bottom pinned).
  // The mode picker is now its OWN window (no longer grows the chip window), so
  // it's deliberately NOT a reason to go tall here — only real in-bar content is.
  const needsTall =
    phase.kind === "recognizing" ||
    phase.kind === "polishing" ||
    phase.kind === "translating" ||
    phase.kind === "done" ||
    phase.kind === "error" ||
    (phase.kind === "recording" &&
      !!phase.interim &&
      phase.interim.trim().length > 0);
  useEffect(() => {
    if (!isTauri() || !settings.persistentBar) return;
    void invoke("chipbar_grow", { tall: needsTall }).catch(() => {});
  }, [needsTall, settings.persistentBar]);

  // Keep the chip's open-state in sync when the picker window closes itself
  // (outside-click blur, Escape, or a pick) so the chevron / toggle stay right.
  useEffect(() => {
    if (!isTauri()) return;
    let un: UnlistenFn | undefined;
    void listen("modepicker:closed", () => setMenuOpen(false)).then((u) => {
      un = u;
    });
    return () => un?.();
  }, []);

  // The mode picker is an idle-only affordance — close its window the moment a
  // recording / result takes over.
  useEffect(() => {
    if (phase.kind !== "idle") {
      setMenuOpen(false);
      if (isTauri()) void invoke("hide_modepicker").catch(() => {});
    }
  }, [phase.kind]);

  // Model warming HUD — a compact spinner pill. Its own branch (not the morph
  // shell) since it's a transient overlay shown on top of whatever the bar's
  // window happens to be sized to.
  if (phase.kind === "warming") {
    return (
      <div className="fixed inset-0 grid items-end justify-center pb-[18px] px-4 pointer-events-none">
        <div className="pointer-events-auto animate-vibeking-chip-in">
          <WarmingPill
            label={t("chipbar.warming", { model: providerLabel(phase.provider) })}
          />
        </div>
      </div>
    );
  }

  // Idle bar + recording chip share ONE morphing shell, so the idle bar
  // visually morphs into the recording bar (width / height / radius / tint
  // animate; the child content crossfades) instead of one vanishing and the
  // other popping in. The mode picker is its own window (see ModePickerWindow),
  // opened via `show_modepicker`, so it never resizes this window.
  if (phase.kind === "idle" || phase.kind === "recording") {
    if (phase.kind === "idle" && !settings.persistentBar) return null;
    const variant = phase.kind === "recording" ? "recording" : "idle";
    return (
      <div className="fixed inset-0 grid items-end justify-center pb-[18px] px-4 pointer-events-none">
        <div className="relative pointer-events-auto animate-vibeking-chip-in">
          <MorphShell variant={variant}>
            {phase.kind === "recording" ? (
              <RecordingContent
                label={
                  phase.mode === "toggle"
                    ? t("chipbar.toggleRecording")
                    : t("app.recording")
                }
                toggle={phase.mode === "toggle"}
                activeMode={activeMode}
                offLabel={t("chipbar.refinementOff")}
                interim={phase.interim}
                warmingLabel={phase.warming ? t("chipbar.warmingShort") : null}
                audioWarning={
                  audioHealth.live
                    ? null
                    : audioHealth.micPermission
                      ? { text: t("chipbar.noAudio") }
                      : {
                          text: t("chipbar.micDenied"),
                          onClick: () =>
                            void invoke("open_settings_pane", {
                              pane: "microphone",
                            }).catch(() => {}),
                        }
                }
                // Latched (toggle) recordings can be stopped with a click on
                // the mic — the mouse counterpart to tapping the hotkey again.
                onStop={
                  phase.mode === "toggle"
                    ? () => void toggleRecordingFromUi()
                    : undefined
                }
                stopHint={t("chipbar.idleStop")}
                micFallback={phase.micFallback}
                fallbackHint={
                  phase.micFallback
                    ? t("chipbar.micFallback", {
                        actual:
                          phase.micFallback.actual ??
                          t("home.microphoneSystem"),
                        requested: phase.micFallback.requested,
                      })
                    : null
                }
              />
            ) : (
              <IdleBarContent
                settings={settings}
                activeMode={activeMode}
                menuOpen={menuOpen}
                onModeClick={() => {
                  // Toggle the picker WINDOW (see ModePickerWindow). Tracked
                  // locally so the chevron reflects open state and a second
                  // click closes it; the picker also reports `modepicker:closed`.
                  setMenuOpen((open) => {
                    const next = !open;
                    void invoke(
                      next ? "show_modepicker" : "hide_modepicker",
                    ).catch(() => {});
                    return next;
                  });
                }}
                onRecord={() => void toggleRecordingFromUi()}
                onOpenApp={() =>
                  void invoke("focus_main_window").catch(() => {})
                }
                onClose={() => void updateSettings({ persistentBar: false })}
              />
            )}
          </MorphShell>
        </div>
      </div>
    );
  }

  // Tall cards: processing / result / error / correction prompts.
  return (
    <div className="fixed inset-0 grid items-end justify-center pb-[18px] px-4 pointer-events-none">
      <div className="pointer-events-auto animate-vibeking-chip-in">
        {phase.kind === "correction-prompt-ask" ? (
          <CorrectionPromptCard
            phase={phase}
            onKeep={(from, to) => {
              cancelPendingAutoKeep();
              void persistCorrection(from, to);
              setPhase({ kind: "idle" });
            }}
            onDiscard={() => {
              cancelPendingAutoKeep();
              setPhase({ kind: "idle" });
            }}
            onEditEnter={() => {
              // Any button click means the user is taking control — cancel
              // the auto-keep timer and stop the visible countdown so the
              // chip doesn't disappear while they're typing.
              cancelPendingAutoKeep();
              setPhase((p) =>
                p.kind === "correction-prompt-ask"
                  ? { ...p, secondsLeft: 0 }
                  : p,
              );
            }}
          />
        ) : phase.kind === "correction-success-auto" ? (
          <CorrectionSuccessCard corrections={phase.corrections} />
        ) : (
          <Card phase={phase} t={t} onDismiss={dismissChip} />
        )}
      </div>
    </div>
  );
}

// One persistent glass shell that animates its own width / height / corner
// radius / tint between the idle and recording states. Width and height come
// from a measured ref so they can transition (CSS can't animate `auto`); a
// ResizeObserver keeps the shell fitted as live text grows. The keyed inner
// content remounts on variant change, replaying the crossfade-in.
function MorphShell({
  variant,
  children,
}: {
  variant: "idle" | "recording";
  children: React.ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [dims, setDims] = useState<{ w: number; h: number } | null>(null);
  // The size transition (the morph) must fire ONLY on a variant change. Content
  // growth within a variant — the live transcript streaming in — has to resize
  // instantly, or the transition lags behind and `overflow:hidden` clips the
  // card's bottom on every update. So we arm `animate` briefly on each variant
  // flip and disarm it once the morph has settled.
  const [animate, setAnimate] = useState(false);
  const prevVariant = useRef(variant);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    // offsetWidth/Height are LAYOUT sizes — unaffected by ancestor transforms.
    // getBoundingClientRect would include the entrance `scale(0.94)`, so on the
    // first toggle-on the shell would be measured at 94% and crop the content.
    const measure = () => setDims({ w: el.offsetWidth, h: el.offsetHeight });
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [variant]);
  useLayoutEffect(() => {
    if (prevVariant.current === variant) return;
    prevVariant.current = variant;
    setAnimate(true);
    const id = window.setTimeout(() => setAnimate(false), 360);
    return () => window.clearTimeout(id);
  }, [variant]);
  const tall = (dims?.h ?? 0) > 80;
  return (
    <div
      className="vk-glass-bar morph-shell"
      data-variant={variant}
      data-animate={animate ? "true" : "false"}
      style={{
        width: dims?.w,
        height: dims?.h,
        borderRadius: tall ? 22 : 999,
      }}
    >
      <div ref={ref} key={variant} className="morph-content">
        {children}
      </div>
    </div>
  );
}

// Recording chip CONTENT only (the glass shell is provided by MorphShell).
function RecordingContent({
  label,
  toggle = false,
  activeMode,
  offLabel,
  interim,
  warmingLabel,
  micFallback,
  fallbackHint,
  onStop,
  stopHint,
  audioWarning,
}: {
  label: string;
  toggle?: boolean;
  activeMode: RefinementMode | null;
  offLabel: string;
  interim: string | null;
  // Translated "Warming…" hint shown while a cold Gemma sidecar loads (no live
  // preview yet). `null` when not warming.
  warmingLabel?: string | null;
  micFallback: MicFallback | null;
  fallbackHint: string | null;
  onStop?: () => void;
  stopHint?: string;
  audioWarning?: { text: string; onClick?: () => void } | null;
}) {
  // Toggle mode reuses the recording pill but gets a distinct accent
  // (purple) and the label flips to the toggle-stop hint, so the user
  // can tell at a glance that the recording is latched and will only
  // stop on the next press.
  const accent = toggle ? ACCENTS.polishing : ACCENTS.recording;

  // Shown only when the Rust watcher reports the mic stream is genuinely dead
  // (never on a thinking pause) — so the user fixes a muted/denied/wrong mic
  // instead of dictating into the void. Clickable when it's a permission issue
  // (opens System Settings).
  const noAudioWarning = audioWarning ? (
    <button
      type="button"
      onClick={audioWarning.onClick}
      disabled={!audioWarning.onClick}
      className={`inline-flex items-center gap-1 text-[10.5px] font-medium animate-vibeking-fade-up text-left ${
        audioWarning.onClick ? "hover:underline cursor-pointer" : "cursor-default"
      }`}
      style={{ color: "var(--vk-danger)" }}
    >
      <MicOff className="size-3 shrink-0" />
      <span className="truncate">{audioWarning.text}</span>
    </button>
  ) : null;

  // Re-key the badge by mode id so React remounts it on cycle. The
  // remount drives the brief opacity fade-in defined in
  // `animate-vibeking-fade-up` (already used elsewhere in the chip bar)
  // so the user can see the Shift+Tab change land instead of the badge
  // silently swapping characters in place.
  const badgeKey = activeMode?.id ?? "off";

  // Live streaming transcript. Once partials arrive, the compact pill grows
  // into a text card that visualizes what the user has said in real time.
  const hasLive = !!interim && interim.trim().length > 0;
  const liveRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    // Keep the newest words in view as the transcript grows.
    const el = liveRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [interim]);

  const modeBadge = (
    <span
      key={badgeKey}
      className={`ml-auto inline-flex items-center gap-1 px-2 py-[3px] rounded-full text-[11px] font-medium tabular-nums animate-vibeking-fade-up ${
        activeMode ? "vk-glass-control" : ""
      }`}
      style={{
        color: activeMode ? "var(--vk-glass-ink)" : "var(--vk-glass-ink-2)",
      }}
      title="Shift+Tab to cycle"
    >
      {activeMode ? (
        <>
          <span aria-hidden>{activeMode.emoji}</span>
          <span>{activeMode.name}</span>
        </>
      ) : (
        <span>{offLabel}</span>
      )}
    </span>
  );

  // When the recording is latched (toggle mode), the mic badge doubles as a
  // stop button — Mic by default, a stop square on hover — so a mouse user can
  // end the recording without reaching for the keyboard.
  const micBadge = onStop ? (
    <button
      type="button"
      onClick={onStop}
      title={stopHint}
      className="group size-7 rounded-full grid place-items-center shrink-0 hover:brightness-105 active:scale-95 transition"
      style={{ background: accent.bg, color: accent.fg }}
    >
      <Mic className="size-3.5 group-hover:hidden" />
      <Square className="size-3 hidden group-hover:block fill-current" />
    </button>
  ) : (
    <div
      className="size-7 rounded-full grid place-items-center shrink-0"
      style={{ background: accent.bg, color: accent.fg }}
    >
      <Mic className="size-3.5" />
    </div>
  );

  // Expanded live-transcript layout.
  if (hasLive) {
    return (
      <div className="flex flex-col gap-2 w-[460px] max-w-[92vw] px-4 py-3">
        <div className="flex items-center gap-2.5">
          {micBadge}
          <Equalizer accent={accent.fg} />
          <span
            className="text-[12.5px] font-medium tabular-nums"
            style={{ color: toggle ? accent.fg : "var(--vk-text-2)" }}
          >
            {label}
          </span>
          {modeBadge}
        </div>
        {noAudioWarning}
        <div
          ref={liveRef}
          // ~9-10 lines — fills the 340px chip-bar window above the header row.
          // Auto-scrolls to the newest text (see effect below) so the user
          // always sees what they just said, not a clipped 2-line sliver. (The
          // window height is set in tauri.conf.json; both must move together.)
          className="max-h-[230px] overflow-hidden text-[14px] leading-[1.7] italic whitespace-pre-wrap break-words"
          style={{ color: "var(--vk-text-3)" }}
        >
          <LiveTranscript text={interim ?? ""} />
        </div>
      </div>
    );
  }

  // Compact recording row (no live text yet, or a non-streaming engine).
  // Matches the idle bar's height so the morph keeps a constant bar height.
  return (
    <div className="flex items-center gap-2.5 h-11 px-3.5 min-w-[240px]">
      {micBadge}
      <Equalizer accent={accent.fg} />
      <div className="flex flex-col leading-tight min-w-0 max-w-[440px]">
        <span
          className="text-[12.5px] font-medium tabular-nums"
          style={{ color: toggle ? accent.fg : "var(--vk-text-2)" }}
        >
          {label}
        </span>
        {noAudioWarning}
        {warmingLabel && !hasLive ? (
          <span
            className="inline-flex items-center gap-1 text-[10.5px] animate-vibeking-fade-up"
            style={{ color: "var(--vk-info-fg-3)" }}
          >
            <Loader2 className="size-3 shrink-0 animate-spin" />
            <span className="truncate">{warmingLabel}</span>
          </span>
        ) : micFallback && fallbackHint ? (
          <span
            className="text-[10.5px] truncate"
            style={{ color: "var(--vk-text-9)" }}
            title={fallbackHint}
          >
            {fallbackHint}
          </span>
        ) : null}
      </div>
      {modeBadge}
    </div>
  );
}

// Perceptual streaming layer. The backend hands us a fresh interim transcript
// roughly once a second (Qwen3 native, or the universal sliding-window). Painted
// naively that lands as a jarring ~1 s batch swap. Instead we diff each update
// against the last, hold the unchanged STABLE PREFIX solid, and reveal only the
// newly-arrived tokens with a short staggered blur-in — turning one 1 s chunk of
// N words into a ~300 ms typewriter reveal. Same data, every engine, no new
// runtime: it just makes chunks *feel* like flowing word-by-word typing.
//
// Tokenization is script-aware: CJK characters each become their own token
// (char-by-char reveal, the natural granularity for Chinese/Japanese), while
// space-delimited scripts reveal word-by-word. Tokens carry their trailing
// whitespace so reconstruction is lossless under `whitespace-pre-wrap`.
const CJK_RANGES =
  "\\u3400-\\u4dbf\\u4e00-\\u9fff\\u3040-\\u30ff\\uac00-\\ud7af\\u3000-\\u303f\\uff00-\\uffef";
const TOKEN_RE = new RegExp(
  `[${CJK_RANGES}]\\s*|[^\\s${CJK_RANGES}]+\\s*|\\s+`,
  "gu",
);

function tokenizeLive(text: string): string[] {
  return text.match(TOKEN_RE) ?? [];
}

// Opacity of the still-settling "live tail" (tokens not yet stable across
// paints). They render dimmed and fade up to full strength as the model
// confirms them — a calm dictation feel that replaces the old per-word blur-in
// "pop", which read as flicker during fast streaming (esp. Parakeet/Qwen3).
const LIVE_TAIL_OPACITY = 0.4;
// Hard cap on rendered tokens. The backend accumulates the full recording, but
// only the last few lines are visible — so cap DOM work for long dictations.
const MAX_RENDER_TOKENS = 260;

function LiveTranscript({ text }: { text: string }) {
  const tokens = tokenizeLive(text);

  // Longest common prefix (by token) with the previous paint. Everything up to
  // `prefix` is committed and renders solid with no animation; tokens at or
  // beyond it are "fresh" this paint and blur in. Keying spans by absolute
  // index keeps committed tokens mounted across updates, so their one-shot
  // CSS animation never replays — only genuinely new tokens animate.
  const prevRef = useRef<string[]>([]);
  const prev = prevRef.current;
  let prefix = 0;
  while (
    prefix < tokens.length &&
    prefix < prev.length &&
    tokens[prefix] === prev[prefix]
  ) {
    prefix += 1;
  }
  useEffect(() => {
    prevRef.current = tokens;
  });

  // Settle on pause. A word only goes solid once a *later* paint repeats it
  // unchanged — but when the user stops talking, no new partial arrives, so the
  // last word(s) would sit dimmed forever. After a short idle with no text
  // change, fade the whole tail up to solid (the phrase has clearly settled).
  // Resets on every change, so it never fires mid-speech.
  const [settled, setSettled] = useState(false);
  useEffect(() => {
    setSettled(false);
    const id = window.setTimeout(() => setSettled(true), 500);
    return () => window.clearTimeout(id);
  }, [text]);

  // The backend now accumulates the whole recording into `text`, so for long
  // dictation this can be thousands of tokens. Only the last few lines are ever
  // visible (max-height + auto-scroll-to-newest), so render just the tail —
  // bounds DOM/animation work without changing what the user sees. Diffing
  // above still runs on the full arrays so the stable prefix stays correct.
  const renderStart = Math.max(0, tokens.length - MAX_RENDER_TOKENS);

  return (
    <>
      {tokens.slice(renderStart).map((tok, idx) => {
        const i = renderStart + idx;
        // Tokens at/after the common-prefix boundary are still "live" this paint
        // (the model is still settling them). Render them dimmed; as they
        // stabilize across paints the prefix grows past them and they fade up to
        // full strength via the opacity transition — words slide in calmly
        // instead of popping, so there's no per-word flicker.
        const live = !settled && i >= prefix;
        return (
          // No per-span white-space override: spans inherit the parent's
          // `whitespace-pre-wrap` so tokens (with baked-in trailing space) wrap
          // at the card edge.
          <span
            key={i}
            style={{
              opacity: live ? LIVE_TAIL_OPACITY : 1,
              transition: "opacity 240ms ease-out",
            }}
          >
            {tok}
          </span>
        );
      })}
      <span
        aria-hidden
        className="inline-block align-[-0.1em] ml-[1px] w-[2px] h-[1em] rounded-full"
        style={{
          background: "var(--vk-accent-2)",
          animation: "vibeking-cursor-blink 1.1s ease-in-out infinite",
        }}
      />
      <style>{`
        @keyframes vibeking-cursor-blink {
          0%, 100% { opacity: 0.15; }
          50%      { opacity: 0.9; }
        }
      `}</style>
    </>
  );
}

// Compact "warming model" pill — a spinning loader + label shown while a local
// model loads into memory after a switch / app open. Mirrors the compact
// recording row's glass + height so it sits naturally where the bar lives.
function WarmingPill({ label }: { label: string }) {
  const accent = ACCENTS.warming;
  return (
    <div className="vk-glass-bar flex items-center gap-2.5 h-11 px-3.5 rounded-full min-w-[220px] max-w-[92vw]">
      <div
        className="size-7 rounded-full grid place-items-center shrink-0 animate-vibeking-halo"
        style={
          {
            background: accent.bg,
            color: accent.fg,
            "--vk-halo": accent.bg,
          } as React.CSSProperties
        }
      >
        <Loader2 className="size-3.5 animate-spin" />
      </div>
      <span
        className="text-[12.5px] font-medium"
        style={{ color: "var(--vk-text-2)" }}
      >
        {label}
      </span>
    </div>
  );
}

function Card({
  phase,
  t,
  onDismiss,
}: {
  phase: Exclude<
    Phase,
    | { kind: "idle" }
    | { kind: "warming" }
    | { kind: "recording" }
    | { kind: "correction-prompt-ask" }
    | { kind: "correction-success-auto" }
  >;
  t: (k: string) => string;
  onDismiss?: () => void;
}) {
  const accent = ACCENTS[phase.kind];
  const isError = phase.kind === "error";

  const { Icon, label, raw, final } = ((): {
    Icon: typeof Mic;
    label: string;
    raw: string | null;
    final: string | null;
  } => {
    switch (phase.kind) {
      case "recognizing":
        return {
          Icon: Volume2,
          label: t("app.recognizing"),
          raw: phase.raw,
          final: null,
        };
      case "polishing":
        return {
          Icon: Sparkles,
          label: t("app.vibeking"),
          raw: phase.raw,
          final: null,
        };
      case "translating":
        return {
          Icon: Languages,
          label: t("app.translating"),
          raw: phase.raw,
          final: null,
        };
      case "done":
        return {
          Icon: CheckCircle2,
          label: phase.translated
            ? t("app.translating")
            : phase.polished
              ? t("app.vibeking")
              : t("app.recognizing"),
          raw: phase.raw,
          final: phase.final !== phase.raw ? phase.final : null,
        };
      case "error":
        return {
          Icon: AlertTriangle,
          label: t("chipbar.errorLabel"),
          raw: phase.message,
          final: null,
        };
    }
  })();

  const hasBody = Boolean(raw || final);
  // Active "working" phases get a breathing halo on the icon; the two LLM
  // rewrite phases additionally shimmer the in-flight text.
  const isWorking =
    phase.kind === "recognizing" ||
    phase.kind === "polishing" ||
    phase.kind === "translating";
  const isVibing =
    phase.kind === "polishing" || phase.kind === "translating";

  return (
    <div
      onClick={isError ? onDismiss : undefined}
      title={isError ? t("chipbar.errorDismiss") : undefined}
      className={`vk-glass-bar flex gap-3 px-4 py-3 w-[520px] max-w-[92vw]
        rounded-2xl
        ${hasBody ? "items-start" : "items-center"}
        ${isError ? "cursor-pointer hover:brightness-[0.98] transition" : ""}`}
    >
      <div
        className={`size-6 rounded-full grid place-items-center shrink-0 transition-colors duration-300 ${hasBody ? "mt-px" : ""} ${isWorking ? "animate-vibeking-halo" : ""}`}
        style={
          {
            background: accent.bg,
            color: accent.fg,
            "--vk-halo": accent.bg,
          } as React.CSSProperties
        }
      >
        <Icon className="size-3.5" />
      </div>
      <div className="min-w-0 flex-1">
        <div
          key={label}
          className="text-[12.5px] font-semibold leading-snug transition-colors duration-300 animate-vibeking-fade-up"
          style={{ color: accent.fg }}
        >
          {label}
        </div>

        {raw && final ? (
          // Done state with an actual LLM edit. Render as a word-level
          // diff so the user can SEE what the model changed — removed
          // tokens struck through, added tokens highlighted. Both
          // appear inline in a single paragraph instead of stacked
          // raw/final, which made minor edits (punctuation, casing)
          // easy to miss.
          <div className="mt-1.5 text-[13px] leading-snug font-medium text-[var(--vk-text)] animate-vibeking-fade-up">
            {diffWords(raw, final).map((op, i) => {
              if (op.kind === "equal") {
                return <span key={i}>{op.text}</span>;
              }
              if (op.kind === "removed") {
                return (
                  <span
                    key={i}
                    className="text-[var(--vk-text-9)] line-through decoration-[var(--vk-text-11)] decoration-[1px]"
                  >
                    {op.text}
                  </span>
                );
              }
              return (
                <span
                  key={i}
                  className="text-[var(--vk-accent-2)] font-semibold"
                >
                  {op.text}
                </span>
              );
            })}
          </div>
        ) : raw && isVibing ? (
          // The model is actively rewriting this text — sweep an Apple-
          // Intelligence-style shimmer across it so the user feels the work
          // happening on their words, not just a static "Vibing" label.
          <div className="mt-1.5 text-[12.5px] leading-snug vibeking-shimmer-text">
            {raw}
          </div>
        ) : raw ? (
          // No LLM edit (polish off, or model returned identical text) —
          // just render the raw transcript plainly.
          <div className="mt-1.5 text-[12.5px] leading-snug text-[var(--vk-text-3)]">
            {raw}
          </div>
        ) : null}
      </div>
      {isError ? (
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            onDismiss?.();
          }}
          title={t("chipbar.errorDismiss")}
          className="-mr-1 -mt-0.5 size-6 rounded-full grid place-items-center shrink-0 text-[var(--vk-text-8)] hover:text-[var(--vk-text-3)] hover:bg-[var(--vk-surface-3)] transition-colors"
        >
          <X className="size-3.5" />
        </button>
      ) : null}
    </div>
  );
}

// Pill base styling shared by the recording pill + correction prompt + success
// toast — same shape, shadow, and surface so they read as a family.
const CORRECTION_PILL_CLS = `
  vk-glass-bar
  flex items-center gap-3 px-4 py-2.5
  rounded-full
`;

function CorrectionPromptCard({
  phase,
  onKeep,
  onDiscard,
  onEditEnter,
}: {
  phase: Extract<Phase, { kind: "correction-prompt-ask" }>;
  onKeep: (from: string, to: string) => void;
  onDiscard: () => void;
  onEditEnter: () => void;
}) {
  const accent = ACCENTS["correction-prompt-ask"];
  const [editing, setEditing] = useState(false);
  const [draftFrom, setDraftFrom] = useState(phase.from);
  const [draftTo, setDraftTo] = useState(phase.to);

  // Reset draft when a new correction replaces this one mid-flight.
  useEffect(() => {
    setDraftFrom(phase.from);
    setDraftTo(phase.to);
    setEditing(false);
  }, [phase.from, phase.to]);

  if (editing) {
    const canSave =
      !!draftFrom.trim() &&
      !!draftTo.trim() &&
      draftFrom.trim() !== draftTo.trim();
    return (
      <div className={`${CORRECTION_PILL_CLS} min-w-[480px]`}>
        <PillIcon accent={accent} icon={<Sparkles className="size-3.5" />} />
        <input
          type="text"
          value={draftFrom}
          onChange={(e) => setDraftFrom(e.target.value)}
          className="min-w-0 flex-1 px-2.5 py-1 text-[12.5px] rounded-full border border-[var(--vk-shadow-08)] bg-[var(--vk-surface)] text-[var(--vk-text-2)] focus:outline-none focus:border-[var(--vk-accent-pop)]"
          autoFocus
        />
        <span
          className="shrink-0 text-[12.5px]"
          style={{ color: accent.muted }}
        >
          →
        </span>
        <input
          type="text"
          value={draftTo}
          onChange={(e) => setDraftTo(e.target.value)}
          className="min-w-0 flex-1 px-2.5 py-1 text-[12.5px] rounded-full border border-[var(--vk-shadow-08)] bg-[var(--vk-surface)] font-medium text-[var(--vk-text)] focus:outline-none focus:border-[var(--vk-accent-pop)]"
        />
        <PillBtn
          primary
          accent={accent}
          onClick={() => canSave && onKeep(draftFrom.trim(), draftTo.trim())}
          disabled={!canSave}
        >
          <Check className="size-3.5" />
        </PillBtn>
        <PillBtn accent={accent} onClick={onDiscard}>
          <X className="size-3.5" />
        </PillBtn>
      </div>
    );
  }

  return (
    <div className={`${CORRECTION_PILL_CLS} min-w-[340px]`}>
      <PillIcon accent={accent} icon={<Sparkles className="size-3.5" />} />
      <span className="text-[12.5px] leading-none">
        <span className="text-[var(--vk-text-9)] line-through decoration-[var(--vk-text-11)] decoration-[1px]">
          {phase.from}
        </span>
        <span className="mx-1.5" style={{ color: accent.muted }}>
          →
        </span>
        <span className="font-semibold text-[var(--vk-text)]">
          {phase.to}
        </span>
      </span>
      <span
        className="ml-auto text-[10.5px] tabular-nums font-medium"
        style={{ color: accent.muted }}
      >
        {phase.secondsLeft}s
      </span>
      <PillBtn primary accent={accent} onClick={() => onKeep(phase.from, phase.to)}>
        <Check className="size-3.5" />
      </PillBtn>
      <PillBtn
        accent={accent}
        onClick={() => {
          onEditEnter();
          setEditing(true);
        }}
      >
        <Pencil className="size-3.5" />
      </PillBtn>
      <PillBtn accent={accent} onClick={onDiscard}>
        <X className="size-3.5" />
      </PillBtn>
    </div>
  );
}

function CorrectionSuccessCard({
  corrections,
}: {
  corrections: { from: string; to: string }[];
}) {
  const { t } = useTranslation();
  const accent = ACCENTS["correction-success-auto"];
  if (corrections.length === 0) return null;

  // Single correction → keep the tight inline form. Multiple → show the
  // count plus the most recent two pairs (avoids unbounded pill growth
  // when many corrections fire within the 2s window).
  if (corrections.length === 1) {
    const { from, to } = corrections[0];
    return (
      <div className={`${CORRECTION_PILL_CLS} min-w-[280px]`}>
        <PillIcon accent={accent} icon={<CheckCircle2 className="size-3.5" />} />
        <span className="text-[12.5px] leading-none">
          <span className="text-[var(--vk-text-8)]">
            {t("chipbar.correctionLearnedSingle")}
          </span>{" "}
          <SwapInline from={from} to={to} mutedColor={accent.muted} />
        </span>
      </div>
    );
  }

  const visible = corrections.slice(-3);
  const hiddenCount = corrections.length - visible.length;
  return (
    <div className={`${CORRECTION_PILL_CLS} min-w-[320px] !rounded-2xl !py-2`}>
      <PillIcon accent={accent} icon={<CheckCircle2 className="size-3.5" />} />
      <div className="flex flex-col gap-0.5 text-[12.5px] leading-snug">
        <span className="font-semibold text-[var(--vk-text)]">
          {t("chipbar.correctionLearnedBatch", { count: corrections.length })}
          {hiddenCount > 0
            ? t("chipbar.correctionLearnedBatchSubtitle", {
                n: visible.length,
              })
            : ""}
        </span>
        {visible.map((c, i) => (
          <SwapInline
            key={`${c.from}->${c.to}-${i}`}
            from={c.from}
            to={c.to}
            mutedColor={accent.muted}
          />
        ))}
      </div>
    </div>
  );
}

function SwapInline({
  from,
  to,
  mutedColor,
}: {
  from: string;
  to: string;
  mutedColor: string;
}) {
  return (
    <span>
      <span className="text-[var(--vk-text-9)] line-through decoration-[var(--vk-text-11)] decoration-[1px]">
        {from}
      </span>
      <span className="mx-1.5" style={{ color: mutedColor }}>
        →
      </span>
      <span className="font-semibold text-[var(--vk-text)]">{to}</span>
    </span>
  );
}

function PillIcon({
  accent,
  icon,
}: {
  accent: Accent;
  icon: React.ReactNode;
}) {
  return (
    <div
      className="size-7 rounded-full grid place-items-center shrink-0"
      style={{ background: accent.bg, color: accent.fg }}
    >
      {icon}
    </div>
  );
}

function PillBtn({
  primary,
  accent,
  onClick,
  disabled,
  children,
}: {
  primary?: boolean;
  accent: Accent;
  onClick?: () => void;
  disabled?: boolean;
  children: React.ReactNode;
}) {
  const style = primary
    ? { background: accent.fg, color: "var(--vk-surface)" }
    : { background: "var(--vk-surface-3)", color: "var(--vk-text-4)" };
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className="size-7 rounded-full grid place-items-center shrink-0 hover:opacity-85 disabled:opacity-40 disabled:cursor-not-allowed transition-opacity"
      style={style}
    >
      {children}
    </button>
  );
}

function Equalizer({ accent }: { accent: string }) {
  const bars = 8;
  return (
    <div className="flex items-center gap-0.5 h-4">
      {Array.from({ length: bars }).map((_, i) => {
        // Center-weighted, slightly de-synced bars so the waveform breathes
        // organically instead of marching as one rigid traveling wave —
        // closer to how the macOS Voice Memos / Siri meters actually move.
        const fromCenter = Math.abs(i - (bars - 1) / 2);
        const minScale = 0.2 + fromCenter * 0.12; // edges sit lower
        const duration = 0.78 + (i % 3) * 0.14; // gentle phase drift
        return (
          <span
            key={i}
            className="w-[3px] rounded-full origin-center"
            style={
              {
                background: accent,
                height: "100%",
                "--eq-min": minScale.toFixed(2),
                animation: `vibeking-eq ${duration}s cubic-bezier(0.4, 0, 0.2, 1) ${i * 70}ms infinite`,
              } as React.CSSProperties
            }
          />
        );
      })}
      <style>{`
        @keyframes vibeking-eq {
          0%, 100% { transform: scaleY(var(--eq-min, 0.25)); }
          50%      { transform: scaleY(1); }
        }
      `}</style>
    </div>
  );
}

// Always-on idle bar (opt-in via Settings → "Always-on dictation bar"). A
// compact, calm resting state of the chip bar that sits near the bottom of the
// screen: brand · hotkey hint · clickable refinement-mode badge | open-app ·
// mic · close. Tuned to the same proportions as the recording pill so the two
// read as one family, just smaller and quieter when nothing is happening.
// Idle bar CONTENT only (the glass shell is provided by MorphShell; the mode
// picker is rendered by ChipBar as a sibling of the shell). Menu state is
// lifted to ChipBar so the popover can escape the shell's overflow.
function IdleBarContent({
  settings,
  activeMode,
  menuOpen,
  onModeClick,
  onRecord,
  onOpenApp,
  onClose,
}: {
  settings: Settings;
  activeMode: RefinementMode | null;
  menuOpen: boolean;
  onModeClick: () => void;
  onRecord: () => void;
  onOpenApp: () => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();

  // Known single keys keep their verbose label ("⌃ Right Control"); any custom
  // combo falls back to the compact glyph form ("⌃R + ⇧R + /") instead of the
  // raw "right-control+right-shift+slash", which used to overflow the bar.
  const hotkeyLabel =
    RECORD_HOTKEYS.find((h) => h.value === settings.recordHotkey)?.label ??
    formatHotkey(settings.recordHotkey);

  return (
    <div className="inline-flex items-center gap-[7px] h-11 pl-[11px] pr-[7px]">
        <BrandChip
          size="sm"
          className="!size-[18px] rounded-[5px] shadow-[0_1px_3px_var(--vk-shadow-08)]"
        />
        <span className="ml-0.5 text-[11px] font-medium text-[var(--vk-glass-ink-2)] whitespace-nowrap">
          {t("chipbar.idleHold")}
        </span>
        <Kbd
          title={hotkeyLabel}
          className="vk-glass-keycap h-[22px] min-w-0 max-w-[160px] shrink truncate whitespace-nowrap px-[7px] text-[11px] font-normal rounded-[7px]"
        >
          {hotkeyLabel}
        </Kbd>

        <button
          type="button"
          onClick={onModeClick}
          className={`ml-px inline-flex items-center gap-1 pl-2 pr-1.5 h-[22px] rounded-full text-[11px] font-medium transition hover:brightness-105 active:scale-[0.97] max-w-[128px] ${
            activeMode ? "vk-glass-control" : ""
          }`}
          style={{
            color: activeMode
              ? "var(--vk-glass-ink)"
              : "var(--vk-glass-ink-2)",
          }}
          title={t("chipbar.idleMode")}
        >
          {activeMode ? (
            <>
              <span aria-hidden>{activeMode.emoji}</span>
              <span className="truncate">{activeMode.name}</span>
            </>
          ) : (
            <span>{t("chipbar.refinementOff")}</span>
          )}
          <ChevronDown
            className={`size-3 opacity-50 shrink-0 transition-transform ${menuOpen ? "rotate-180" : ""}`}
          />
        </button>

        <div className="vk-glass-divider mx-[3px] h-5" />

        <IdleIconButton onClick={onOpenApp} title={t("chipbar.idleOpenApp")}>
          <Settings2 className="size-[15px]" />
        </IdleIconButton>
        <button
          type="button"
          onClick={onRecord}
          title={t("chipbar.idleStart")}
          className="vk-glass-mic size-8 rounded-full grid place-items-center shrink-0 text-white hover:brightness-110 active:scale-95 transition"
        >
          <Mic className="size-[15px]" />
        </button>
        <IdleIconButton onClick={onClose} title={t("chipbar.idleClose")}>
          <X className="size-[14px]" />
        </IdleIconButton>
    </div>
  );
}

function IdleIconButton({
  onClick,
  title,
  children,
}: {
  onClick: () => void;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      className="size-7 rounded-full grid place-items-center shrink-0 text-[var(--vk-glass-ink-2)] hover:text-[var(--vk-glass-ink)] hover:bg-[var(--vk-glass-chip)] transition-colors"
    >
      {children}
    </button>
  );
}

// Mode-picker surface (rows + JS hover). Rendered inside its OWN window
// (ModePickerWindow) so the chip bar never resizes for it — positioning is done
// natively (see windows::show_modepicker); this is just the menu surface.
function ModePicker({
  modes,
  activeId,
  offLabel,
  onPick,
  onClose,
}: {
  modes: RefinementMode[];
  activeId: string | null;
  offLabel: string;
  onPick: (id: string | null) => void;
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement | null>(null);
  // JS-driven hover. WKWebView (Tauri's macOS engine) stops flushing CSS
  // `:hover` repaints inside a transparent window after the first paint, so
  // the highlight would only work once. Tracking the hovered row in React
  // state forces a real DOM update on every move, which does flush.
  const [hovered, setHovered] = useState<string | null>(null);

  useEffect(() => {
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [onClose]);

  return (
    <div
      ref={ref}
      onMouseLeave={() => setHovered(null)}
      className="vk-menu-surface w-[210px] max-h-[340px] overflow-y-auto p-1 rounded-2xl animate-vibeking-chip-in"
    >
      {modes.map((m) => (
        <ModeRow
          key={m.id}
          active={m.id === activeId}
          hovered={hovered === m.id}
          onHover={() => setHovered(m.id)}
          onClick={() => onPick(m.id)}
        >
          <span aria-hidden>{m.emoji}</span>
          <span className="truncate">{m.name}</span>
        </ModeRow>
      ))}
      <div className="my-1 mx-2 h-px bg-[var(--vk-glass-divider)]" />
      <ModeRow
        active={activeId === null}
        hovered={hovered === "__off__"}
        onHover={() => setHovered("__off__")}
        onClick={() => onPick(null)}
      >
        <span className="text-[var(--vk-glass-ink-2)]">{offLabel}</span>
      </ModeRow>
    </div>
  );
}

// The mode picker's OWN window (label "modepicker", routed in App.tsx). It hosts
// just the picker surface, bottom-right-anchored so it sits flush above the chip
// bar (the window itself is positioned natively by windows::show_modepicker).
// Living in a dedicated window means opening it never resizes the chip window
// (no bar flicker) and its webview is never clipped (so hover keeps working).
export function ModePickerWindow() {
  const { t } = useTranslation();
  const [settings, updateSettings] = useSettings();
  const modes = settings.quickSwitchModeIds
    .map((id) => settings.refinementModes.find((m) => m.id === id))
    .filter((m): m is RefinementMode => !!m);
  const close = () => void invoke("hide_modepicker").catch(() => {});

  // Same transparent-root treatment as the chip bar — without it this window's
  // <html>/<body> keep their default opaque background (the white box).
  useEffect(() => {
    document.documentElement.classList.add("chipbar");
    return () => document.documentElement.classList.remove("chipbar");
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    // Empty area is click-through (pointer-events-none) so it never blocks apps
    // behind it — outside clicks blur the window, which auto-hides it (lib.rs).
    <div className="fixed inset-0 flex items-end justify-end p-1.5 pointer-events-none">
      <div className="pointer-events-auto">
        <ModePicker
          modes={modes}
          activeId={settings.activeRefinementModeId ?? null}
          offLabel={t("chipbar.refinementOff")}
          onPick={(id) => {
            void updateSettings({ activeRefinementModeId: id });
            close();
          }}
          onClose={close}
        />
      </div>
    </div>
  );
}

function ModeRow({
  active,
  hovered,
  onHover,
  onClick,
  children,
}: {
  active: boolean;
  hovered: boolean;
  onHover: () => void;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      onMouseEnter={onHover}
      onMouseMove={onHover}
      className="w-full flex items-center gap-2 px-2.5 h-8 rounded-lg text-[12.5px] text-left text-[var(--vk-glass-ink)]"
      style={{
        background: active || hovered ? "var(--vk-menu-hover)" : undefined,
      }}
    >
      {children}
      {active ? (
        <Check className="size-3.5 ml-auto shrink-0 text-[var(--vk-accent-2)]" />
      ) : null}
    </button>
  );
}
