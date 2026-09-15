import { ChevronDown } from "lucide-react";
import { cn } from "@/lib/utils";

type Option = { value: string; label: string; disabled?: boolean };

type Props = {
  value: string;
  onChange: (value: string) => void;
  options: Option[];
  disabled?: boolean;
  className?: string;
  size?: "sm" | "md";
};

export function Select({
  value,
  onChange,
  options,
  disabled,
  className,
  size = "sm",
}: Props) {
  const sizeStyles =
    size === "md" ? "h-9 text-[13px] pl-3 pr-8" : "h-7 text-[12px] pl-2.5 pr-7";

  return (
    <div className={cn("relative inline-flex", className)}>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        disabled={disabled}
        className={cn(
          "appearance-none cursor-pointer",
          "rounded-md border border-[var(--vk-border-strong)] bg-[var(--vk-surface)]",
          "text-[var(--vk-text-2)] font-medium",
          "hover:bg-[var(--vk-canvas-3)] transition-colors",
          "focus:outline-none focus:ring-2 focus:ring-[--color-ring] focus:ring-offset-1",
          "disabled:opacity-50 disabled:cursor-not-allowed",
          sizeStyles,
        )}
      >
        {options.map((o) => (
          <option key={o.value} value={o.value} disabled={o.disabled}>
            {o.label}
          </option>
        ))}
      </select>
      <ChevronDown
        className="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2 size-3.5 text-[var(--vk-text-8)]"
        aria-hidden="true"
      />
    </div>
  );
}
