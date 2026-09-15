import { cn } from "@/lib/utils";

export function Section({
  eyebrow,
  title,
  description,
  children,
  className,
}: {
  eyebrow?: string;
  title: string;
  description?: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <section className={cn("mt-8 first:mt-0", className)}>
      <header className="mb-3 px-1">
        {eyebrow ? (
          <div className="text-[10.5px] font-semibold uppercase tracking-[0.12em] text-[var(--vk-text-alt)] mb-1.5">
            {eyebrow}
          </div>
        ) : null}
        <h2 className="text-[15px] font-semibold text-[var(--vk-text-2)]">
          {title}
        </h2>
        {description ? (
          <p className="mt-0.5 text-[12.5px] text-[var(--vk-text-7)] leading-relaxed">
            {description}
          </p>
        ) : null}
      </header>
      {children}
    </section>
  );
}

export function SettingsCard({
  children,
  className,
}: {
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "rounded-xl bg-[var(--vk-surface)] overflow-hidden",
        "border border-[var(--vk-border)]",
        "shadow-[0_1px_2px_var(--vk-shadow-03),0_1px_1px_var(--vk-shadow-04)]",
        className,
      )}
    >
      {children}
    </div>
  );
}

export function SettingsRow({
  label,
  hint,
  control,
  children,
  className,
}: {
  label: string;
  hint?: string;
  control?: React.ReactNode;
  children?: React.ReactNode;
  className?: string;
}) {
  return (
    <div
      className={cn(
        "flex items-center justify-between gap-4 px-4 py-3.5",
        "border-t border-[var(--vk-border-3)] first:border-t-0",
        "transition-colors",
        className,
      )}
    >
      <div className="min-w-0 flex-1">
        <div className="text-[13.5px] font-medium text-[var(--vk-text-2)] leading-snug">
          {label}
        </div>
        {hint ? (
          <div className="mt-0.5 text-[11.5px] text-[var(--vk-text-8)] leading-snug truncate">
            {hint}
          </div>
        ) : null}
      </div>
      <div className="flex items-center gap-2 shrink-0">
        {control ?? children}
      </div>
    </div>
  );
}
