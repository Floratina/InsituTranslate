import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

interface ActionCalloutProps {
  icon?: ReactNode;
  children: ReactNode;
  action?: ReactNode;
  className?: string;
}

export function ActionCallout({ icon, children, action, className }: ActionCalloutProps) {
  return (
    <div
      className={cn(
        "flex items-center justify-between gap-2 rounded-[6px] bg-accent/45 px-3 py-2 text-xs text-foreground max-[820px]:items-stretch max-[820px]:flex-col",
        className,
      )}
    >
      <div className="flex min-w-0 items-center gap-2">
        {icon ? <span className="shrink-0 text-primary">{icon}</span> : null}
        <div className="min-w-0">{children}</div>
      </div>
      {action ? <div className="shrink-0">{action}</div> : null}
    </div>
  );
}
