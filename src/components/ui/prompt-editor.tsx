import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { RotateCcw } from "lucide-react";
import { cn } from "@/lib/utils";

type Props = {
  label: string;
  hint?: string;
  value: string;
  defaultValue: string;
  onChange: (next: string) => void;
  rows?: number;
  placeholderTokens?: string[];
};

export function PromptEditor({
  label,
  hint,
  value,
  defaultValue,
  onChange,
  rows = 4,
  placeholderTokens,
}: Props) {
  const { t } = useTranslation();
  // Show the default whenever the saved value is empty so users see a
  // starting point rather than a blank box.
  const effective = value && value.length > 0 ? value : defaultValue;

  const taRef = useRef<HTMLTextAreaElement>(null);
  const [draft, setDraft] = useState(effective);
  const [dirty, setDirty] = useState(false);
  const [focused, setFocused] = useState(false);

  useEffect(() => {
    if (!dirty) setDraft(effective);
  }, [effective, dirty]);

  function commit() {
    setFocused(false);
    if (draft === defaultValue) {
      // Treat "matches default" as "use default" — keep persisted value empty.
      if (value !== "") onChange("");
    } else if (draft !== value) {
      onChange(draft);
    }
    setDirty(false);
  }

  function reset() {
    setDraft(defaultValue);
    onChange("");
    setDirty(false);
  }

  const isModified = draft !== defaultValue;

  return (
    <div className="px-4 py-3.5 border-t border-[var(--vk-border-3)] first:border-t-0">
      <div className="flex items-center justify-between mb-1.5">
        <div className="min-w-0">
          <div className="text-[13.5px] font-medium text-[var(--vk-text-2)]">
            {label}
          </div>
          {hint ? (
            <div className="mt-0.5 text-[11.5px] text-[var(--vk-text-8)]">
              {hint}
            </div>
          ) : null}
        </div>
        {isModified ? (
          <button
            onClick={reset}
            className="inline-flex items-center gap-1 h-6 px-2 rounded text-[11px] font-medium text-[var(--vk-text-8)] hover:text-[var(--vk-text-2)] hover:bg-[var(--vk-surface-3)] transition-colors"
            aria-label={t("home.polishPromptReset")}
          >
            <RotateCcw className="size-3" />
            {t("home.polishPromptReset")}
          </button>
        ) : null}
      </div>
      <textarea
        ref={taRef}
        value={draft}
        onChange={(e) => {
          setDraft(e.target.value);
          setDirty(true);
        }}
        onFocus={() => setFocused(true)}
        onBlur={commit}
        rows={rows}
        spellCheck={false}
        className={cn(
          "w-full rounded-md border bg-[var(--vk-surface)] px-3 py-2",
          "text-[12.5px] leading-relaxed text-[var(--vk-text)]",
          "font-mono",
          "resize-none outline-none",
          "transition-[border-color,box-shadow] duration-150",
          focused
            ? "border-[var(--vk-accent-4)]"
            : isModified
              ? "border-[var(--vk-text-13)]"
              : "border-[var(--vk-border)]",
          !focused && "hover:border-[var(--vk-text-soft)]",
        )}
        style={
          focused
            ? { boxShadow: "0 0 0 3px var(--vk-chip-glass-shadow-3)" }
            : undefined
        }
      />
      {placeholderTokens && placeholderTokens.length > 0 ? (
        <div className="mt-1.5 flex flex-wrap gap-1 text-[10.5px] text-[var(--vk-text-8)]">
          <span>Tokens:</span>
          {placeholderTokens.map((tok) => (
            <code
              key={tok}
              className="font-mono px-1 py-0.5 rounded bg-[var(--vk-surface-3)] border border-[var(--vk-border-2)] text-[var(--vk-text-4)]"
            >
              {tok}
            </code>
          ))}
        </div>
      ) : null}
    </div>
  );
}
