import * as React from "react"
import { Check, ChevronDown } from "lucide-react"
import { Select as SelectPrimitive } from "radix-ui"
import { motion } from "motion/react"

import { cn } from "@/lib/utils"

const Select = SelectPrimitive.Root
const SelectValue = SelectPrimitive.Value
const selectTriggerClassName =
  "inline-flex h-8 w-full items-center gap-2 overflow-hidden rounded-[6px] border bg-background px-2.5 text-sm whitespace-nowrap outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:opacity-50"
const selectItemClassName =
  "relative flex h-8 w-full cursor-default select-none items-center overflow-hidden py-1 pr-8 pl-3 text-left text-sm whitespace-nowrap outline-none hover:bg-accent focus:bg-accent data-disabled:opacity-50"

function SelectTrigger({
  className,
  children,
  ...props
}: React.ComponentProps<typeof SelectPrimitive.Trigger>) {
  return (
    <SelectPrimitive.Trigger
      className={cn(
        selectTriggerClassName,
        className,
      )}
      {...props}
    >
      <span className="min-w-0 flex-1 overflow-hidden text-left text-clip whitespace-nowrap">
        {children}
      </span>
      <SelectPrimitive.Icon className="shrink-0">
        <ChevronDown className="size-4 text-muted-foreground" strokeWidth={1.8} />
      </SelectPrimitive.Icon>
    </SelectPrimitive.Trigger>
  )
}

interface SelectContentProps
  extends React.ComponentProps<typeof SelectPrimitive.Content> {
  viewportClassName?: string
}

function SelectContent({
  className,
  children,
  viewportClassName,
  ...props
}: SelectContentProps) {
  return (
    <SelectPrimitive.Portal>
      <SelectPrimitive.Content
        className="z-[60] min-w-[var(--radix-select-trigger-width)]"
        position="popper"
        sideOffset={4}
        {...props}
      >
        <motion.div
          initial={{ opacity: 0, y: -4, scale: 0.985 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          transition={{ duration: 0.22, ease: [0.03, 0.59, 0.19, 1] }}
          className={cn(
            "overflow-hidden rounded-[6px] border bg-popover text-popover-foreground shadow-lg",
            className,
          )}
        >
          <SelectPrimitive.Viewport
            className={cn(
              "scrollbar-subtle max-h-[min(22rem,var(--radix-select-content-available-height))] overflow-x-hidden overflow-y-auto overscroll-contain",
              viewportClassName,
            )}
          >
            {children}
          </SelectPrimitive.Viewport>
        </motion.div>
      </SelectPrimitive.Content>
    </SelectPrimitive.Portal>
  )
}

function SelectItem({
  className,
  children,
  ...props
}: React.ComponentProps<typeof SelectPrimitive.Item>) {
  return (
    <SelectPrimitive.Item
      className={cn(
        selectItemClassName,
        className,
      )}
      {...props}
    >
      <SelectPrimitive.ItemText>{children}</SelectPrimitive.ItemText>
      <SelectPrimitive.ItemIndicator className="absolute right-3">
        <Check className="size-3.5" strokeWidth={1.8} />
      </SelectPrimitive.ItemIndicator>
    </SelectPrimitive.Item>
  )
}

export {
  Select,
  SelectValue,
  SelectTrigger,
  SelectContent,
  SelectItem,
  selectTriggerClassName,
  selectItemClassName,
}
