import type { LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";

export type TabItem = {
  id: string;
  label: string;
  icon?: LucideIcon;
};

/**
 * Left-aligned underline tabs. No enclosing box — the row sits flush with the
 * page title and cards, and the active tab is marked by a quiet accent
 * underline (the icon also picks up the accent). Controlled: the parent owns
 * `value` and updates it in `onChange`.
 */
export function Tabs({
  items,
  value,
  onChange,
  className,
  "aria-label": ariaLabel,
}: {
  items: TabItem[];
  value: string;
  onChange: (id: string) => void;
  className?: string;
  "aria-label"?: string;
}) {
  return (
    <div
      role="tablist"
      aria-label={ariaLabel}
      aria-orientation="horizontal"
      className={cn("flex items-center gap-6", className)}
    >
      {items.map((it) => {
        const active = it.id === value;
        const Icon = it.icon;
        return (
          <button
            key={it.id}
            type="button"
            role="tab"
            aria-selected={active}
            onClick={() => onChange(it.id)}
            className={cn(
              "group relative inline-flex items-center gap-1.5 pb-2 text-[13px] font-medium transition-colors",
              active
                ? "text-[var(--vk-text)]"
                : "text-[var(--vk-text-7)] hover:text-[var(--vk-text-2)]",
            )}
          >
            {Icon && (
              <Icon
                className={cn(
                  "size-3.5 transition-colors",
                  active
                    ? "text-[var(--vk-accent)]"
                    : "text-[var(--vk-text-8)] group-hover:text-[var(--vk-text-5)]",
                )}
              />
            )}
            {it.label}
            {/* Active underline — crossfades between tabs (opacity only, so it
                respects reduced motion). */}
            <span
              aria-hidden
              className={cn(
                "absolute inset-x-0 bottom-0 h-0.5 rounded-full bg-[var(--vk-accent)] transition-opacity duration-200",
                active ? "opacity-100" : "opacity-0",
              )}
            />
          </button>
        );
      })}
    </div>
  );
}
