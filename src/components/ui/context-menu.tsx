import * as React from "react"
import { Check, ChevronRight } from "lucide-react"
import { ContextMenu as ContextMenuPrimitive } from "radix-ui"
import { motion } from "motion/react"

import { cn } from "@/lib/utils"

const ContextMenu = ContextMenuPrimitive.Root
const ContextMenuTrigger = ContextMenuPrimitive.Trigger
const ContextMenuSub = ContextMenuPrimitive.Sub
const ContextMenuSubTrigger = ContextMenuPrimitive.SubTrigger

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
        <motion.div
          initial={{ opacity: 0, scale: 0.985 }}
          animate={{ opacity: 1, scale: 1 }}
          transition={{ duration: 0.15, ease: [0.03, 0.59, 0.19, 1] }}
          className={cn(
            "origin-[var(--radix-context-menu-content-transform-origin)] overflow-hidden rounded-[6px] border bg-popover text-popover-foreground shadow-lg transform-gpu",
            className,
          )}
        >
          {children}
        </motion.div>
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
        <motion.div
          initial={{ opacity: 0, x: -3 }}
          animate={{ opacity: 1, x: 0 }}
          transition={{ duration: 0.12, ease: [0.03, 0.59, 0.19, 1] }}
          className={cn(
            "origin-[var(--radix-context-menu-content-transform-origin)] overflow-hidden rounded-[6px] border bg-popover text-popover-foreground shadow-lg transform-gpu",
            className,
          )}
        >
          {children}
        </motion.div>
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
