import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  CheckCircle2,
  AlertCircle,
  ExternalLink,
  RefreshCw,
} from "lucide-react";
import { isTauri } from "@/lib/runtime";
import {
  checkPermissions,
  openSettingsPane,
  subscribeEventTapFailed,
  type PermissionsStatus,
  type SettingsPane,
} from "@/lib/permissions";
import { BrandChip } from "@/components/BrandMark";
import { HelpButton } from "@/components/HelpDialog";
import { useOnboardingCompleted } from "@/lib/onboarding";
import { OnboardingWizard } from "@/components/onboarding/OnboardingWizard";

type Row = {
  pane: SettingsPane;
  title: string;
  why: string;
  granted: boolean;
};

export function PermissionsGate({ children }: { children: React.ReactNode }) {
  const { t } = useTranslation();
  const [status, setStatus] = useState<PermissionsStatus | null>(null);
  const [dismissedThisSession, setDismissedThisSession] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [lastCheckedAt, setLastCheckedAt] = useState<Date | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [onboardingDone, completeOnboarding] = useOnboardingCompleted();

  const refresh = async () => {
    if (!isTauri()) return;
    setRefreshing(true);
    setError(null);
    const startedAt = Date.now();
    try {
      const next = await checkPermissions();
      console.log("[vibeking] check_permissions ->", next);
      setStatus(next);
      setLastCheckedAt(new Date());
    } catch (e) {
      console.error("[vibeking] check_permissions failed:", e);
      setError(String(e));
    } finally {
      // Ensure the spinner is visible for at least 400ms so the click registers
      // visually even when the underlying check finishes in microseconds.
      const elapsed = Date.now() - startedAt;
      if (elapsed < 400) {
        await new Promise((r) => setTimeout(r, 400 - elapsed));
      }
      setRefreshing(false);
    }
  };

  useEffect(() => {
    if (!isTauri()) return;
    void refresh();
    // Re-check whenever the user returns to the app — they likely just toggled
    // a permission in System Settings.
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    // If the event tap fails after startup, surface immediately.
    let unlisten: (() => void) | undefined;
    void subscribeEventTapFailed(() => {
      setDismissedThisSession(false);
      void refresh();
    }).then((off) => (unlisten = off));
    return () => {
      window.removeEventListener("focus", onFocus);
      unlisten?.();
    };
  }, []);

  // Order: [onboarding pending?] -> [permissions pending?] -> [app].
  // Onboarding is purely a frontend localStorage gate, so it runs in
  // both browser dev (vite at :1420) and Tauri. Permissions are
  // Tauri-only — checked further down once onboarding is settled.
  if (!onboardingDone) {
    return (
      <>
        {children}
        <OnboardingWizard onComplete={completeOnboarding} />
      </>
    );
  }

  if (!isTauri() || status === null) {
    return <>{children}</>;
  }

  const allGranted =
    status.accessibility && status.input_monitoring && status.microphone;
  if (allGranted || dismissedThisSession) {
    return <>{children}</>;
  }

  const rows: Row[] = [
    {
      pane: "input-monitoring",
      title: t("permissions.inputMonitoringTitle"),
      why: t("permissions.inputMonitoringWhy"),
      granted: status.input_monitoring,
    },
    {
      pane: "accessibility",
      title: t("permissions.accessibilityTitle"),
      why: t("permissions.accessibilityWhy"),
      granted: status.accessibility,
    },
    {
      pane: "microphone",
      title: t("permissions.microphoneTitle"),
      why: t("permissions.microphoneWhy"),
      granted: status.microphone,
    },
  ];

  return (
    <>
      {children}
      <div className="fixed inset-0 z-[100] grid place-items-center bg-[var(--vk-chip-overlay-3)] backdrop-blur-sm animate-vibeking-fade-up">
        <div className="w-[480px] max-w-[92vw] rounded-2xl bg-[var(--vk-surface)] shadow-[0_24px_60px_var(--vk-chip-overlay-2),0_2px_8px_var(--vk-chip-overlay-1)] border border-[var(--vk-border-2)] overflow-hidden">
          <div className="px-6 pt-6 pb-4 border-b border-[var(--vk-border-3)]">
            <div className="flex items-center gap-3 mb-3">
              <BrandChip size="md" />
              <div className="flex-1">
                <div className="flex items-center justify-between gap-2">
                  <h2 className="text-[16px] font-semibold text-[var(--vk-text)] leading-tight">
                    {t("permissions.title")}
                  </h2>
                  <HelpButton />
                </div>
                <p className="text-[12px] text-[var(--vk-text-7)] leading-tight mt-0.5">
                  {t("permissions.subtitle")}
                </p>
              </div>
            </div>
          </div>

          <div className="px-6 py-4 space-y-3">
            {rows.map((row) => (
              <PermissionRow
                key={row.pane}
                row={row}
                onOpen={() => void openSettingsPane(row.pane)}
              />
            ))}
          </div>

          {error && (
            <div className="mx-6 mb-3 rounded-md border border-[var(--vk-danger-border-4)] bg-[var(--vk-danger-soft-bg-3)] px-3 py-2 text-[11.5px] text-[var(--vk-danger-5)]">
              {t("permissions.checkFailed", { error })}
            </div>
          )}

          <div className="px-6 pb-5 pt-2 flex items-center justify-between gap-2">
            <div className="flex flex-col gap-0.5">
              <button
                type="button"
                onClick={() => setDismissedThisSession(true)}
                className="text-left text-[12px] text-[var(--vk-text-8)] hover:text-[var(--vk-text-4)] transition-colors"
              >
                {t("permissions.skipForNow")}
              </button>
              {lastCheckedAt && (
                <span className="text-[10.5px] text-[var(--vk-text-9)] tabular-nums">
                  {t("permissions.lastChecked", {
                    time: lastCheckedAt.toLocaleTimeString(),
                  })}
                </span>
              )}
            </div>
            <button
              type="button"
              onClick={refresh}
              disabled={refreshing}
              className="inline-flex items-center gap-1.5 h-8 px-3 rounded-md bg-[var(--vk-accent-4)] text-[12.5px] font-medium text-white hover:bg-[var(--vk-accent-5)] disabled:opacity-60 transition-colors shadow-[0_1px_2px_var(--vk-chip-glass-shadow-2),inset_0_1px_0_var(--vk-chip-highlight)]"
            >
              <RefreshCw
                className={`size-3.5 ${refreshing ? "animate-spin" : ""}`}
              />
              {t("permissions.recheck")}
            </button>
          </div>

          <p className="px-6 pb-5 pt-1 text-[11px] leading-snug text-[var(--vk-text-8)]">
            {t("permissions.restartNote")}
          </p>
        </div>
      </div>
    </>
  );
}


