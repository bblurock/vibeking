import { cn } from "@/lib/utils";

export type Status = "ready" | "recording" | "transcribing" | "polishing" | "translating";

export function TranslateBadge({ on, target }: { on: boolean; target: string }) {
  if (!on) return null;
  return (
    <div
      className="inline-flex items-center gap-1.5 h-6 pl-2 pr-2.5 rounded-full text-[11px] font-medium"
      style={{
        background: "var(--vk-success-soft-bg)",
        color: "var(--vk-success-4)",
      }}
      title={`Translating to ${target}. Press ⌥/ to disable.`}
    >
      <span className="size-1.5 rounded-full" style={{ background: "var(--vk-success)" }} />
      Translating → {target}
    </div>
  );
}

export function StatusPill({ status, label }: { status: Status; label: string }) {
  const dot =
    status === "ready"
      ? "bg-[var(--vk-success-dot)]"
      : status === "recording"
        ? "bg-[var(--vk-danger-3)] animate-vibeking-pulse"
        : "bg-[var(--vk-warning-dot)] animate-vibeking-pulse";

  const halo =
    status === "ready"
      ? "shadow-[0_0_0_3px_var(--vk-success-ring)]"
      : status === "recording"
        ? "shadow-[0_0_0_3px_var(--vk-danger-ring)]"
        : "shadow-[0_0_0_3px_var(--vk-warning-ring)]";

  return (
    <div className="inline-flex items-center gap-2 text-[12px] text-[var(--vk-text-6)]">
      <span className={cn("size-1.5 rounded-full", dot, halo)} />
      <span className="font-medium">{label}</span>
    </div>
  );
}
