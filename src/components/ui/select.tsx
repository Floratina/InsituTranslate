import * as React from "react"
import { Check, ChevronDown } from "lucide-react"
import { Select as SelectPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"

interface SelectControlContextValue {
  disabled?: boolean
  open: boolean
  setOpen: (open: boolean) => void
}

const SelectControlContext = React.createContext<SelectControlContextValue | null>(null)

function Select({
  disabled,
  open: openProp,
  defaultOpen,
  onOpenChange,
  ...props
}: React.ComponentProps<typeof SelectPrimitive.Root>) {
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
    <SelectControlContext.Provider value={{ disabled, open, setOpen }}>
      <SelectPrimitive.Root
        disabled={disabled}
        open={open}
        onOpenChange={setOpen}
        {...props}
      />
    </SelectControlContext.Provider>
  )
}

const SelectValue = SelectPrimitive.Value
const selectTriggerClassName =
  "inline-flex h-8 w-full items-center gap-2 overflow-hidden rounded-[6px] border border-input bg-background px-2.5 text-sm whitespace-nowrap outline-none transition-[background-color,border-color,color] duration-[80ms] ease-out hover:border-ring hover:bg-[var(--button-standard-hover-bg)] active:border-[var(--button-standard-pressed-border)] active:bg-[var(--button-standard-pressed-bg)] active:duration-[60ms] focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 data-[state=open]:border-ring data-[state=open]:bg-[var(--button-standard-hover-bg)] disabled:pointer-events-none disabled:opacity-50"
const selectItemClassName =
  "relative flex h-8 w-full cursor-default select-none items-center overflow-hidden py-1 pr-8 pl-3 text-left text-sm whitespace-nowrap outline-none hover:bg-accent focus:bg-accent data-disabled:opacity-50"

function SelectTrigger({
  className,
  children,
  disabled,
  onClick,
  onPointerCancel,
  onPointerDown,
  onPointerUp,
  ...props
}: React.ComponentProps<typeof SelectPrimitive.Trigger>) {
  const control = React.useContext(SelectControlContext)
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
      control?.disabled ||
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

    if (
      event.defaultPrevented ||
      disabled ||
      control?.disabled ||
      control === null ||
      !pendingPointerRef.current
    ) {
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
    <SelectPrimitive.Trigger
      {...props}
      disabled={disabled}
      className={cn(
        selectTriggerClassName,
        className,
      )}
      onClick={handleClick}
      onPointerCancel={handlePointerCancel}
      onPointerDown={handlePointerDown}
      onPointerUp={handlePointerUp}
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
        <div
          style={{
            transformOrigin: "var(--radix-select-content-transform-origin)",
          }}
          className={cn(
            "floating-menu-enter overflow-hidden rounded-[6px] border bg-popover text-popover-foreground shadow-lg transform-gpu",
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
        </div>
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
