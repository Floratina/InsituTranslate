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

interface SelectScrollbarMetrics {
  visible: boolean
  thumbHeight: number
  thumbTop: number
}

function SelectViewportScrollbar({
  viewportRef,
}: {
  viewportRef: React.RefObject<HTMLDivElement | null>
}) {
  const [metrics, setMetrics] = React.useState<SelectScrollbarMetrics>({
    visible: false,
    thumbHeight: 0,
    thumbTop: 0,
  })
  const dragStateRef = React.useRef<{
    maxScrollTop: number
    maxThumbTop: number
    startScrollTop: number
    startY: number
  } | null>(null)

  const updateMetrics = React.useCallback(() => {
    const viewport = viewportRef.current
    if (!viewport) return

    const { clientHeight, scrollHeight, scrollTop } = viewport
    const visible = scrollHeight > clientHeight + 1
    if (!visible) {
      setMetrics((current) =>
        current.visible ? { visible: false, thumbHeight: 0, thumbTop: 0 } : current,
      )
      return
    }

    const thumbHeight = Math.max(20, (clientHeight / scrollHeight) * clientHeight)
    const maxThumbTop = clientHeight - thumbHeight
    const maxScrollTop = scrollHeight - clientHeight
    const thumbTop = maxScrollTop > 0 ? (scrollTop / maxScrollTop) * maxThumbTop : 0
    setMetrics((current) => {
      const next = { visible, thumbHeight, thumbTop }
      return current.visible === next.visible &&
        Math.abs(current.thumbHeight - next.thumbHeight) < 0.5 &&
        Math.abs(current.thumbTop - next.thumbTop) < 0.5
        ? current
        : next
    })
  }, [viewportRef])

  React.useLayoutEffect(() => {
    const viewport = viewportRef.current
    if (!viewport) return

    updateMetrics()
    viewport.addEventListener("scroll", updateMetrics, { passive: true })
    window.addEventListener("resize", updateMetrics)

    const resizeObserver = new ResizeObserver(updateMetrics)
    resizeObserver.observe(viewport)
    for (const child of Array.from(viewport.children)) {
      resizeObserver.observe(child)
    }

    return () => {
      viewport.removeEventListener("scroll", updateMetrics)
      window.removeEventListener("resize", updateMetrics)
      resizeObserver.disconnect()
    }
  }, [updateMetrics, viewportRef])

  if (!metrics.visible) return null

  function handlePointerDown(event: React.PointerEvent<HTMLDivElement>): void {
    const viewport = viewportRef.current
    if (!viewport) return

    event.preventDefault()
    event.currentTarget.setPointerCapture(event.pointerId)
    dragStateRef.current = {
      maxScrollTop: viewport.scrollHeight - viewport.clientHeight,
      maxThumbTop: viewport.clientHeight - metrics.thumbHeight,
      startScrollTop: viewport.scrollTop,
      startY: event.clientY,
    }
  }

  function handlePointerMove(event: React.PointerEvent<HTMLDivElement>): void {
    const viewport = viewportRef.current
    const dragState = dragStateRef.current
    if (!viewport || !dragState || dragState.maxThumbTop <= 0) return

    const deltaY = event.clientY - dragState.startY
    viewport.scrollTop =
      dragState.startScrollTop +
      (deltaY / dragState.maxThumbTop) * dragState.maxScrollTop
  }

  function handlePointerUp(event: React.PointerEvent<HTMLDivElement>): void {
    dragStateRef.current = null
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId)
    }
  }

  return (
    <div className="pointer-events-none absolute top-0 right-0 bottom-0 z-10 w-1.5">
      <div
        className="pointer-events-auto absolute right-0 w-1.5 rounded-full bg-muted-foreground/45 transition-colors duration-150 hover:bg-ring/70"
        style={{
          height: metrics.thumbHeight,
          transform: `translateY(${metrics.thumbTop}px)`,
        }}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerCancel={handlePointerUp}
      />
    </div>
  )
}

function SelectContent({
  className,
  children,
  viewportClassName,
  ...props
}: SelectContentProps) {
  const viewportRef = React.useRef<HTMLDivElement | null>(null)

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
            "floating-menu-enter relative overflow-hidden rounded-[6px] border bg-popover text-popover-foreground shadow-lg transform-gpu",
            className,
          )}
        >
          <SelectPrimitive.Viewport
            ref={viewportRef}
            className={cn(
              "scrollbar-hidden max-h-[min(22rem,var(--radix-select-content-available-height))] overflow-x-hidden overflow-y-auto overscroll-contain",
              viewportClassName,
            )}
          >
            {children}
          </SelectPrimitive.Viewport>
          <SelectViewportScrollbar viewportRef={viewportRef} />
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
