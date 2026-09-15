import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ExternalLink, HelpCircle, X } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";

type Action = { label: string; pane: "accessibility" | "microphone" };

type Tip = {
  q: string;
  a: string;
  actions?: Action[];
};

// Tips re-resolve when the user switches language. These are plain-language
// answers, not terminal commands — a signed + notarized build no longer hits
// the Gatekeeper-quarantine or per-rebuild-permission issues that the old
// troubleshooting tips addressed.
function useTips(): Tip[] {
  const { t } = useTranslation();
  return [
    { q: t("help.tip1Q"), a: t("help.tip1A") },
    {
      q: t("help.tip2Q"),
      a: t("help.tip2A"),
      actions: [
        { label: t("help.openMic"), pane: "microphone" },
        { label: t("help.openAccessibility"), pane: "accessibility" },
      ],
    },
    { q: t("help.tip3Q"), a: t("help.tip3A") },
  ];
}

export function HelpButton({
  size = "sm",
  className = "",
}: {
  size?: "sm" | "md";
  className?: string;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const dim = size === "sm" ? "size-3.5" : "size-4";
  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        title={t("help.buttonTitle")}
        aria-label={t("help.buttonAria")}
        className={`inline-grid place-items-center size-5 rounded-full text-[var(--vk-text-8)] hover:bg-[var(--vk-border-2)] hover:text-[var(--vk-text-3)] transition-colors ${className}`}
      >
        <HelpCircle className={dim} />
      </button>
      {open && <HelpDialog onClose={() => setOpen(false)} />}
    </>
  );
}

function HelpDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const tips = useTips();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div
      className="fixed inset-0 z-[110] grid place-items-center bg-[var(--vk-chip-overlay-3)] backdrop-blur-sm animate-vibeking-fade-up"
      onClick={onClose}
    >
      <div
        className="w-[520px] max-w-[92vw] max-h-[80vh] overflow-y-auto rounded-2xl bg-[var(--vk-surface)] shadow-[0_24px_60px_var(--vk-chip-overlay-2),0_2px_8px_var(--vk-chip-overlay-1)] border border-[var(--vk-border-2)]"
        onClick={(e) => e.stopPropagation()}
      >
        <header className="flex items-center justify-between px-6 pt-5 pb-3 border-b border-[var(--vk-border-3)]">
          <div>
            <h2 className="text-[15px] font-semibold text-[var(--vk-text)] leading-tight">
              {t("help.title")}
            </h2>
            <p className="text-[12px] text-[var(--vk-text-7)] leading-tight mt-0.5">
              {t("help.subtitle")}
            </p>
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label={t("help.closeAria")}
            className="inline-grid place-items-center size-7 rounded-md text-[var(--vk-text-7)] hover:bg-[var(--vk-border-3)] hover:text-[var(--vk-text-2)] transition-colors"
          >
            <X className="size-4" />
          </button>
        </header>

        <div className="px-6 py-4 space-y-5">
          {tips.map((tip) => (
            <Section key={tip.q} tip={tip} />
          ))}
        </div>

        <footer className="px-6 pb-5 pt-2 text-[11px] leading-snug text-[var(--vk-text-8)]">
          {t("help.footer")}
        </footer>
      </div>
    </div>
  );
}

function Section({ tip }: { tip: Tip }) {
  return (
    <div>
      <h3 className="text-[13px] font-medium text-[var(--vk-text)] mb-1">
        {tip.q}
      </h3>
      <p className="text-[12.5px] leading-relaxed text-[var(--vk-text-5)]">
        {tip.a}
      </p>
      {tip.actions && (
        <div className="mt-2.5 flex flex-wrap gap-2">
          {tip.actions.map((action) => (
            <SettingsLink key={action.pane} action={action} />
          ))}
        </div>
      )}
    </div>
  );
}

function SettingsLink({ action }: { action: Action }) {
  return (
    <button
      type="button"
      onClick={() => {
        void invoke("open_settings_pane", { pane: action.pane }).catch(() => {
          // Non-fatal: the user can still open System Settings by hand.
        });
      }}
      className="inline-flex items-center gap-1.5 h-7 px-2.5 rounded-md border border-[var(--vk-border-strong)] bg-[var(--vk-surface)] text-[12px] font-medium text-[var(--vk-text-4)] hover:bg-[var(--vk-canvas-3)] hover:border-[var(--vk-border-strong-2)] transition-colors"
    >
      <ExternalLink className="size-3" />
      {action.label}
    </button>
  );
}
