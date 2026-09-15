import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import { isTauri } from "@/lib/runtime";
import {
  getModelStatus,
  subscribeModelComplete,
  subscribeModelProgress,
  useIsPreparing,
  type ModelProgress,
} from "@/lib/local-model";
import { useSettings } from "@/lib/use-settings";

// Persistent, non-dismissible "preparing local engine" strip.
// Mounted alongside ErrorBanner so it lives outside the route tree and
// survives navigation. Auto-dismisses on `model:complete` for parakeet.
//
// Two prep signals are honored:
//  - `useIsPreparing("parakeet")` — in-memory tracker that flips the
//    moment downloadModel() runs in this frontend session. Reacts
//    instantly without a Tauri roundtrip.
//  - `status.preparing` — Rust-side PREPARING flag exposed via
//    getModelStatus. Survives Cmd+R / page reload because Rust state
//    isn't reset; the in-memory tracker is.
//
// Either one being true keeps the banner up; both being false hides it.
// fluidaudio-rs emits a single (0, None) progress tick and then
// completes, so spinner is indeterminate — no percentage bar.
export function ParakeetPrepBanner() {
  const { t } = useTranslation();
  const [settings] = useSettings();
  const externallyPreparing = useIsPreparing("parakeet");
  const [backendPreparing, setBackendPreparing] = useState(false);
  const [progress, setProgress] = useState<ModelProgress | null>(null);

  useEffect(() => {
    if (!isTauri()) return;
    if (settings.provider !== "fluid-audio") {
      setBackendPreparing(false);
      setProgress(null);
      return;
    }
    let cancelled = false;
    let offComplete: (() => void) | undefined;
    let offProgress: (() => void) | undefined;
    const refresh = () =>
      getModelStatus({ engine: "parakeet" })
        .then((s) => {
          if (!cancelled) setBackendPreparing(s.preparing);
        })
        .catch(() => {
          /* leave state as-is on transient errors */
        });
    void refresh();
    void subscribeModelProgress((p) => {
      if (p.engine !== "parakeet") return;
      if (!cancelled) setProgress(p);
    }).then((off) => {
      if (cancelled) off();
      else offProgress = off;
    });
    void subscribeModelComplete((c) => {
      if (c.engine !== "parakeet") return;
      if (!cancelled) {
        setBackendPreparing(false);
        setProgress(null);
      }
    }).then((off) => {
      if (cancelled) off();
      else offComplete = off;
    });
    return () => {
      cancelled = true;
      offProgress?.();
      offComplete?.();
    };
  }, [settings.provider]);

  const visible =
    settings.provider === "fluid-audio" &&
    (externallyPreparing || backendPreparing);

  const pct =
    progress && progress.total
      ? Math.min(99, Math.max(0, Math.floor((progress.downloaded / progress.total) * 100)))
      : null;

  if (!visible) return null;

  return (
    <div
      role="status"
      aria-live="polite"
      className="fixed top-3 left-1/2 -translate-x-1/2 z-50 pointer-events-none animate-vibeking-fade-up"
    >
      <div className="pointer-events-auto inline-flex items-center gap-2 rounded-full border border-[var(--vk-border-2)] bg-[var(--vk-surface)] px-3 py-1.5 shadow-[0_8px_24px_var(--vk-shadow-10),0_1px_2px_var(--vk-shadow-04)]">
        <Loader2 className="size-3.5 animate-spin text-[var(--vk-accent-4)]" />
        <span className="text-[12px] font-medium text-[var(--vk-text-2)]">
          {t("onboarding.preparingBanner")}
        </span>
        {pct !== null && (
          <span className="text-[12px] font-medium text-[var(--vk-text-7)] tabular-nums">
            {pct}%
          </span>
        )}
      </div>
    </div>
  );
}
