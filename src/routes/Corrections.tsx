import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ArrowRight,
  Check,
  Download,
  Pencil,
  Plus,
  Search,
  Sparkles,
  Trash2,
  X,
} from "lucide-react";
import { useSettings } from "@/lib/use-settings";
import type { CorrectionEntry } from "@/lib/settings";
import i18n from "@/lib/i18n";

const FILTER_VISIBLE_THRESHOLD = 8;

type TFn = (key: string, options?: Record<string, unknown>) => string;

function formatLearnedAt(ms: number, t: TFn): string {
  const now = Date.now();
  const diff = Math.max(0, now - ms);
  const sec = Math.floor(diff / 1000);
  if (sec < 60) return t("common.timeJustNow");
  const min = Math.floor(sec / 60);
  if (min < 60) return t("common.timeMinsAgo", { n: min });
  const hr = Math.floor(min / 60);
  if (hr < 24) return t("common.timeHoursAgo", { n: hr });
  const day = Math.floor(hr / 24);
  if (day === 1) return t("common.timeYesterday");
  if (day < 7) return t("common.timeDaysAgo", { n: day });
  const d = new Date(ms);
  const sameYear = d.getFullYear() === new Date().getFullYear();
  const opts: Intl.DateTimeFormatOptions = sameYear
    ? { month: "short", day: "numeric" }
    : { year: "numeric", month: "short", day: "numeric" };
  return d.toLocaleDateString(i18n.language, opts);
}

