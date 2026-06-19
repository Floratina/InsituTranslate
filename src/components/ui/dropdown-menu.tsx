import * as React from "react"
import { ChevronRight } from "lucide-react"
import { DropdownMenu as DropdownMenuPrimitive } from "radix-ui"
import { motion } from "motion/react"

import { cn } from "@/lib/utils"

const DropdownMenu = DropdownMenuPrimitive.Root
const DropdownMenuTrigger = DropdownMenuPrimitive.Trigger
const DropdownMenuSub = DropdownMenuPrimitive.Sub

function DropdownMenuContent({
  className,
  children,
  sideOffset = 4,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Content>) {
  return (
    <DropdownMenuPrimitive.Portal>
      <DropdownMenuPrimitive.Content
        sideOffset={sideOffset}
        className="z-[70] min-w-36"
        {...props}
      >
        <motion.div
          initial={{ opacity: 0, y: -4, scale: 0.985 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          transition={{ duration: 0.22, ease: [0.03, 0.59, 0.19, 1] }}
          className={cn(
            "scrollbar-subtle max-h-[min(22rem,var(--radix-dropdown-menu-content-available-height))] overflow-x-hidden overflow-y-auto overscroll-contain rounded-[6px] border bg-popover text-popover-foreground shadow-lg",
            className,
          )}
        >
          {children}
        </motion.div>
      </DropdownMenuPrimitive.Content>
    </DropdownMenuPrimitive.Portal>
  )
}

function DropdownMenuSubContent({
  className,
  children,
  sideOffset = 4,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.SubContent>) {
  return (
    <DropdownMenuPrimitive.Portal>
      <DropdownMenuPrimitive.SubContent
        sideOffset={sideOffset}
        className="z-[80] min-w-36"
        {...props}
      >
        <motion.div
          initial={{ opacity: 0, x: -3 }}
          animate={{ opacity: 1, x: 0 }}
          transition={{ duration: 0.12, ease: [0.03, 0.59, 0.19, 1] }}
          className={cn(
            "origin-[var(--radix-dropdown-menu-content-transform-origin)] overflow-hidden rounded-[6px] border bg-popover text-popover-foreground shadow-lg transform-gpu",
            className,
          )}
        >
          {children}
        </motion.div>
      </DropdownMenuPrimitive.SubContent>
    </DropdownMenuPrimitive.Portal>
  )
}

function DropdownMenuItem({
  className,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Item>) {
  return (
    <DropdownMenuPrimitive.Item
      className={cn(
        "flex h-8 cursor-default select-none items-center gap-2 px-3 text-sm outline-none focus:bg-accent data-disabled:pointer-events-none data-disabled:opacity-50",
        className,
      )}
      {...props}
    />
  )
}

function DropdownMenuSubTriggerItem({
  className,
  children,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.SubTrigger>) {
  return (
    <DropdownMenuPrimitive.SubTrigger
      className={cn(
        "flex h-8 cursor-default select-none items-center gap-2 px-3 text-sm outline-none focus:bg-accent data-[state=open]:bg-accent",
        className,
      )}
      {...props}
    >
      {children}
      <ChevronRight className="ml-auto size-3.5" />
    </DropdownMenuPrimitive.SubTrigger>
  )
}

function DropdownMenuSeparator({
  className,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Separator>) {
  return (
    <DropdownMenuPrimitive.Separator
      className={cn("my-0.5 h-0 shrink-0 border-t border-border/75 bg-transparent", className)}
      {...props}
    />
  )
}

export {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTriggerItem,
  DropdownMenuTrigger,
}
