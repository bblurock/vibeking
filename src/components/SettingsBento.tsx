import { useId } from "react";
import { cn } from "@/lib/utils";

/**
 * SettingsBento — one concern per card. The toggle (or mode selector)
 * lives in the header so the user instantly sees the switch AND what it
 * controls. When `disabled` is true, the body grays to 50% opacity,
 * pointer events are blocked, and assistive tech gets aria-disabled.
 *
 * Layout invariants used everywhere in Home.tsx:
 *   - rounded-xl, white surface, hairline border, subtle 1px shadow
 *   - header is a flex row: { eyebrow + title + description } | { control }
 *   - body is rendered inside `<div data-bento-body>` so the disabled
 *     treatment is a single CSS toggle, never re-implemented at call sites
 *
 * Header eyebrow text matches the existing all-caps spec
 * (10.5px / 0.12em tracking / 0.55 lightness). Use sparingly —
 * the bento is the unit of orientation now, not the eyebrow.
 */
export function SettingsBento({
  eyebrow,
  title,
  description,
  control,
  controlLabel,
  disabled = false,
  disabledNote,
  footer,
  children,
  className,
}: {
  eyebrow?: string;
  title: string;
  description?: string;
  /**
   * Right-hand control rendered in the bento header. Typically a `<Switch>`
   * for boolean toggles or a `<Select>` for multi-mode (Off/Ask/Auto, etc).
   * Pass `undefined` for always-on bentos (Recording, STT).
   */
  control?: React.ReactNode;
  /** A11y label for the header control. Used as `aria-labelledby` target. */
  controlLabel?: string;
  /** When true, body grays out and stops receiving pointer events. */
  disabled?: boolean;
  /**
   * Optional short text shown next to the title when disabled — e.g. "(off)".
   * Tells the user the body is intentionally inert, not broken.
   */
  disabledNote?: string;
  /** Optional footer slot rendered below the body, outside the disabled region. */
  footer?: React.ReactNode;
  children: React.ReactNode;
  className?: string;
}) {
  const titleId = useId();

  return (
    <section
      className={cn(
        "mt-4 first:mt-0",
        "rounded-xl bg-[var(--vk-surface)] overflow-hidden",
        "border border-[var(--vk-border)]",
        "shadow-[0_1px_2px_var(--vk-shadow-03),0_1px_1px_var(--vk-shadow-04)]",
        // When disabled, the surface itself dims very slightly so the card
        // reads as "asleep" alongside the body's heavier dim.
        disabled && "bg-[var(--vk-canvas)]",
        className,
      )}
      aria-labelledby={titleId}
    >
      <header
        className={cn(
          "flex items-start justify-between gap-4 px-4 py-3.5",
          "border-b border-[var(--vk-border-3)]",
        )}
      >
        <div className="min-w-0 flex-1">
          {eyebrow ? (
            <div className="text-[10px] font-semibold uppercase tracking-[0.12em] text-[var(--vk-text-eyebrow)] mb-1">
              {eyebrow}
            </div>
          ) : null}
          <div className="flex items-baseline gap-2 flex-wrap">
            <h2
              id={titleId}
              className={cn(
                "text-[14px] font-semibold leading-snug",
                disabled
                  ? "text-[var(--vk-text-6)]"
                  : "text-[var(--vk-text)]",
              )}
            >
              {title}
            </h2>
            {disabled && disabledNote ? (
              <span className="text-[11px] font-medium text-[var(--vk-text-8)] tabular-nums">
                {disabledNote}
              </span>
            ) : null}
          </div>
          {description ? (
            <p
              className={cn(
                "mt-1 text-[12px] leading-relaxed max-w-[60ch]",
                disabled
                  ? "text-[var(--vk-text-9)]"
                  : "text-[var(--vk-text-7)]",
              )}
            >
              {description}
            </p>
          ) : null}
        </div>
        {control ? (
          <div
            className="flex items-center gap-2 shrink-0 pt-0.5"
            aria-label={controlLabel}
          >
            {control}
          </div>
        ) : null}
      </header>

      <div
        data-bento-body
        // 150ms ease-out matches the rest of the app's micro-transitions.
        // Pointer-events:none keeps inputs focusable via tab only if the
        // surrounding inert attribute below is absent; we use `inert` so
        // screen readers and keyboard nav skip the disabled body entirely.
        inert={disabled ? true : undefined}
        aria-hidden={disabled || undefined}
        className={cn(
          "transition-opacity duration-150 ease-out",
          disabled && "opacity-50 pointer-events-none select-none",
        )}
      >
        {children}
      </div>

      {footer ? (
        <div className="border-t border-[var(--vk-border-3)]">{footer}</div>
      ) : null}
    </section>
  );
}

/**
 * BentoRow — same visual rhythm as the previous SettingsRow, but lives
 * INSIDE a SettingsBento body (no card wrapper, no top border on first
 * row since the body sits flush under the bento header divider).
 */
export function BentoRow({
  label,
  hint,
  children,
  className,
}: {
  label: string;
  hint?: string;
  children?: React.ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex items-center justify-between gap-4 px-4 py-3.5",
        "border-t border-[var(--vk-border-3)] first:border-t-0",
        className,
      )}
    >
      <div className="min-w-0 flex-1">
        <div className="text-[13px] font-medium text-[var(--vk-text-2)] leading-snug">
          {label}
        </div>
        {hint ? (
          <div className="mt-0.5 text-[11.5px] text-[var(--vk-text-8)] leading-snug">
            {hint}
          </div>
        ) : null}
      </div>
      <div className="flex items-center gap-2 shrink-0">{children}</div>
    </div>
  );
}

/**
 * BentoBlock — a free-form full-width region inside the bento body, used
 * for the prompt editors that don't fit the label-on-left / control-on-right
 * grid. Maintains the same horizontal padding and divider behavior.
 */
export function BentoBlock({
  children,
  className,
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "px-4 py-3.5 border-t border-[var(--vk-border-3)] first:border-t-0",
        className,
      )}
    >
      {children}
    </div>
  );
}
