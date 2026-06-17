import * as React from "react"

import { cn } from "@/lib/utils"

function SurfaceList({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="surface-list"
      className={cn(
        "overflow-hidden rounded-[6px] border bg-popover text-popover-foreground",
        className,
      )}
      {...props}
    />
  )
}

function SurfaceListItem({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="surface-list-item"
      className={cn(
        "flex min-h-14 items-center gap-2 border-b !border-border/70 px-3 py-2 transition-colors last:border-b-0 hover:bg-accent/55",
        className,
      )}
      {...props}
    />
  )
}

function SurfaceListEmpty({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="surface-list-empty"
      className={cn("p-6 text-center text-xs text-muted-foreground", className)}
      {...props}
    />
  )
}

export { SurfaceList, SurfaceListEmpty, SurfaceListItem }
