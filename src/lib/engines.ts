// ---------------------------------------------------------------------------
// engines.ts — single source of truth for the on-device / cloud engine catalog
// plus the download/readiness hook. Shared by the Settings EnginePicker and the
// first-run OnboardingWizard so both describe the same engines identically.
//
// The "name" here is a calm, plain product name — no model IDs, no acceleration
// jargon. Sizes/copy come from i18n (home.enginePicker.*); this table only
// carries identity (provider value, engine id, on-device flag, gating).
// ---------------------------------------------------------------------------

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { Provider } from "@/lib/settings";
import { isTauri } from "@/lib/runtime";
import {
  downloadModel,
  deleteModel,
  getModelStatus,
  subscribeModelComplete,
  subscribeModelProgress,
  useIsPreparing,
  type Engine,
  type ModelProgress,
} from "@/lib/local-model";

export type Pill = { key: string; tone: "accent" | "neutral" | "amber" };

export type OnDeviceEngine = {
  provider: Provider;
  engine: Engine;
  /** Plain product name surfaced to the user. */
  name: string;
  /** Requires an Apple-silicon Mac to run. */
  requiresAppleSilicon: boolean;
  /** The single recommended default — pre-selected, accent-highlighted. */
  recommended?: boolean;
  pills: Pill[];
  /** i18n keys for the bullet copy. */
  bestForKey: string;
  sizeKey: string;
};

// Order is presentation order (recommended first). One — and only one — engine
// carries the accent "推荐" pill + `recommended`; the rest carry a neutral
// descriptive strength label so the grid reads as a recommendation, not four
// identical tiles.
export const ON_DEVICE: OnDeviceEngine[] = [
  {
    provider: "gemma",
    engine: "gemma",
    name: "Gemma",
    requiresAppleSilicon: true,
    recommended: true,
    pills: [{ key: "home.enginePicker.pillRecommended", tone: "accent" }],
    bestForKey: "home.enginePicker.gemmaBestFor",
    sizeKey: "home.enginePicker.gemmaSize",
  },
  {
    provider: "qwen3",
    engine: "qwen3",
    name: "Qwen3",
    requiresAppleSilicon: true,
    pills: [{ key: "home.enginePicker.pillCjkBest", tone: "neutral" }],
    bestForKey: "home.enginePicker.qwen3BestFor",
    sizeKey: "home.enginePicker.qwen3Size",
  },
  {
    provider: "fluid-audio",
    engine: "parakeet",
    name: "Parakeet",
    requiresAppleSilicon: true,
    pills: [{ key: "home.enginePicker.pillFastest", tone: "neutral" }],
    bestForKey: "home.enginePicker.parakeetBestFor",
    sizeKey: "home.enginePicker.parakeetSize",
  },
  {
    provider: "local",
    engine: "whisper",
    name: "Whisper",
    requiresAppleSilicon: false,
    pills: [{ key: "home.enginePicker.pillBroadest", tone: "neutral" }],
    bestForKey: "home.enginePicker.whisperBestFor",
    sizeKey: "home.enginePicker.whisperSize",
  },
];

export type CloudEngine = { provider: Provider; name: string };

export const CLOUD: CloudEngine[] = [
  { provider: "deepgram", name: "Deepgram" },
  { provider: "openai", name: "OpenAI" },
  { provider: "groq", name: "Groq" },
  { provider: "elevenlabs", name: "ElevenLabs" },
];

// ---------------------------------------------------------------------------
// useEngineStatus — unifies the four old *EngineRow effects into one hook.
//
// It replicates, for any engine, exactly what each row did:
//   - an initial getModelStatus({ engine }) to learn exists + preparing
//   - a model:progress subscription filtered to this engine (Whisper emits a
//     real %, the others little/none)
//   - a model:complete subscription that re-reads status and flips ready
//   - busy = useIsPreparing(engine) || backend `preparing` flag
//   - pct derived from progress when total is known, else null (indeterminate)
//   - error surfaced via the caller's humanError
//
// `inTauri === false` short-circuits all subscriptions: on-device engines
// can't download in the web preview, so the row shows a "Desktop app only"
// note instead.
// ---------------------------------------------------------------------------

export type EngineStatus = {
  /** null while the first status read is in flight. */
  ready: boolean | null;
  busy: boolean;
  /** 0–100 when a real percentage is known, else null (indeterminate). */
  pct: number | null;
  error: string | null;
  inTauri: boolean;
  download: () => Promise<void>;
  remove: () => Promise<void>;
};

function humanError(
  e: unknown,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  const msg = String(e);
  if (msg.includes("'invoke'")) return t("home.localWhisperInvokeError");
  if (msg.length > 80) return msg.slice(0, 80) + "…";
  return msg;
}

export function useEngineStatus(engine: Engine): EngineStatus {
  const { t } = useTranslation();
  const inTauri = isTauri();
  const [ready, setReady] = useState<boolean | null>(null);
  const [backendPreparing, setBackendPreparing] = useState(false);
  const [progress, setProgress] = useState<ModelProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const externallyPreparing = useIsPreparing(engine);
  const busy = externallyPreparing || backendPreparing;

  const pct =
    progress && progress.total
      ? Math.min(
          99,
          Math.max(0, Math.floor((progress.downloaded / progress.total) * 100)),
        )
      : null;

  useEffect(() => {
    if (!inTauri) return;
    let cancelled = false;
    let offProgress: (() => void) | undefined;
    let offComplete: (() => void) | undefined;

    const refresh = () =>
      getModelStatus({ engine })
        .then((s) => {
          if (cancelled) return;
          setReady(s.exists);
          setBackendPreparing(s.preparing);
        })
        .catch((e) => {
          if (!cancelled) setError(humanError(e, t));
        });

    void refresh();
    void subscribeModelProgress((p) => {
      if (p.engine !== engine) return;
      if (!cancelled) setProgress(p);
    }).then((o) => (offProgress = o));
    void subscribeModelComplete((p) => {
      if (p.engine !== engine) return;
      if (!cancelled) {
        setProgress(null);
        setBackendPreparing(false);
        void refresh();
      }
    }).then((o) => (offComplete = o));

    return () => {
      cancelled = true;
      offProgress?.();
      offComplete?.();
    };
  }, [engine, inTauri, t]);

  async function download() {
    setError(null);
    try {
      await downloadModel({ engine });
    } catch (e) {
      setError(humanError(e, t));
    }
  }

  async function remove() {
    setError(null);
    try {
      await deleteModel({ engine });
      setReady(false);
    } catch (e) {
      setError(humanError(e, t));
    }
  }

  return { ready, busy, pct, error, inTauri, download, remove };
}
