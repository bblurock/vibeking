import { NavLink } from "react-router-dom";
import {
  Home as HomeIcon,
  BookText,
  History as HistoryIcon,
  Sparkles,
  MessageSquareText,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";
import { BrandChip } from "@/components/BrandMark";
import { HelpButton } from "@/components/HelpDialog";

type Item = {
  to: string;
  icon: React.ComponentType<{ className?: string }>;
  labelKey: "nav.home" | "nav.hotwords" | "nav.history" | "nav.corrections";
};

const ITEMS: Item[] = [
  { to: "/", icon: HomeIcon, labelKey: "nav.home" },
  { to: "/hotwords", icon: BookText, labelKey: "nav.hotwords" },
  { to: "/history", icon: HistoryIcon, labelKey: "nav.history" },
  { to: "/corrections", icon: Sparkles, labelKey: "nav.corrections" },
];

function SideLink({
  to,
  icon: Icon,
  label,
  end,
}: {
  to: string;
  icon: React.ComponentType<{ className?: string }>;
  label: string;
  end?: boolean;
}) {
  return (
    <NavLink
      to={to}
      end={end}
      className={({ isActive }) =>
        cn(
          "flex items-center gap-2.5 rounded-md px-2.5 py-1.5",
          "text-[13px] font-medium transition-colors duration-100",
          isActive
            ? "bg-[var(--vk-accent-soft-bg)] text-[var(--vk-accent-3)]"
            : "text-[var(--vk-text-5)] hover:bg-[var(--vk-surface-3)] hover:text-[var(--vk-text-2)]",
        )
      }
    >
      {({ isActive }) => (
        <>
          <Icon
            className={cn(
              "size-[14px]",
              isActive ? "text-[var(--vk-accent)]" : "text-[var(--vk-text-7)]",
            )}
          />
          <span>{label}</span>
        </>
      )}
    </NavLink>
  );
}

export function Sidebar() {
  const { t } = useTranslation();
  return (
    <aside className="flex w-48 shrink-0 flex-col border-r border-[var(--vk-border-2)] bg-[var(--vk-sidebar)]">
      <div className="h-12 flex items-center justify-between px-4 border-b border-[var(--vk-border-3)]">
        <div className="flex items-center gap-2">
          <BrandChip size="sm" />
          <span className="text-[13px] font-semibold text-[var(--vk-text-2)] tracking-tight">
            Vibeking
          </span>
        </div>
        <HelpButton />
      </div>

      <nav className="p-2 space-y-0.5">
        {ITEMS.map(({ to, icon, labelKey }) => (
          <SideLink
            key={to}
            to={to}
            icon={icon}
            label={t(labelKey)}
            end={to === "/"}
          />
        ))}
      </nav>

      {/* Feedback is pinned to the bottom — reachable from anywhere, out of the
          way of the primary navigation. */}
      <div className="mt-auto p-2 border-t border-[var(--vk-border-3)]">
        <SideLink
          to="/support"
          icon={MessageSquareText}
          label={t("feedback.nav")}
        />
      </div>
    </aside>
  );
}