export function Corrections() {
  const { t } = useTranslation();
  const [settings, update, ready] = useSettings();
  const [filter, setFilter] = useState("");

  if (!ready) {
    return <div className="px-10 py-10 max-w-[680px] mx-auto" />;
  }

  const dict = settings.correctionDictionary;
  const filterTrim = filter.trim().toLowerCase();
  const filtered = useMemo(() => {
    if (!filterTrim) return dict.map((entry, idx) => ({ entry, idx }));
    return dict
      .map((entry, idx) => ({ entry, idx }))
      .filter(({ entry }) =>
        entry.from.toLowerCase().includes(filterTrim) ||
        entry.to.toLowerCase().includes(filterTrim),
      );
  }, [dict, filterTrim]);

  function removeAt(idx: number) {
    void update({
      correctionDictionary: dict.filter((_, i) => i !== idx),
    });
  }

  function updateAt(idx: number, from: string, to: string) {
    void update({
      correctionDictionary: dict.map((entry, i) =>
        i === idx ? { ...entry, from, to } : entry,
      ),
    });
  }

  function addManual(from: string, to: string) {
    const f = from.trim();
    const t = to.trim();
    if (!f || !t || f === t) return;
    // Replace any existing entry with the same `from`, otherwise prepend.
    const existing = dict.findIndex((e) => e.from === f);
    const newEntry: CorrectionEntry = { from: f, to: t, learnedAtMs: Date.now() };
    const next =
      existing >= 0
        ? dict.map((e, i) => (i === existing ? newEntry : e))
        : [newEntry, ...dict];
    void update({ correctionDictionary: next });
  }

  function wipeAll() {
    if (dict.length === 0) return;
    const ok = window.confirm(
      t("corrections.confirmWipe", { count: dict.length }),
    );
    if (!ok) return;
    void update({ correctionDictionary: [] });
  }

  function exportJson() {
    const blob = new Blob([JSON.stringify(dict, null, 2)], {
      type: "application/json",
    });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "vibeking-corrections.json";
    a.click();
    URL.revokeObjectURL(url);
  }

  const showFilter = dict.length >= FILTER_VISIBLE_THRESHOLD;

  return (
    <div className="px-10 py-10 max-w-[680px] mx-auto animate-vibeking-fade-up">
      <header className="flex items-end justify-between pb-7 border-b border-[var(--vk-border-2)]">
        <div>
          <h1 className="text-[24px] font-semibold tracking-tight text-[var(--vk-text)] leading-none">
            {t("corrections.title")}
          </h1>
          <p className="mt-2 text-[13px] text-[var(--vk-text-7)] max-w-[60ch] leading-relaxed">
            {t("corrections.description")}
          </p>
        </div>
        <div className="flex items-center gap-1.5">
          <IconButton
            label={t("common.exportJson")}
            disabled={dict.length === 0}
            onClick={exportJson}
          >
            <Download className="size-3.5" />
          </IconButton>
          <IconButton
            label={t("common.wipeAll")}
            destructive
            disabled={dict.length === 0}
            onClick={wipeAll}
          >
            <Trash2 className="size-3.5" />
          </IconButton>
        </div>
      </header>

      <AddManualRow onAdd={addManual} />

      <div className="mt-8">
        <div className="text-[10.5px] font-semibold uppercase tracking-[0.12em] text-[var(--vk-text-alt)] mb-3 px-1 flex items-center justify-between gap-3">
          <span className="inline-flex items-center gap-2">
            <span>{t("corrections.listLabel")}</span>
            <span className="text-[var(--vk-text-9)] tabular-nums normal-case tracking-normal text-[11px]">
              {filterTrim && filtered.length !== dict.length
                ? t("corrections.countOf", {
                    visible: filtered.length,
                    total: dict.length,
                  })
                : dict.length}
            </span>
          </span>
          {showFilter ? (
            <FilterInput value={filter} onChange={setFilter} />
          ) : null}
        </div>

        {dict.length === 0 ? (
          <EmptyState />
        ) : filtered.length === 0 ? (
          <div className="rounded-xl border border-dashed border-[var(--vk-border-strong)] bg-[var(--vk-canvas-2)] px-6 py-8 text-center text-[12.5px] text-[var(--vk-text-7)]">
            {t("corrections.emptyFilter", { filter })}
          </div>
        ) : (
          <div className="space-y-1">
            {filtered.map(({ entry, idx }) => (
              <CorrectionRow
                key={`${entry.learnedAtMs}-${idx}-${entry.from}`}
                entry={entry}
                onRemove={() => removeAt(idx)}
                onUpdate={(from, to) => updateAt(idx, from, to)}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

function IconButton({
  children,
  label,
  onClick,
  disabled,
  destructive,
}: {
  children: React.ReactNode;
  label: string;
  onClick?: () => void;
  disabled?: boolean;
  destructive?: boolean;
}) {
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      aria-label={label}
      title={label}
      className={`size-8 inline-flex items-center justify-center rounded-lg border border-[var(--vk-border)] bg-[var(--vk-surface)] text-[var(--vk-text-7)] disabled:opacity-35 disabled:cursor-not-allowed transition-all ${
        destructive
          ? "hover:border-[var(--vk-danger-border-2)] hover:text-[var(--vk-danger-6)] hover:bg-[var(--vk-danger-soft-bg-4)]"
          : "hover:border-[var(--vk-border-strong-2)] hover:text-[var(--vk-text-2)] hover:bg-[var(--vk-canvas-3)]"
      }`}
    >
      {children}
    </button>
  );
}

function FilterInput({
  value,
  onChange,
}: {
  value: string;
  onChange: (v: string) => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-1.5 h-7 px-2 rounded-md border border-[var(--vk-border)] bg-[var(--vk-surface)] text-[12px] focus-within:border-[var(--vk-accent-4)] focus-within:ring-2 focus-within:ring-[var(--vk-chip-glass-shadow-4)] transition-all normal-case tracking-normal">
      <Search className="size-3 text-[var(--vk-text-9)] shrink-0" />
      <input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={t("common.filterPlaceholder")}
        spellCheck={false}
        className="w-32 bg-transparent text-[var(--vk-text-2)] placeholder:text-[var(--vk-text-9)] focus:outline-none"
      />
      {value ? (
        <button
          onClick={() => onChange("")}
          aria-label={t("common.clearFilter")}
          className="text-[var(--vk-text-9)] hover:text-[var(--vk-text-2)]"
        >
          <X className="size-3" />
        </button>
      ) : null}
    </div>
  );
}

function AddManualRow({
  onAdd,
}: {
  onAdd: (from: string, to: string) => void;
}) {
  const { t } = useTranslation();
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const canAdd = from.trim() && to.trim() && from.trim() !== to.trim();

  function commit() {
    if (!canAdd) return;
    onAdd(from, to);
    setFrom("");
    setTo("");
  }

  return (
    <div className="mt-6 grid grid-cols-[1fr_auto_1fr_auto] items-center gap-2 px-1.5 py-1.5 rounded-lg border border-[var(--vk-border)] bg-[var(--vk-surface)] shadow-[0_1px_2px_var(--vk-shadow-03)] focus-within:border-[var(--vk-accent-4)] focus-within:ring-2 focus-within:ring-[var(--vk-chip-glass-shadow-4)] transition-all">
      <input
        value={from}
        onChange={(e) => setFrom(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && commit()}
        placeholder={t("corrections.addFromPlaceholder")}
        spellCheck={false}
        className="font-mono bg-transparent text-[12.5px] text-[var(--vk-text-2)] placeholder:text-[var(--vk-text-soft-2)] placeholder:font-sans px-2.5 h-7 focus:outline-none"
      />
      <ArrowRight className="size-3.5 text-[var(--vk-text-10)] shrink-0" />
      <input
        value={to}
        onChange={(e) => setTo(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && commit()}
        placeholder={t("corrections.addToPlaceholder")}
        spellCheck={false}
        className="font-mono bg-transparent text-[12.5px] font-medium text-[var(--vk-text)] placeholder:text-[var(--vk-text-soft-2)] placeholder:font-sans placeholder:font-normal px-2.5 h-7 focus:outline-none"
      />
      <button
        onClick={commit}
        disabled={!canAdd}
        className="inline-flex items-center gap-1 h-7 px-2.5 rounded-md bg-[var(--vk-accent-4)] text-white text-[12px] font-medium hover:bg-[var(--vk-accent-5)] disabled:opacity-40 disabled:cursor-not-allowed transition-colors shadow-[0_1px_2px_var(--vk-chip-glass-shadow-2),inset_0_1px_0_var(--vk-chip-highlight)]"
      >
        <Plus className="size-3.5" />
        {t("corrections.add")}
      </button>
    </div>
  );
}

function EmptyState() {
  const { t } = useTranslation();
  return (
    <div className="rounded-xl border border-dashed border-[var(--vk-border-strong)] bg-[var(--vk-canvas-2)] px-6 py-10 text-center">
      <div className="mx-auto size-9 rounded-full bg-[var(--vk-accent-soft-bg-2)] grid place-items-center text-[var(--vk-accent-2)]">
        <Sparkles className="size-4" />
      </div>
      <div className="mt-3 text-[13px] font-medium text-[var(--vk-text-4)]">
        {t("corrections.emptyTitle")}
      </div>
      <p className="mt-2 text-[12px] leading-relaxed text-[var(--vk-text-8)] max-w-[44ch] mx-auto">
        {t("corrections.emptyBody")}
      </p>
      <p className="mt-2 text-[11px] text-[var(--vk-text-9)]">
        {t("corrections.emptyHint")}
      </p>
    </div>
  );
}

function CorrectionRow({
  entry,
  onRemove,
  onUpdate,
}: {
  entry: CorrectionEntry;
  onRemove: () => void;
  onUpdate: (from: string, to: string) => void;
}) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [draftFrom, setDraftFrom] = useState(entry.from);
  const [draftTo, setDraftTo] = useState(entry.to);
  const fromInputRef = useRef<HTMLInputElement>(null);

  // Reset the draft when the underlying entry changes from outside (cross-
  // webview broadcast etc), but only when we're not actively editing.
  useEffect(() => {
    if (!editing) {
      setDraftFrom(entry.from);
      setDraftTo(entry.to);
    }
  }, [entry.from, entry.to, editing]);

  useEffect(() => {
    if (editing) {
      fromInputRef.current?.focus();
      fromInputRef.current?.select();
    }
  }, [editing]);

  function enterEdit() {
    setDraftFrom(entry.from);
    setDraftTo(entry.to);
    setEditing(true);
  }

  function cancelEdit() {
    setDraftFrom(entry.from);
    setDraftTo(entry.to);
    setEditing(false);
  }

  function commit() {
    const from = draftFrom.trim();
    const to = draftTo.trim();
    if (!from || !to || from === to) return;
    if (from === entry.from && to === entry.to) {
      setEditing(false);
      return;
    }
    onUpdate(from, to);
    setEditing(false);
  }

  const canSave =
    !!draftFrom.trim() &&
    !!draftTo.trim() &&
    draftFrom.trim() !== draftTo.trim();

  // Same grid in both states so the row doesn't reflow on enter/exit edit.
  const grid =
    "grid grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto_auto] items-center gap-2.5 px-3 h-11 rounded-lg transition-colors";

  if (editing) {
    return (
      <article
        className={`${grid} bg-[var(--vk-surface)] border border-[var(--vk-accent-8)] ring-2 ring-[var(--vk-chip-glass-shadow-4)]`}
      >
        <input
          ref={fromInputRef}
          type="text"
          value={draftFrom}
          onChange={(e) => setDraftFrom(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit();
            if (e.key === "Escape") cancelEdit();
          }}
          className="min-w-0 font-mono text-[12.5px] px-2 h-7 rounded-md border border-[var(--vk-border-strong)] bg-[var(--vk-canvas-2)] text-[var(--vk-text-2)] focus:outline-none focus:border-[var(--vk-accent-2)] focus:bg-[var(--vk-surface)]"
        />
        <ArrowRight className="size-3.5 text-[var(--vk-text-10)] shrink-0" />
        <input
          type="text"
          value={draftTo}
          onChange={(e) => setDraftTo(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit();
            if (e.key === "Escape") cancelEdit();
          }}
          className="min-w-0 font-mono text-[12.5px] font-medium px-2 h-7 rounded-md border border-[var(--vk-border-strong)] bg-[var(--vk-canvas-2)] text-[var(--vk-text)] focus:outline-none focus:border-[var(--vk-accent-2)] focus:bg-[var(--vk-surface)]"
        />
        <RowAction
          label={t("common.save")}
          variant="primary"
          onClick={commit}
          disabled={!canSave}
        >
          <Check className="size-3.5" />
        </RowAction>
        <RowAction label={t("common.cancel")} onClick={cancelEdit}>
          <X className="size-3.5" />
        </RowAction>
      </article>
    );
  }

  return (
    <article
      className={`group ${grid} bg-[var(--vk-surface)] border border-transparent hover:border-[var(--vk-border)] hover:bg-[var(--vk-canvas)]`}
    >
      <span
        className="font-mono text-[12.5px] text-[var(--vk-text-alt-2)] truncate"
        title={entry.from}
      >
        {entry.from}
      </span>
      <ArrowRight className="size-3.5 text-[var(--vk-text-10)] shrink-0" />
      <span
        className="font-mono text-[12.5px] font-semibold text-[var(--vk-text)] truncate"
        title={entry.to}
      >
        {entry.to}
      </span>
      <div className="flex items-center gap-2 pl-1">
        <span className="text-[10.5px] tabular-nums text-[var(--vk-text-9)] tracking-tight">
          {formatLearnedAt(entry.learnedAtMs, t)}
        </span>
        <div className="flex items-center gap-0.5 opacity-50 group-hover:opacity-100 transition-opacity">
          <RowAction
            label={t("corrections.actionEdit", {
              from: entry.from,
              to: entry.to,
            })}
            onClick={enterEdit}
          >
            <Pencil className="size-3.5" />
          </RowAction>
          <RowAction
            label={t("corrections.actionRemove", {
              from: entry.from,
              to: entry.to,
            })}
            destructive
            onClick={onRemove}
          >
            <X className="size-3.5" />
          </RowAction>
        </div>
      </div>
    </article>
  );
}

function RowAction({
  children,
  label,
  onClick,
  disabled,
  variant,
  destructive,
}: {
  children: React.ReactNode;
  label: string;
  onClick?: () => void;
  disabled?: boolean;
  variant?: "primary";
  destructive?: boolean;
}) {
  const styles =
    variant === "primary"
      ? "bg-[var(--vk-accent-4)] text-white hover:bg-[var(--vk-accent-5)] disabled:opacity-40 shadow-[0_1px_2px_var(--vk-chip-glass-shadow-2),inset_0_1px_0_var(--vk-chip-highlight)]"
      : destructive
        ? "text-[var(--vk-text-8)] hover:bg-[var(--vk-danger-soft-bg-2)] hover:text-[var(--vk-danger-6)]"
        : "text-[var(--vk-text-8)] hover:bg-[var(--vk-accent-soft-bg-5)] hover:text-[var(--vk-text-2)]";
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      aria-label={label}
      title={label}
      className={`size-7 inline-flex items-center justify-center rounded-md transition-colors disabled:cursor-not-allowed ${styles}`}
    >
      {children}
    </button>
  );
}