function PermissionRow({
  row,
  onOpen,
}: {
  row: Row;
  onOpen: () => void;
}) {
  const { t } = useTranslation();
  const granted = row.granted;
  return (
    <div className="flex items-start gap-3 p-3 rounded-lg border border-[var(--vk-border-2)] bg-[var(--vk-canvas)]">
      <div className="mt-0.5 size-5 grid place-items-center">
        {granted ? (
          <CheckCircle2 className="size-5 text-[var(--vk-success)]" />
        ) : (
          <AlertCircle className="size-5 text-[var(--vk-warning)]" />
        )}
      </div>
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <h3 className="text-[13px] font-medium text-[var(--vk-text)] leading-none">
            {row.title}
          </h3>
          {granted ? (
            <span className="text-[10.5px] font-semibold uppercase tracking-wider text-[var(--vk-success)]">
              {t("permissions.statusGranted")}
            </span>
          ) : (
            <span className="text-[10.5px] font-semibold uppercase tracking-wider text-[var(--vk-warning)]">
              {t("permissions.statusRequired")}
            </span>
          )}
        </div>
        <p className="mt-1 text-[12px] leading-snug text-[var(--vk-text-6)]">
          {row.why}
        </p>
        {!granted && (
          <button
            type="button"
            onClick={onOpen}
            className="mt-2 inline-flex items-center gap-1 h-6 px-2 rounded text-[11.5px] font-medium text-[var(--vk-accent-7)] hover:bg-[var(--vk-accent-soft-bg)] transition-colors"
          >
            <ExternalLink className="size-3" />
            {t("permissions.openSettings")}
          </button>
        )}
      </div>
    </div>
  );
}
