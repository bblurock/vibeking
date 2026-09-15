import { cn } from "@/lib/utils";

export function Kbd({
  children,
  className,
  title,
}: {
  children: React.ReactNode;
  className?: string;
  title?: string;
}) {
  return (
    <kbd
      title={title}
      className={cn(
        "inline-flex h-7 min-w-7 items-center justify-center gap-1 px-2",
        "rounded-md border border-[var(--vk-border-strong)] bg-[var(--vk-canvas-3)]",
        "text-[12px] leading-none text-[var(--vk-text-alt-2)]",
        "font-medium tracking-tight",
        "shadow-[inset_0_-1px_0_var(--vk-border-strong),0_1px_0_var(--vk-surface)]",
        className,
      )}
    >
      {children}
    </kbd>
  );
}
