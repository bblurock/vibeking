import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";

/// Editable model picker: a free-text input (so a not-yet-pulled or remote
/// model id always works) paired with a dropdown of models discovered from the
/// provider's `GET /v1/models` endpoint. Fetching/caching lives in the parent;
/// this component only renders the input, the discovered-model menu, a refresh
/// affordance, and the loading/empty/error status line.
///
/// The dropdown is portal-mounted with fixed positioning anchored to the input
/// rect — settings rows live inside cards with `overflow-hidden`, so an
/// `absolute`-positioned panel would get clipped (same reasoning as Combobox).
export function ModelCombobox({
  value,
  onChange,
  models,
  loading,
  error,
  onRefresh,
  placeholder,
}: {
  value: string;
  onChange: (v: string) => void;
  models: string[];
  loading: boolean;
  error: string | null;
  onRefresh: () => void;
  placeholder?: string;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const inputWrapRef = useRef<HTMLDivElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const [rect, setRect] = useState<{
    left: number;
    top: number;
    width: number;
  } | null>(null);

  const reposition = useCallback(() => {
    const el = inputWrapRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setRect({ left: r.left, top: r.bottom + 4, width: r.width });
  }, []);

  useLayoutEffect(() => {
    if (open) reposition();
  }, [open, reposition]);

  useEffect(() => {
    if (!open) return;
    const handler = () => reposition();
    window.addEventListener("scroll", handler, true);
    window.addEventListener("resize", handler);
    return () => {
      window.removeEventListener("scroll", handler, true);
      window.removeEventListener("resize", handler);
    };
  }, [open, reposition]);

  // Close on outside click / Esc.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const target = e.target as Node;
      if (
        inputWrapRef.current?.contains(target) ||
        panelRef.current?.contains(target)
      ) {
        return;
      }
      setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const status = loading
    ? t("home.modelComboLoading")
    : error
      ? t("home.modelComboUnreachable")
      : models.length > 0
        ? t("home.modelComboCount", { count: models.length })
        : t("home.modelComboNone");

  const panel =
    open && rect && models.length > 0
      ? createPortal(
          <div
            ref={panelRef}
            className={cn(
              "fixed z-[1000] max-h-[280px] overflow-y-auto py-1",
              "rounded-lg border border-[var(--vk-border-strong)] bg-[var(--vk-surface)]",
              "shadow-[0_12px_36px_var(--vk-shadow-12),0_2px_6px_var(--vk-shadow-05)]",
            )}
            style={{ left: rect.left, top: rect.top, minWidth: rect.width }}
          >
            {models.map((m) => (
              <button
                type="button"
                key={m}
                onMouseDown={(e) => {
                  // mousedown (not click) so the input doesn't blur before commit.
                  e.preventDefault();
                  onChange(m);
                  setOpen(false);
                }}
                className={cn(
                  "flex w-full items-center gap-1.5 px-2.5 py-1.5 text-left text-[12px] font-mono",
                  m === value
                    ? "bg-[var(--vk-canvas-3)] text-[var(--vk-text)]"
                    : "text-[var(--vk-text-2)] hover:bg-[var(--vk-canvas-3)]",
                )}
              >
                <Check
                  className={cn(
                    "size-3 shrink-0",
                    m === value ? "opacity-100" : "opacity-0",
                  )}
                  aria-hidden="true"
                />
                <span className="truncate">{m}</span>
              </button>
            ))}
          </div>,
          document.body,
        )
      : null;

  return (
    <div className="inline-flex flex-col items-end gap-1">
      <div className="inline-flex items-center gap-1">
        <div ref={inputWrapRef} className="relative inline-flex">
          <input
            type="text"
            value={value}
            onChange={(e) => onChange(e.target.value)}
            placeholder={placeholder}
            spellCheck={false}
            autoComplete="off"
            className={cn(
              "h-7 w-[200px] rounded-md border border-[var(--vk-border-strong)] bg-[var(--vk-surface)]",
              "pl-2.5 pr-7 text-[12px] text-[var(--vk-text-2)] font-mono",
              "placeholder:text-[var(--vk-text-10)]",
              "hover:border-[var(--vk-border-strong-2)] transition-colors",
              "focus:outline-none focus:ring-2 focus:ring-[--color-ring] focus:ring-offset-1 focus:border-transparent",
            )}
          />
          <button
            type="button"
            aria-label={t("home.modelComboToggle")}
            onClick={() => setOpen((o) => !o)}
            className="absolute right-0 top-0 grid h-7 w-7 place-items-center text-[var(--vk-text-8)] hover:text-[var(--vk-text-2)]"
          >
            <ChevronDown className="size-3.5" aria-hidden="true" />
          </button>
        </div>
        <button
          type="button"
          aria-label={t("home.modelComboRefresh")}
          title={t("home.modelComboRefresh")}
          onClick={onRefresh}
          className="grid h-7 w-7 place-items-center rounded-md border border-[var(--vk-border-strong)] bg-[var(--vk-surface)] text-[var(--vk-text-8)] hover:bg-[var(--vk-canvas-3)] hover:text-[var(--vk-text-2)] transition-colors"
        >
          <RefreshCw
            className={cn("size-3.5", loading && "animate-spin")}
            aria-hidden="true"
          />
        </button>
      </div>

      <span
        className={cn(
          "text-[10px] leading-none",
          error ? "text-[var(--vk-warning,#c08a2a)]" : "text-[var(--vk-text-10)]",
        )}
      >
        {status}
      </span>

      {panel}
    </div>
  );
}
