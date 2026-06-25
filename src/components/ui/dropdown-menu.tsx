import * as React from "react"
import { ChevronRight } from "lucide-react"
import { DropdownMenu as DropdownMenuPrimitive } from "radix-ui"

import { ScrollArea } from "@/components/ui/scroll-area"
import { cn } from "@/lib/utils"

interface DropdownMenuControlContextValue {
  open: boolean
  setOpen: (open: boolean) => void
}

const DropdownMenuControlContext =
  React.createContext<DropdownMenuControlContextValue | null>(null)

function DropdownMenu({
  open: openProp,
  defaultOpen,
  onOpenChange,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Root>) {
  const [uncontrolledOpen, setUncontrolledOpen] = React.useState(defaultOpen ?? false)
  const open = openProp ?? uncontrolledOpen

  const setOpen = React.useCallback(
    (nextOpen: boolean) => {
      if (openProp === undefined) {
        setUncontrolledOpen(nextOpen)
      }
      onOpenChange?.(nextOpen)
    },
    [onOpenChange, openProp],
  )

  return (
    <DropdownMenuControlContext.Provider value={{ open, setOpen }}>
      <DropdownMenuPrimitive.Root open={open} onOpenChange={setOpen} {...props} />
    </DropdownMenuControlContext.Provider>
  )
}

const DropdownMenuTrigger = React.forwardRef<
  React.ElementRef<typeof DropdownMenuPrimitive.Trigger>,
  React.ComponentPropsWithoutRef<typeof DropdownMenuPrimitive.Trigger>
>(
  (
    {
      disabled,
      onClick,
      onPointerCancel,
      onPointerDown,
      onPointerUp,
      ...props
    },
    ref,
  ) => {
    const control = React.useContext(DropdownMenuControlContext)
    const pendingPointerRef = React.useRef(false)
    const pendingPointerIdRef = React.useRef<number | null>(null)
    const suppressNextClickRef = React.useRef(false)

    const clearPendingPointer = React.useCallback((target?: HTMLButtonElement) => {
      if (
        target &&
        pendingPointerIdRef.current !== null &&
        target.hasPointerCapture(pendingPointerIdRef.current)
      ) {
        target.releasePointerCapture(pendingPointerIdRef.current)
      }
      pendingPointerRef.current = false
      pendingPointerIdRef.current = null
    }, [])

    const handlePointerDown: React.PointerEventHandler<HTMLButtonElement> = (event) => {
      onPointerDown?.(event)

      if (
        event.defaultPrevented ||
        disabled ||
        control === null ||
        event.button !== 0 ||
        event.ctrlKey
      ) {
        return
      }

      pendingPointerRef.current = true
      pendingPointerIdRef.current = event.pointerId
      suppressNextClickRef.current = true
      event.currentTarget.setPointerCapture(event.pointerId)
      event.preventDefault()
    }

    const handlePointerUp: React.PointerEventHandler<HTMLButtonElement> = (event) => {
      onPointerUp?.(event)

      if (event.defaultPrevented || disabled || control === null || !pendingPointerRef.current) {
        return
      }

      const rect = event.currentTarget.getBoundingClientRect()
      const releasedInside =
        event.clientX >= rect.left &&
        event.clientX <= rect.right &&
        event.clientY >= rect.top &&
        event.clientY <= rect.bottom

      clearPendingPointer(event.currentTarget)
      if (releasedInside) {
        control.setOpen(!control.open)
      }
    }

    const handlePointerCancel: React.PointerEventHandler<HTMLButtonElement> = (event) => {
      clearPendingPointer(event.currentTarget)
      onPointerCancel?.(event)
    }

    const handleClick: React.MouseEventHandler<HTMLButtonElement> = (event) => {
      onClick?.(event)

      if (!suppressNextClickRef.current) {
        return
      }

      suppressNextClickRef.current = false
      event.preventDefault()
      event.stopPropagation()
    }

    return (
      <DropdownMenuPrimitive.Trigger
        {...props}
        ref={ref}
        disabled={disabled}
        onClick={handleClick}
        onPointerCancel={handlePointerCancel}
        onPointerDown={handlePointerDown}
        onPointerUp={handlePointerUp}
      />
    )
  },
)
DropdownMenuTrigger.displayName = "DropdownMenuTrigger"

const DropdownMenuSub = DropdownMenuPrimitive.Sub
const DropdownMenuGroup = DropdownMenuPrimitive.Group

interface DropdownMenuContentProps
  extends React.ComponentProps<typeof DropdownMenuPrimitive.Content> {
  viewportClassName?: string
}

function DropdownMenuSurface({
  className,
  children,
  viewportClassName,
}: {
  className?: string
  children: React.ReactNode
  viewportClassName?: string
}) {
  return (
    <div
      style={{
        transformOrigin: "var(--radix-dropdown-menu-content-transform-origin)",
      }}
      className={cn(
        "floating-menu-enter-y overflow-hidden rounded-[6px] border bg-popover text-popover-foreground shadow-lg transform-gpu",
        className,
      )}
    >
      <ScrollArea
        className="w-full max-h-[min(22rem,var(--radix-dropdown-menu-content-available-height))]"
        viewportClassName={cn(
          "h-auto max-h-[min(22rem,var(--radix-dropdown-menu-content-available-height))] overscroll-contain",
          viewportClassName,
        )}
      >
        {children}
      </ScrollArea>
    </div>
  )
}

function DropdownMenuContent({
  className,
  children,
  sideOffset = 4,
  viewportClassName,
  ...props
}: DropdownMenuContentProps) {
  return (
    <DropdownMenuPrimitive.Portal>
      <DropdownMenuPrimitive.Content
        sideOffset={sideOffset}
        className="z-[70] min-w-36"
        {...props}
      >
        <DropdownMenuSurface
          className={className}
          viewportClassName={viewportClassName}
        >
          {children}
        </DropdownMenuSurface>
      </DropdownMenuPrimitive.Content>
    </DropdownMenuPrimitive.Portal>
  )
}

interface DropdownMenuSubContentProps
  extends React.ComponentProps<typeof DropdownMenuPrimitive.SubContent> {
  viewportClassName?: string
}

function DropdownMenuSubContent({
  className,
  children,
  sideOffset = 4,
  viewportClassName,
  ...props
}: DropdownMenuSubContentProps) {
  return (
    <DropdownMenuPrimitive.Portal>
      <DropdownMenuPrimitive.SubContent
        sideOffset={sideOffset}
        className="z-[80] min-w-36"
        {...props}
      >
        <DropdownMenuSurface
          className={className}
          viewportClassName={viewportClassName}
        >
          {children}
        </DropdownMenuSurface>
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

function DropdownMenuLabel({
  className,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Label>) {
  return (
    <DropdownMenuPrimitive.Label
      className={cn(
        "flex h-7 select-none items-center bg-muted/55 px-3 text-2xs font-semibold text-muted-foreground",
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
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTriggerItem,
  DropdownMenuTrigger,
}
