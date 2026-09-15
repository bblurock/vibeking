import { cn } from "@/lib/utils";

/**
 * The app's one determinate loading bar. Extracted from the inline bar in
 * EnginePicker so the onboarding wizard's top stepper track and the
 * model-download bar share a single visual definition.
 *
 *  - `value` is clamped to 0–100.
 *  - `gradient` swaps the flat accent fill for the brand accent→light sweep
 *    used on the onboarding bars.
 *  - `className` styles the track (height/width/bg); `barClassName` the fill.
 */
export function ProgressBar({
  value,
  gradient = false,
  className,
  barClassName,
}: {
  value: number;
  gradient?: boolean;
  className?: string;
  barClassName?: string;
}) {
  const pct = Math.max(0, Math.min(100, value));
  return (
    <div
      role="progressbar"
      aria-valuenow={Math.round(pct)}
      aria-valuemin={0}
      aria-valuemax={100}
      className={cn(
        "h-1 w-full overflow-hidden rounded-full bg-[var(--vk-surface-3)]",
        className,
      )}
    >
      <div
        className={cn(
          "h-full rounded-full transition-[width] duration-300 ease-out motion-reduce:transition-none",
          gradient
            ? "bg-gradient-to-r from-[var(--vk-accent-9)] to-[var(--vk-accent)]"
            : "bg-[var(--vk-accent)]",
          barClassName,
        )}
        style={{ width: `${pct}%` }}
      />
    </div>
  );
}
