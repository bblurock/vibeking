import appIconUrl from "@/assets/vibeking-app-icon.png";
import openVoice from "@/brand/open-voice.json";

type Props = {
  className?: string;
  tone?: "light" | "mono";
};

// Shared D2 geometry also generates the monochrome menu-bar asset.
export function OpenVoiceSymbol({ className, tone = "mono" }: Props) {
  return (
    <svg
      viewBox={openVoice.viewBox}
      fill={tone === "light" ? "white" : "currentColor"}
      className={className}
      aria-hidden="true"
      focusable="false"
    >
      {openVoice.paths.map((d) => <path key={d} d={d} />)}
    </svg>
  );
}

// The same artwork is used in onboarding, the sidebar, home, and recording bar.
export function BrandChip({
  size = "md",
  className = "",
}: {
  size?: "sm" | "md";
  className?: string;
}) {
  const dims = size === "sm" ? "size-5" : "size-10";
  return (
    <img
      src={appIconUrl}
      alt=""
      aria-hidden="true"
      draggable={false}
      className={`${dims} shrink-0 select-none ${className}`}
    />
  );
}
