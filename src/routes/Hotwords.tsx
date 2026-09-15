import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  BookText,
  Check,
  Download,
  Pencil,
  Plus,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { useSettings } from "@/lib/use-settings";

const FILTER_VISIBLE_THRESHOLD = 8;
const BATCH_DELIM = /[,\n\t;]+/;

function parseBatch(raw: string): string[] {
  return raw
    .split(BATCH_DELIM)
    .map((w) => w.trim())
    .filter((w) => w.length > 0);
}

export function Hotwords() {
  const { t } = useTranslation();
  const [settings, update] = useSettings();
  const [draft, setDraft] = useState("");
  const [filter, setFilter] = useState("");

  const hotwords = settings.hotwords;
  const filterTrim = filter.trim().toLowerCase();

  const visible = useMemo(() => {
    if (!filterTrim) return hotwords;
    return hotwords.filter((w) => w.toLowerCase().includes(filterTrim));
  }, [hotwords, filterTrim]);

  // The add input accepts a single word OR a delimited batch
  // ("a, b, c" or pasted newline-separated). Show a preview when the draft
  // would resolve to >1 word.
  const draftBatch = parseBatch(draft);
  const isBatch = draftBatch.length > 1;
  const dedupedBatch = draftBatch.filter((w) => !hotwords.includes(w));
  const draftCanCommit = isBatch
    ? dedupedBatch.length > 0
    : draft.trim().length > 0 && !hotwords.includes(draft.trim());

  function commit() {
    if (!draftCanCommit) return;
    if (isBatch) {
      void update({ hotwords: [...dedupedBatch, ...hotwords] });
    } else {
      void update({ hotwords: [draft.trim(), ...hotwords] });
    }
    setDraft("");
  }

  function remove(w: string) {
    void update({ hotwords: hotwords.filter((x) => x !== w) });
  }

  function updateAt(oldWord: string, newWord: string) {
    const v = newWord.trim();
    if (!v) return;
    const exists = hotwords.some((w) => w !== oldWord && w === v);
    if (exists) {
      // Renaming to an existing entry → just remove the old one.
      remove(oldWord);
      return;
    }
    void update({
      hotwords: hotwords.map((w) => (w === oldWord ? v : w)),
    });
  }

  function wipeAll() {
    if (hotwords.length === 0) return;
    const ok = window.confirm(
      t("hotwords.confirmWipe", { count: hotwords.length }),
    );
    if (!ok) return;
    void update({ hotwords: [] });
  }

  function exportJson() {
    const blob = new Blob([JSON.stringify(hotwords, null, 2)], {
      type: "application/json",
    });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = "vibeking-hotwords.json";
    a.click();
    URL.revokeObjectURL(url);
  }

  const showFilter = hotwords.length >= FILTER_VISIBLE_THRESHOLD;

  return (
    <div className="px-10 py-10 max-w-[680px] mx-auto animate-vibeking-fade-up">
      <header className="flex items-end justify-between pb-7 border-b border-[var(--vk-border-2)]">
        <div>
          <h1 className="text-[24px] font-semibold tracking-tight text-[var(--vk-text)] leading-none">
            {t("hotwords.title")}
          </h1>
          <p className="mt-2 text-[13px] text-[var(--vk-text-7)] max-w-[60ch] leading-relaxed">
            {t("hotwords.description")}
          </p>
        </div>
        <div className="flex items-center gap-1.5">
          <IconButton
            label={t("common.exportJson")}
            disabled={hotwords.length === 0}
            onClick={exportJson}
          >
            <Download className="size-3.5" />
          </IconButton>
          <IconButton
            label={t("common.wipeAll")}
            destructive
            disabled={hotwords.length === 0}
            onClick={wipeAll}
          >
            <Trash2 className="size-3.5" />
          </IconButton>
        </div>
      </header>

      <div className="mt-6">
        <div className="flex items-stretch gap-2 rounded-lg border border-[var(--vk-border)] bg-[var(--vk-surface)] pl-3 pr-1.5 py-1.5 shadow-[0_1px_2px_var(--vk-shadow-03)] focus-within:border-[var(--vk-accent-4)] focus-within:ring-2 focus-within:ring-[var(--vk-chip-glass-shadow-4)] transition-all">
          <input
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                commit();
              }
            }}
            placeholder={t("hotwords.addPlaceholder")}
            spellCheck={false}
            className="flex-1 bg-transparent text-[13.5px] text-[var(--vk-text-2)] placeholder:text-[var(--vk-text-9)] focus:outline-none"
          />
          <button
            onClick={commit}
            disabled={!draftCanCommit}
            className="inline-flex items-center gap-1 rounded-md bg-[var(--vk-accent-4)] text-white px-3 h-7 self-center text-[12px] font-medium hover:bg-[var(--vk-accent-5)] disabled:opacity-40 disabled:cursor-not-allowed transition-colors shadow-[0_1px_2px_var(--vk-chip-glass-shadow-2),inset_0_1px_0_var(--vk-chip-highlight)]"
          >
            <Plus className="size-3.5" />
            {isBatch
              ? t("hotwords.addN", { n: dedupedBatch.length })
              : t("hotwords.add")}
          </button>
        </div>
        {isBatch ? (
          <div className="mt-2 px-1 text-[11.5px] text-[var(--vk-text-8)] leading-relaxed">
            <span className="font-medium text-[var(--vk-text-5)]">
              {t("hotwords.batchNewCount", { n: dedupedBatch.length })}
            </span>{" "}
            {dedupedBatch.length !== draftBatch.length
              ? t("hotwords.batchAlreadyPresent", {
                  n: draftBatch.length - dedupedBatch.length,
                })
              : null}
            {dedupedBatch.length > 0 ? (
              <>
                {" — "}
                <span className="font-mono text-[var(--vk-text-alt-2)]">
                  {dedupedBatch.slice(0, 6).join(", ")}
                  {dedupedBatch.length > 6
                    ? t("hotwords.batchMore", { n: dedupedBatch.length - 6 })
                    : ""}
                </span>
              </>
            ) : null}
          </div>
        ) : null}
      </div>

      <div className="mt-8">
        <div className="text-[10.5px] font-semibold uppercase tracking-[0.12em] text-[var(--vk-text-alt)] mb-3 px-1 flex items-center justify-between gap-3">
          <span className="inline-flex items-center gap-2">
            <span>{t("hotwords.listLabel")}</span>
            <span className="text-[var(--vk-text-9)] tabular-nums normal-case tracking-normal text-[11px]">
              {filterTrim && visible.length !== hotwords.length
                ? t("hotwords.countOf", {
                    visible: visible.length,
                    total: hotwords.length,
                  })
                : hotwords.length}
            </span>
          </span>
          {showFilter ? (
            <FilterInput value={filter} onChange={setFilter} />
          ) : null}
        </div>

        {hotwords.length === 0 ? (
          <EmptyState />
        ) : visible.length === 0 ? (
          <div className="rounded-xl border border-dashed border-[var(--vk-border-strong)] bg-[var(--vk-canvas-2)] px-6 py-8 text-center text-[12.5px] text-[var(--vk-text-7)]">
            {t("hotwords.emptyFilter", { filter })}
          </div>
        ) : (
          <div className="flex flex-wrap gap-1.5">
            {visible.map((w) => (
              <HotwordChip
                key={w}
                word={w}
                onRemove={() => remove(w)}
                onRename={(next) => updateAt(w, next)}
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

function EmptyState() {
  const { t } = useTranslation();
  return (
    <div className="rounded-xl border border-dashed border-[var(--vk-border-strong)] bg-[var(--vk-canvas-2)] px-6 py-10 text-center">
      <div className="mx-auto size-9 rounded-full bg-[var(--vk-accent-soft-bg-2)] grid place-items-center text-[var(--vk-accent-2)]">
        <BookText className="size-4" />
      </div>
      <div className="mt-3 text-[13px] font-medium text-[var(--vk-text-4)]">
        {t("hotwords.emptyTitle")}
      </div>
      <p className="mt-2 text-[12px] leading-relaxed text-[var(--vk-text-8)] max-w-[44ch] mx-auto">
        {t("hotwords.emptyBody")}
      </p>
      <p className="mt-2 text-[11px] text-[var(--vk-text-9)]">
        {t("hotwords.emptyHint")}
      </p>
    </div>
  );
}

function HotwordChip({
  word,
  onRemove,
  onRename,
}: {
  word: string;
  onRemove: () => void;
  onRename: (next: string) => void;
}) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(word);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!editing) setDraft(word);
  }, [word, editing]);

  useEffect(() => {
    if (editing) {
      inputRef.current?.focus();
      inputRef.current?.select();
    }
  }, [editing]);

  function commit() {
    const v = draft.trim();
    if (!v) {
      cancelEdit();
      return;
    }
    if (v !== word) onRename(v);
    setEditing(false);
  }

  function cancelEdit() {
    setDraft(word);
    setEditing(false);
  }

  if (editing) {
    return (
      <div className="inline-flex items-center gap-1 h-8 pl-2 pr-1 rounded-full border border-[var(--vk-accent-8)] bg-[var(--vk-surface)] ring-2 ring-[var(--vk-chip-glass-shadow-4)] shadow-[0_1px_2px_var(--vk-shadow-03)]">
        <input
          ref={inputRef}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit();
            if (e.key === "Escape") cancelEdit();
          }}
          onBlur={commit}
          spellCheck={false}
          className="bg-transparent text-[12.5px] font-medium text-[var(--vk-text-2)] focus:outline-none"
          style={{ width: `${Math.max(6, draft.length + 1)}ch` }}
        />
        <button
          onClick={commit}
          aria-label={t("common.save")}
          className="size-5 inline-flex items-center justify-center rounded-full text-white bg-[var(--vk-accent-4)] hover:bg-[var(--vk-accent-5)] transition-colors"
        >
          <Check className="size-3" />
        </button>
      </div>
    );
  }

  return (
    <div className="group inline-flex items-center h-8 pl-3 pr-1 rounded-full border border-[var(--vk-border)] bg-[var(--vk-surface)] text-[12.5px] font-medium text-[var(--vk-text-2)] hover:border-[var(--vk-border-strong-2)] hover:bg-[var(--vk-canvas-3)] transition-colors shadow-[0_1px_2px_var(--vk-shadow-03)]">
      <button
        type="button"
        onClick={() => setEditing(true)}
        className="pr-1.5 cursor-text focus:outline-none"
        title={t("hotwords.actionEdit", { word })}
      >
        {word}
      </button>
      <button
        type="button"
        onClick={() => setEditing(true)}
        aria-label={t("hotwords.actionEdit", { word })}
        className="size-5 inline-flex items-center justify-center rounded-full text-[var(--vk-text-9)] opacity-0 group-hover:opacity-100 hover:text-[var(--vk-accent-6)] hover:bg-[var(--vk-accent-soft-bg-2)] transition-all"
      >
        <Pencil className="size-3" />
      </button>
      <button
        onClick={onRemove}
        aria-label={t("hotwords.actionRemove", { word })}
        className="size-5 inline-flex items-center justify-center rounded-full text-[var(--vk-text-9)] hover:text-[var(--vk-danger-6)] hover:bg-[var(--vk-danger-soft-bg)] transition-colors"
      >
        <X className="size-3" />
      </button>
    </div>
  );
}
