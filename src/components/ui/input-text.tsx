import { cn } from "@/lib/utils";

export function InputText({
  value,
  onChange,
  placeholder,
  className,
  monospace = false,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  className?: string;
  monospace?: boolean;
}) {
  return (
    <input
      type="text"
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      spellCheck={false}
      autoComplete="off"
      className={cn(
        "h-7 w-[200px] rounded-md border border-[var(--vk-border-strong)] bg-[var(--vk-surface)]",
        "px-2.5 text-[12px] text-[var(--vk-text-2)]",
        "placeholder:text-[var(--vk-text-10)]",
        "hover:border-[var(--vk-border-strong-2)] transition-colors",
        "focus:outline-none focus:ring-2 focus:ring-[--color-ring] focus:ring-offset-1 focus:border-transparent",
        monospace && "font-mono",
        className,
      )}
    />
  );
}
