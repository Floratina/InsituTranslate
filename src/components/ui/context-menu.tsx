import * as React from "react"
import { Check, ChevronRight } from "lucide-react"
import { ContextMenu as ContextMenuPrimitive } from "radix-ui"

import { ScrollArea } from "@/components/ui/scroll-area"
import { cn } from "@/lib/utils"

const ContextMenu = ContextMenuPrimitive.Root
const ContextMenuTrigger = ContextMenuPrimitive.Trigger
const ContextMenuSub = ContextMenuPrimitive.Sub
const ContextMenuSubTrigger = ContextMenuPrimitive.SubTrigger

function ContextMenuSurface({
  className,
  children,
}: {
  className?: string
  children: React.ReactNode
}) {
  return (
    <div
      style={{
        transformOrigin: "var(--radix-context-menu-content-transform-origin)",
      }}
      className={cn(
        "floating-menu-enter-y overflow-hidden rounded-[6px] border bg-popover text-popover-foreground shadow-lg transform-gpu",
        className,
      )}
    >
      <ScrollArea
        className="max-h-[min(22rem,var(--radix-context-menu-content-available-height))]"
        viewportClassName="h-auto max-h-[min(22rem,var(--radix-context-menu-content-available-height))] overscroll-contain"
      >
        {children}
      </ScrollArea>
    </div>
  )
}

function ContextMenuContent({
  className,
  children,
  ...props
}: React.ComponentProps<typeof ContextMenuPrimitive.Content>) {
  return (
    <ContextMenuPrimitive.Portal>
      <ContextMenuPrimitive.Content
        className="z-[70] min-w-40"
        {...props}
      >
        <ContextMenuSurface className={className}>
          {children}
        </ContextMenuSurface>
      </ContextMenuPrimitive.Content>
    </ContextMenuPrimitive.Portal>
  )
}

function ContextMenuSubContent({
  className,
  children,
  ...props
}: React.ComponentProps<typeof ContextMenuPrimitive.SubContent>) {
  return (
    <ContextMenuPrimitive.Portal>
      <ContextMenuPrimitive.SubContent
        className="z-[80] min-w-36"
        sideOffset={4}
        {...props}
      >
        <ContextMenuSurface className={className}>
          {children}
        </ContextMenuSurface>
      </ContextMenuPrimitive.SubContent>
    </ContextMenuPrimitive.Portal>
  )
}

function ContextMenuItem({
  className,
  ...props
}: React.ComponentProps<typeof ContextMenuPrimitive.Item>) {
  return (
    <ContextMenuPrimitive.Item
      className={cn(
        "flex h-8 cursor-default select-none items-center gap-2 px-3 text-sm outline-none focus:bg-accent data-disabled:pointer-events-none data-disabled:opacity-50",
        className,
      )}
      {...props}
    />
  )
}

function ContextMenuCheckboxItem({
  className,
  children,
  ...props
}: React.ComponentProps<typeof ContextMenuPrimitive.CheckboxItem>) {
  return (
    <ContextMenuPrimitive.CheckboxItem
      className={cn(
        "relative flex h-8 cursor-default select-none items-center gap-2 px-3 pr-8 text-sm outline-none focus:bg-accent",
        className,
      )}
      {...props}
    >
      {children}
      <ContextMenuPrimitive.ItemIndicator className="absolute right-2">
        <Check className="size-3.5" />
      </ContextMenuPrimitive.ItemIndicator>
    </ContextMenuPrimitive.CheckboxItem>
  )
}

function ContextMenuSubTriggerItem({
  className,
  children,
  ...props
}: React.ComponentProps<typeof ContextMenuPrimitive.SubTrigger>) {
  return (
    <ContextMenuPrimitive.SubTrigger
      className={cn(
        "flex h-8 cursor-default select-none items-center gap-2 px-3 text-sm outline-none focus:bg-accent data-[state=open]:bg-accent",
        className,
      )}
      {...props}
    >
      {children}
      <ChevronRight className="ml-auto size-3.5" />
    </ContextMenuPrimitive.SubTrigger>
  )
}

function ContextMenuSeparator({
  className,
  ...props
}: React.ComponentProps<typeof ContextMenuPrimitive.Separator>) {
  return (
    <ContextMenuPrimitive.Separator
      className={cn("my-0.5 h-0 shrink-0 border-t border-border/75 bg-transparent", className)}
      {...props}
    />
  )
}

export {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuCheckboxItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubTrigger,
  ContextMenuSubTriggerItem,
  ContextMenuSubContent,
}
