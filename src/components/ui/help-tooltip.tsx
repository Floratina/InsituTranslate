import { CircleHelp } from "lucide-react";
import { useRef, useState, type KeyboardEvent, type ReactNode } from "react";

import { cn } from "@/lib/utils";

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "./tooltip";

interface HelpTooltipProps {
  children: ReactNode;
  className?: string;
  contentClassName?: string;
}

export function HelpTooltip({ children, className, contentClassName }: HelpTooltipProps) {
  const [open, setOpen] = useState(false);
  const suppressTriggerCloseRef = useRef(false);

  function handleOpenChange(nextOpen: boolean): void {
    if (!nextOpen && suppressTriggerCloseRef.current) {
      suppressTriggerCloseRef.current = false;
      return;
    }
    setOpen(nextOpen);
  }

  function showTooltip(): void {
    suppressTriggerCloseRef.current = true;
    setOpen(true);
  }

  function hideTooltip(): void {
    suppressTriggerCloseRef.current = false;
    setOpen(false);
  }

  function handleKeyDown(event: KeyboardEvent<HTMLButtonElement>): void {
    if (event.key === "Escape") {
      hideTooltip();
    }
  }

  return (
    <TooltipProvider delayDuration={120}>
      <Tooltip open={open} onOpenChange={handleOpenChange}>
        <TooltipTrigger asChild>
          <button
            type="button"
            aria-label="显示说明"
            aria-expanded={open}
            onPointerDownCapture={showTooltip}
            onPointerLeave={hideTooltip}
            onClick={showTooltip}
            onBlur={hideTooltip}
            onKeyDown={handleKeyDown}
            className={cn(
              "inline-flex size-5 items-center justify-center rounded-[6px] text-muted-foreground transition-colors duration-150 hover:bg-accent hover:text-foreground",
              className,
            )}
          >
            <CircleHelp className="size-3.5" strokeWidth={1.8} />
          </button>
        </TooltipTrigger>
        <TooltipContent
          side="top"
          sideOffset={6}
          className={cn("max-w-88 text-left leading-5", contentClassName)}
        >
          {children}
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}
