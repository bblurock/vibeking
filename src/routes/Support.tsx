import { useState } from "react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ExternalLink, ShieldCheck } from "lucide-react";

export function Support() {
  const { t } = useTranslation();
  const [error, setError] = useState(false);
  const openIssues = async () => {
    setError(false);
    try {
      await openUrl("https://github.com/bblurock/vibeking/issues");
    } catch {
      setError(true);
    }
  };
  return (
    <div className="px-10 py-10 max-w-[680px] mx-auto animate-vibeking-fade-up">
      <header className="pb-7 border-b border-[var(--vk-border-2)]">
        <h1 className="text-[24px] font-semibold tracking-tight text-[var(--vk-text)]">{t("feedback.title")}</h1>
        <p className="mt-2 text-[13px] text-[var(--vk-text-7)] leading-relaxed">{t("feedback.subtitle")}</p>
      </header>
      <div className="mt-7 flex items-start gap-2.5 rounded-lg bg-[var(--vk-canvas-2)] px-3.5 py-3">
        <ShieldCheck className="size-4 mt-px shrink-0 text-[var(--vk-text-8)]" />
        <p className="text-[12px] leading-relaxed text-[var(--vk-text-7)]">{t("feedback.privacy")}</p>
      </div>
      <button type="button" onClick={() => void openIssues()} className="mt-5 inline-flex items-center gap-2 rounded-lg bg-[var(--vk-accent-4)] text-white h-9 px-4 text-[13px] font-medium hover:bg-[var(--vk-accent-5)]">
        {t("feedback.send")}<ExternalLink className="size-4" />
      </button>
      {error && <p role="alert" className="mt-3 text-[13px] text-[var(--vk-danger)]">{t("feedback.error")} https://github.com/bblurock/vibeking/issues</p>}
    </div>
  );
}
