import { cn } from "@/lib/utils";

type Props = {
  checked: boolean;
  onChange?: (next: boolean) => void;
  disabled?: boolean;
  className?: string;
  "aria-label"?: string;
};

export function Switch({
  checked,
  onChange,
  disabled,
  className,
  "aria-label": ariaLabel,
}: Props) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      disabled={disabled}
      onClick={() => onChange?.(!checked)}
      className={cn(
        "inline-flex h-[22px] w-[38px] shrink-0 cursor-pointer items-center",
        "rounded-full p-[2px] outline-none",
        "transition-colors duration-150 ease-out",
        "focus-visible:ring-2 focus-visible:ring-[--color-ring] focus-visible:ring-offset-2 focus-visible:ring-offset-[--color-background]",
        "disabled:cursor-not-allowed disabled:opacity-50",
        checked
          ? "bg-[var(--vk-success-dot)]"
          : "bg-[var(--vk-border-strong-3)]",
        className,
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          "block size-[18px] rounded-full bg-[var(--vk-surface)]",
          "shadow-[0_1px_2px_var(--vk-shadow-15),0_1px_3px_var(--vk-shadow-08)]",
          "transition-transform duration-200 ease-out",
        )}
        style={{ transform: checked ? "translateX(16px)" : "translateX(0)" }}
      />
    </button>
  );
}
