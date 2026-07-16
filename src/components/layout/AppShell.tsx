import { useEffect, useState, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
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

import { WindowTitleBar } from "@/components/layout/WindowTitleBar";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

const LEGACY_SIDEBAR_STORAGE_KEY = "insitu-sidebar-v1";
const SIDEBAR_STORAGE_KEY = "insitu-sidebar-v2";
const SIDEBAR_COLLAPSED_WIDTH = 53;
const SIDEBAR_DEFAULT_WIDTH = 220;
const SIDEBAR_MAX_WIDTH = 240;
const SIDEBAR_LABEL_THRESHOLD = 104;

interface SidebarPreferences {
  collapsed: boolean;
  width: number;
  expandedWidth: number;
}

const defaultSidebarPreferences: SidebarPreferences = {
  collapsed: false,
  width: SIDEBAR_DEFAULT_WIDTH,
  expandedWidth: SIDEBAR_DEFAULT_WIDTH,
};

function clampSidebarWidth(width: number): number {
  return Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_COLLAPSED_WIDTH, width));
}

function SidebarToggleIcon({ expanded }: { expanded: boolean }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className="size-4"
    >
      <rect x="3" y="3" width="18" height="18" rx="2" />
      <rect
        x="5"
        y="5"
        width={expanded ? 7 : 4}
        height="14"
        rx="1"
        fill="currentColor"
        stroke="none"
      />
    </svg>
  );
}

function loadSidebarPreferences(): SidebarPreferences {
  window.localStorage.removeItem(LEGACY_SIDEBAR_STORAGE_KEY);
  const stored = window.localStorage.getItem(SIDEBAR_STORAGE_KEY);
  if (!stored) return defaultSidebarPreferences;

  const parsed = JSON.parse(stored) as Partial<SidebarPreferences>;
  const expandedWidth = clampSidebarWidth(
    typeof parsed.expandedWidth === "number" ? parsed.expandedWidth : SIDEBAR_DEFAULT_WIDTH,
  );
  const collapsed = parsed.collapsed === true;
  const width = collapsed
    ? SIDEBAR_COLLAPSED_WIDTH
    : clampSidebarWidth(typeof parsed.width === "number" ? parsed.width : expandedWidth);

  return { collapsed, width, expandedWidth };
}

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

const sidebarInactiveButtonClass =
  "text-muted-foreground hover:bg-[var(--button-ghost-hover-bg)] hover:text-foreground active:bg-[var(--button-ghost-pressed-bg)] active:text-foreground active:duration-[60ms]";

interface SidebarNavigationButtonProps {
  label: string;
  icon: LucideIcon;
  active: boolean;
  labelsVisible: boolean;
  labelsOccupySpace: boolean;
  onClick?: () => void;
}

function SidebarNavigationButton({
  label,
  icon: Icon,
  active,
  labelsVisible,
  labelsOccupySpace,
  onClick,
}: SidebarNavigationButtonProps) {
  const button = (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      aria-current={active ? "page" : undefined}
      className={cn(
        "flex h-9 items-center justify-start gap-2 rounded-[6px] border-0 bg-transparent pl-2.5 pr-1.5 text-left text-sm outline-none transition-[width,background-color,color,box-shadow] duration-150 ease-out focus-visible:ring-3 focus-visible:ring-ring/40",
        labelsOccupySpace ? "w-full" : "w-9",
        active
          ? "bg-enabled-accent/20 font-medium text-enabled-accent hover:bg-enabled-accent/26 hover:text-enabled-accent active:bg-enabled-accent/26 active:text-enabled-accent active:shadow-[inset_0_0_0_999px_rgb(0_0_0_/_0.10)] active:duration-[60ms] dark:bg-accent dark:text-accent-foreground dark:hover:bg-[color-mix(in_oklch,var(--accent),var(--foreground)_8%)] dark:hover:text-accent-foreground dark:active:bg-[color-mix(in_oklch,var(--accent),black_10%)] dark:active:text-accent-foreground dark:active:shadow-none"
          : sidebarInactiveButtonClass,
      )}
    >
      <Icon className="size-4 shrink-0" strokeWidth={1.8} />
      <span
        aria-hidden={!labelsVisible}
        className={cn(
          "min-w-0 overflow-hidden whitespace-nowrap transition-opacity duration-200 ease-out",
          labelsOccupySpace ? "max-w-24" : "max-w-0",
          labelsVisible ? "opacity-100" : "pointer-events-none opacity-0",
        )}
      >
        {label}
      </span>
    </button>
  );

  return (
    <Tooltip open={labelsVisible ? false : undefined}>
      <TooltipTrigger asChild>{button}</TooltipTrigger>
      <TooltipContent side="right" sideOffset={8}>{label}</TooltipContent>
    </Tooltip>
  );
}

