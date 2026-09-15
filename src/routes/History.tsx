import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Trash2, Copy, Check, Clock } from "lucide-react";
import { clearAll, listEntries, type HistoryEntry } from "@/lib/history";
import { Select } from "@/components/ui/select";

function groupByDay(entries: HistoryEntry[]): Record<string, HistoryEntry[]> {
  return entries.reduce<Record<string, HistoryEntry[]>>((acc, e) => {
    const d = new Date(e.createdAt);
    const key = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
    (acc[key] ??= []).push(e);
    return acc;
  }, {});
}

function formatTime(ts: number): string {
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

function formatDuration(ms: number): string {
  return `${(ms / 1000).toFixed(1)}s`;
}

function formatDateLabel(key: string): string {
  const today = new Date();
  const [yyyy, mm, dd] = key.split("-").map((n) => parseInt(n, 10));
  const date = new Date(yyyy, mm - 1, dd);
  const diff = Math.floor(
    (new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime() -
      date.getTime()) /
      (1000 * 60 * 60 * 24),
  );
  if (diff === 0) return "今天";
  if (diff === 1) return "昨天";
  if (diff < 7) return `${diff} 天前`;
  return key;
}

export function History() {
  const { t } = useTranslation();
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [copiedId, setCopiedId] = useState<number | null>(null);

  async function refresh() {
    try {
      setEntries(await listEntries());
    } catch {
      setEntries([]);
    }
  }

  useEffect(() => {
    void refresh();
    const handler = () => void refresh();
    window.addEventListener("vibeking:history-updated", handler);
    return () => window.removeEventListener("vibeking:history-updated", handler);
  }, []);

  async function onClear() {
    try {
      await clearAll();
      await refresh();
    } catch {}
  }

  async function onCopy(entry: HistoryEntry) {
    await navigator.clipboard.writeText(entry.text);
    setCopiedId(entry.id);
    setTimeout(() => setCopiedId(null), 1400);
  }

  const grouped = groupByDay(entries);
  const days = Object.keys(grouped);

  return (
    <div className="px-10 py-10 max-w-[680px] mx-auto animate-vibeking-fade-up">
      <header className="flex items-end justify-between pb-8 border-b border-[var(--vk-border-2)]">
        <div>
          <h1 className="text-[24px] font-semibold tracking-tight text-[var(--vk-text)] leading-none">
            {t("history.title")}
          </h1>
          <p className="mt-2 text-[13px] text-[var(--vk-text-7)]">
            {entries.length === 0 ? "no transcripts yet" : `${entries.length} transcripts`}
          </p>
        </div>
        {entries.length > 0 ? (
          <button
            onClick={onClear}
            className="inline-flex items-center gap-1.5 h-7 px-2.5 rounded-md border border-[var(--vk-border-strong)] bg-[var(--vk-surface)] text-[12px] font-medium text-[var(--vk-text-6)] hover:bg-[var(--vk-canvas-3)] hover:border-[var(--vk-border-strong-2)] hover:text-[var(--vk-danger-6)] transition-colors"
          >
            <Trash2 className="size-3.5" />
            {t("history.clear")}
          </button>
        ) : null}
      </header>

      <div className="mt-6 flex items-center justify-between gap-3 px-4 py-3 rounded-lg bg-[var(--vk-surface-2)] border border-[var(--vk-border-2)]">
        <div className="flex items-center gap-2.5">
          <Clock className="size-4 text-[var(--vk-text-8)]" />
          <div>
            <div className="text-[12.5px] font-medium text-[var(--vk-text-2)]">
              {t("history.retention")}
            </div>
            <div className="text-[11px] text-[var(--vk-text-8)] mt-0.5">
              {t("history.retentionDescription")}
            </div>
          </div>
        </div>
        <Select
          value="forever"
          onChange={() => {}}
          options={[
            { value: "forever", label: t("history.forever") },
            { value: "30d", label: t("history.days30") },
            { value: "7d", label: t("history.days7") },
            { value: "off", label: t("history.off") },
          ]}
        />
      </div>

      {days.length === 0 ? (
        <div className="mt-12 rounded-xl border border-dashed border-[var(--vk-border-strong)] bg-[var(--vk-canvas-2)] py-16 text-center">
          <div className="mx-auto size-9 rounded-full bg-[var(--vk-accent-soft-bg-2)] grid place-items-center text-[var(--vk-accent-2)]">
            <Clock className="size-4" />
          </div>
          <div className="mt-3 text-[13px] text-[var(--vk-text-6)]">
            history will appear here
          </div>
          <div className="mt-1 text-[11.5px] text-[var(--vk-text-9)]">
            Hold the hotkey, speak, release
          </div>
        </div>
      ) : (
        <div className="mt-8 space-y-7">
          {days.map((day) => (
            <section key={day}>
              <div className="text-[10.5px] font-semibold uppercase tracking-[0.12em] text-[var(--vk-text-alt)] mb-2 px-1">
                {formatDateLabel(day)}
              </div>
              <div className="space-y-2">
                {grouped[day].map((e) => (
                  <article
                    key={e.id}
                    className="group flex items-start gap-3 px-4 py-3 rounded-lg bg-[var(--vk-surface)] border border-[var(--vk-border-2)] hover:border-[var(--vk-border-strong-3)] hover:shadow-[0_2px_8px_var(--vk-shadow-04)] transition-all"
                  >
                    <div className="w-12 shrink-0 tabular-nums text-[12px] text-[var(--vk-text-8)] pt-px">
                      {formatTime(e.createdAt)}
                    </div>
                    <div className="flex-1 min-w-0">
                      {e.text ? (
                        <p className="text-[13px] leading-relaxed text-[var(--vk-text-2)]">
                          {e.text}
                        </p>
                      ) : (
                        <p className="text-[12.5px] italic text-[var(--vk-text-9)]">
                          empty
                        </p>
                      )}
                      <div className="mt-1.5 flex items-center gap-2 text-[10.5px] text-[var(--vk-text-8)]">
                        <span className="tabular-nums">
                          {formatDuration(e.durationMs)}
                        </span>
                        <span className="size-0.5 rounded-full bg-[var(--vk-text-12)]" />
                        <span className="lowercase">{e.provider}</span>
                      </div>
                    </div>
                    <button
                      onClick={() => void onCopy(e)}
                      aria-label={t("history.copy")}
                      className="size-7 inline-flex items-center justify-center rounded-md text-[var(--vk-text-8)] opacity-0 group-hover:opacity-100 hover:bg-[var(--vk-surface-3)] hover:text-[var(--vk-text-2)] transition-all"
                    >
                      {copiedId === e.id ? (
                        <Check className="size-3.5 text-[var(--vk-success)]" />
                      ) : (
                        <Copy className="size-3.5" />
                      )}
                    </button>
                  </article>
                ))}
              </div>
            </section>
          ))}
        </div>
      )}
    </div>
  );
}
