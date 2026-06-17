import type { ReactNode } from "react";
import {
  BookOpen,
  Bot,
  FilePenLine,
  LayoutDashboard,
  ListChecks,
  Network,
  Settings,
  type LucideIcon,
} from "lucide-react";
import { motion } from "motion/react";

import { WindowTitleBar } from "@/components/layout/WindowTitleBar";
import { cn } from "@/lib/utils";

export type AppPage =
  | "start"
  | "tasks"
  | "proofreading"
  | "glossary"
  | "providers"
  | "assistants"
  | "settings";

interface AppShellProps {
  children: ReactNode;
  activePage: AppPage;
  onNavigate: (page: AppPage) => void;
}

interface NavigationItem {
  label: string;
  icon: LucideIcon;
  page?: AppPage;
}

const navigationItems: NavigationItem[] = [
  { label: "开始", icon: LayoutDashboard, page: "start" },
  { label: "任务", icon: ListChecks, page: "tasks" },
  { label: "校对", icon: FilePenLine, page: "proofreading" },
  { label: "术语表", icon: BookOpen, page: "glossary" },
  { label: "提供商", icon: Network, page: "providers" },
  { label: "助手", icon: Bot, page: "assistants" },
];

const navigationTransition = {
  duration: 0.20,
  ease: [0.03, 0.59, 0.19, 1] as const,
};

interface SidebarNavigationButtonProps {
  label: string;
  icon: LucideIcon;
  active: boolean;
  onClick?: () => void;
}

function SidebarNavigationButton({
  label,
  icon: Icon,
  active,
  onClick,
}: SidebarNavigationButtonProps) {
  return (
    <motion.button
      type="button"
      whileHover={{ x: 2 }}
      whileTap={{ scale: 0.99 }}
      transition={navigationTransition}
      onClick={onClick}
      className={cn(
        "flex h-9 w-full items-center gap-2 rounded-[6px] px-2 text-left text-sm text-muted-foreground",
        active && "bg-accent font-medium text-accent-foreground",
      )}
    >
      <Icon className="size-4" strokeWidth={1.8} />
      {label}
    </motion.button>
  );
}

export function AppShell({ children, activePage, onNavigate }: AppShellProps) {
  return (
    <div className="flex h-dvh w-full min-w-0 overflow-hidden bg-background text-foreground">
      <aside className="flex h-full w-[clamp(10.5rem,22vw,12.5rem)] shrink-0 flex-col border-r bg-sidebar p-3">
        <div className="flex items-center gap-2 px-1 py-2">
          <img
            src="/logo.png"
            alt="InsituTranslate"
            className="size-9 rounded-[0px]"
          />
          <div className="min-w-0">
            <div className="truncate text-[16px] font-semibold">InsituTranslate</div>
          </div>
        </div>
        <div className="mt-1 px-1 text-2xs text-muted-foreground">工作区</div>
        <nav className="mt-2 grid gap-1">
          {navigationItems.map((item) => (
            <SidebarNavigationButton
              key={item.label}
              label={item.label}
              icon={item.icon}
              active={item.page === activePage}
              onClick={item.page ? () => onNavigate(item.page!) : undefined}
            />
          ))}
        </nav>
        <div className="mt-auto pt-2">
          <SidebarNavigationButton
            label="设置"
            icon={Settings}
            active={activePage === "settings"}
            onClick={() => onNavigate("settings")}
          />
        </div>
      </aside>
      <div className="flex min-w-0 flex-1 flex-col overflow-hidden">
        <WindowTitleBar />
        <div className="flex min-h-0 flex-1 overflow-hidden">{children}</div>
      </div>
    </div>
  );
}