export function AppShell({ children, activePage, onNavigate }: AppShellProps) {
  const [sidebar, setSidebar] = useState<SidebarPreferences>(loadSidebarPreferences);
  const [resizing, setResizing] = useState<boolean>(false);
  const shouldShowLabels = !sidebar.collapsed && sidebar.width >= SIDEBAR_LABEL_THRESHOLD;
  const [labelsVisible, setLabelsVisible] = useState<boolean>(shouldShowLabels);
  const [labelsOccupySpace, setLabelsOccupySpace] = useState<boolean>(shouldShowLabels);

  useEffect(() => {
    window.localStorage.setItem(SIDEBAR_STORAGE_KEY, JSON.stringify(sidebar));
  }, [sidebar]);

  useEffect(() => {
    if (shouldShowLabels) {
      setLabelsOccupySpace(true);
      const frame = window.requestAnimationFrame(() => setLabelsVisible(true));
      return () => window.cancelAnimationFrame(frame);
    }

    setLabelsVisible(false);
    const timeout = window.setTimeout(() => setLabelsOccupySpace(false), 200);
    return () => window.clearTimeout(timeout);
  }, [shouldShowLabels]);

  useEffect(() => {
    if (!resizing) return;

    const previousCursor = document.body.style.cursor;
    const previousUserSelect = document.body.style.userSelect;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";

    return () => {
      document.body.style.cursor = previousCursor;
      document.body.style.userSelect = previousUserSelect;
    };
  }, [resizing]);

  function toggleSidebar(): void {
    setSidebar((current) => {
      if (current.collapsed) {
        return {
          ...current,
          collapsed: false,
          width: current.expandedWidth,
        };
      }

      return {
        ...current,
        collapsed: true,
        width: SIDEBAR_COLLAPSED_WIDTH,
      };
    });
  }

  function startResize(event: ReactPointerEvent<HTMLDivElement>): void {
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    setResizing(true);
  }

  function resizeSidebar(event: ReactPointerEvent<HTMLDivElement>): void {
    if (!resizing) return;
    const width = clampSidebarWidth(event.clientX);

    setSidebar((current) => ({
      collapsed: width === SIDEBAR_COLLAPSED_WIDTH,
      width,
      expandedWidth: width >= SIDEBAR_LABEL_THRESHOLD ? width : current.expandedWidth,
    }));
  }

  function stopResize(event: ReactPointerEvent<HTMLDivElement>): void {
    if (!resizing) return;
    event.currentTarget.releasePointerCapture(event.pointerId);
    setResizing(false);
  }

  return (
    <div className="flex h-dvh w-full min-w-0 overflow-hidden bg-background text-foreground">
      <TooltipProvider delayDuration={500}>
      <aside
        className={cn(
          "relative flex h-full shrink-0 flex-col overflow-hidden border-r bg-sidebar p-2",
          !resizing && "transition-[width] duration-200 ease-[cubic-bezier(0.22,0.61,0.36,0.99)]",
        )}
        style={{ width: sidebar.width }}
      >
        <div className="relative z-10 flex h-10 shrink-0 items-center">
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                type="button"
                variant="ghost"
                size="icon-lg"
                aria-label={sidebar.collapsed ? "展开" : "收起"}
                className={cn(
                  "size-9 border-0! focus-visible:border-0!",
                  sidebarInactiveButtonClass,
                )}
                onClick={toggleSidebar}
              >
                <SidebarToggleIcon expanded={!sidebar.collapsed} />
              </Button>
            </TooltipTrigger>
            <TooltipContent side="right" sideOffset={8}>
              {sidebar.collapsed ? "展开" : "收起"}
            </TooltipContent>
          </Tooltip>
        </div>
        <img
          src="/logo.png"
          alt=""
          aria-hidden="true"
          className="pointer-events-none absolute bottom-25 left-1/2 z-0 size-56 max-w-none -translate-x-1/2 select-none opacity-[0.12] dark:opacity-[0.06]"
        />
        <nav className="relative z-10 mt-2 grid gap-1">
          {navigationItems.map((item) => (
            <SidebarNavigationButton
              key={item.label}
              label={item.label}
              icon={item.icon}
              active={item.page === activePage}
              labelsVisible={labelsVisible}
              labelsOccupySpace={labelsOccupySpace}
              onClick={item.page ? () => onNavigate(item.page!) : undefined}
            />
          ))}
        </nav>
        <div className="relative z-10 mt-auto pt-2">
          <SidebarNavigationButton
            label="设置"
            icon={Settings}
            active={activePage === "settings"}
            labelsVisible={labelsVisible}
            labelsOccupySpace={labelsOccupySpace}
            onClick={() => onNavigate("settings")}
          />
        </div>
        <div
          role="separator"
          aria-orientation="vertical"
          aria-label="调整侧边栏宽度"
          className={cn(
            "absolute inset-y-0 right-0 z-10 w-1 cursor-col-resize touch-none transition-colors duration-150 hover:bg-ring/45",
            resizing && "bg-ring/60",
          )}
          onPointerDown={startResize}
          onPointerMove={resizeSidebar}
          onPointerUp={stopResize}
          onPointerCancel={stopResize}
        />
      </aside>
      </TooltipProvider>
      <div className="flex min-w-0 flex-1 flex-col overflow-hidden">
        <WindowTitleBar />
        <div className="flex min-h-0 flex-1 overflow-hidden">{children}</div>
      </div>
    </div>
  );
}
