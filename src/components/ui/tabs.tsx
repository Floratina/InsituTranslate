import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { Tabs as TabsPrimitive } from "radix-ui"
import { motion } from "motion/react"

import { cn } from "@/lib/utils"

interface TabsInteractionContextValue {
  value: string | undefined
  onValueChange: (value: string) => void
}

const TabsInteractionContext =
  React.createContext<TabsInteractionContextValue | null>(null)

function Tabs({
  className,
  value,
  defaultValue,
  onValueChange,
  orientation = "horizontal",
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Root>) {
  const isControlled = value !== undefined
  const [internalValue, setInternalValue] = React.useState<string | undefined>(defaultValue)
  const currentValue = isControlled ? value : internalValue

  const handleValueChange = React.useCallback(
    (nextValue: string) => {
      if (!isControlled) {
        setInternalValue(nextValue)
      }
      onValueChange?.(nextValue)
    },
    [isControlled, onValueChange]
  )

  const interactionContextValue = React.useMemo<TabsInteractionContextValue>(
    () => ({
      value: currentValue,
      onValueChange: handleValueChange,
    }),
    [currentValue, handleValueChange]
  )

  return (
    <TabsInteractionContext.Provider value={interactionContextValue}>
      <TabsPrimitive.Root
        data-slot="tabs"
        data-orientation={orientation}
        value={currentValue}
        onValueChange={handleValueChange}
        className={cn(
          "group/tabs flex gap-2",
          orientation === "horizontal" ? "flex-col" : "flex-row",
          className
        )}
        {...props}
      />
    </TabsInteractionContext.Provider>
  )
}

const tabsListVariants = cva(
  "group/tabs-list inline-flex w-fit items-center justify-center gap-1 rounded-[6px] p-0.5 text-muted-foreground group-data-horizontal/tabs:h-8 group-data-vertical/tabs:h-fit group-data-vertical/tabs:flex-col data-[variant=line]:rounded-none data-[variant=line]:p-0",
  {
    variants: {
      variant: {
        default: "bg-muted/70 dark:bg-muted/45",
        line: "gap-1 bg-transparent",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  }
)

function TabsList({
  className,
  variant = "default",
  ...props
}: React.ComponentProps<typeof TabsPrimitive.List> &
  VariantProps<typeof tabsListVariants>) {
  return (
    <TabsPrimitive.List
      data-slot="tabs-list"
      data-variant={variant}
      className={cn(tabsListVariants({ variant }), className)}
      {...props}
    />
  )
}

function TabsTrigger({
  className,
  value,
  disabled,
  onMouseDown,
  onMouseUp,
  onFocus,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Trigger>) {
  const interactionContext = React.useContext(TabsInteractionContext)
  const pendingMouseActivationRef = React.useRef(false)
  const skipNextFocusActivationRef = React.useRef(false)

  const handleMouseDown = React.useCallback(
    (event: React.MouseEvent<HTMLButtonElement>) => {
      onMouseDown?.(event)

      if (
        event.defaultPrevented ||
        !interactionContext ||
        disabled ||
        event.button !== 0 ||
        event.ctrlKey
      ) {
        return
      }

      event.preventDefault()
      pendingMouseActivationRef.current = true
    },
    [disabled, interactionContext, onMouseDown]
  )

  const handleMouseUp = React.useCallback(
    (event: React.MouseEvent<HTMLButtonElement>) => {
      onMouseUp?.(event)

      if (
        event.defaultPrevented ||
        !interactionContext ||
        !pendingMouseActivationRef.current ||
        disabled ||
        event.button !== 0
      ) {
        pendingMouseActivationRef.current = false
        return
      }

      pendingMouseActivationRef.current = false
      if (interactionContext.value !== value) {
        skipNextFocusActivationRef.current = true
        event.currentTarget.focus({ preventScroll: true })
        skipNextFocusActivationRef.current = false
        interactionContext.onValueChange(value)
      }
    },
    [disabled, interactionContext, onMouseUp, value]
  )

  const handleFocus = React.useCallback(
    (event: React.FocusEvent<HTMLButtonElement>) => {
      onFocus?.(event)

      if (skipNextFocusActivationRef.current) {
        event.preventDefault()
      }
    },
    [onFocus]
  )

  return (
    <TabsPrimitive.Trigger
      data-slot="tabs-trigger"
      value={value}
      disabled={disabled}
      onMouseDown={handleMouseDown}
      onMouseUp={handleMouseUp}
      onFocus={handleFocus}
      className={cn(
        "relative inline-flex h-full flex-1 items-center justify-center gap-1.5 rounded-[5px] border-0 bg-transparent px-1.5 py-0.5 text-sm font-medium whitespace-nowrap text-foreground/60 transition-[background-color,color,box-shadow] duration-150 ease-out outline-none group-data-vertical/tabs:w-full group-data-vertical/tabs:justify-start hover:bg-background/45 hover:text-foreground active:shadow-[inset_0_0_0_9999px_oklch(0_0_0_/_0.10)] active:duration-[60ms] focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-50 has-data-[icon=inline-end]:pr-1 has-data-[icon=inline-start]:pl-1 dark:text-muted-foreground dark:hover:bg-input/35 dark:hover:text-foreground dark:active:shadow-[inset_0_0_0_9999px_oklch(0_0_0_/_0.16)] [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
        "data-[state=active]:bg-enabled-accent/16 data-[state=active]:text-enabled-accent dark:data-[state=active]:bg-enabled-accent/22 dark:data-[state=active]:text-enabled-accent",
        "group-data-[variant=line]/tabs-list:h-8 group-data-[variant=line]/tabs-list:bg-transparent group-data-[variant=line]/tabs-list:hover:bg-transparent group-data-[variant=line]/tabs-list:data-[state=active]:bg-transparent dark:group-data-[variant=line]/tabs-list:data-[state=active]:bg-transparent",
        "after:absolute after:bg-enabled-accent after:opacity-0 after:transition-opacity group-data-horizontal/tabs:after:inset-x-0 group-data-horizontal/tabs:after:bottom-[-5px] group-data-horizontal/tabs:after:h-0.5 group-data-vertical/tabs:after:inset-y-0 group-data-vertical/tabs:after:-right-1 group-data-vertical/tabs:after:w-0.5 group-data-[variant=line]/tabs-list:data-[state=active]:after:opacity-100",
        className
      )}
      {...props}
    />
  )
}

function TabsContent({
  className,
  children,
  ...props
}: React.ComponentProps<typeof TabsPrimitive.Content>) {
  return (
    <TabsPrimitive.Content
      data-slot="tabs-content"
      asChild
      {...props}
    >
      <motion.div
        initial={{ opacity: 0, y: 3 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.22, ease: [0.03, 0.59, 0.19, 1] }}
        className={cn("flex-1 text-sm outline-none", className)}
      >
        {children}
      </motion.div>
    </TabsPrimitive.Content>
  )
}

export { Tabs, TabsList, TabsTrigger, TabsContent, tabsListVariants }
