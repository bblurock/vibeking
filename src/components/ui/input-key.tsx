import { useState } from "react";
import { Eye, EyeOff } from "lucide-react";
import { cn } from "@/lib/utils";

export function InputKey({
  value,
  onChange,
  placeholder = "sk-…",
  className,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  className?: string;
}) {
  const [show, setShow] = useState(false);

  return (
    <div className={cn("relative inline-flex items-center", className)}>
      <input
        type={show ? "text" : "password"}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        spellCheck={false}
        autoComplete="off"
        className={cn(
          "h-7 w-[180px] rounded-md border border-[var(--vk-border-strong)] bg-[var(--vk-surface)]",
          "pl-2.5 pr-8 text-[12px] font-mono text-[var(--vk-text-2)]",
          "placeholder:text-[var(--vk-text-10)] placeholder:font-sans",
          "hover:border-[var(--vk-border-strong-2)] transition-colors",
          "focus:outline-none focus:ring-2 focus:ring-[--color-ring] focus:ring-offset-1 focus:border-transparent",
        )}
      />
      <button
        type="button"
        onClick={() => setShow((s) => !s)}
        aria-label={show ? "Hide key" : "Show key"}
        className="absolute right-1.5 inline-flex h-5 w-5 items-center justify-center rounded text-[var(--vk-text-8)] hover:text-[var(--vk-text-2)] hover:bg-[var(--vk-surface-3)] transition-colors"
      >
        {show ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
      </button>
    </div>
  );
}
