"use client"

import * as React from "react"
import { ScrollArea as ScrollAreaPrimitive } from "radix-ui"

import { cn } from "@/lib/utils"

interface ScrollAreaProps
  extends React.ComponentProps<typeof ScrollAreaPrimitive.Root> {
  axis?: "vertical" | "horizontal" | "both"
  viewportClassName?: string
  viewportRef?: React.Ref<HTMLDivElement>
}

const scrollbarThumbClassName =
  "relative flex-1 rounded-full bg-[var(--scrollbar-thumb)] transition-colors duration-150 hover:bg-[var(--scrollbar-thumb-hover)] data-[orientation=horizontal]:min-w-[var(--scrollbar-min-thumb-size)] data-[orientation=vertical]:min-h-[var(--scrollbar-min-thumb-size)]"

function canScrollInDirection(position: number, maximum: number, delta: number): boolean {
  if (delta < 0) return position > 0
  if (delta > 0) return position < maximum
  return false
}

function stopWheelPropagationWhenScrollable(
  event: React.WheelEvent<HTMLElement>,
  axis: "vertical" | "horizontal" | "both" = "vertical",
): void {
  const viewport = event.currentTarget
  const horizontalDelta = event.deltaX || (event.shiftKey ? event.deltaY : 0)
  const verticalDelta = event.shiftKey ? 0 : event.deltaY
  const canScrollHorizontally =
    axis !== "vertical" &&
    canScrollInDirection(
      viewport.scrollLeft,
      viewport.scrollWidth - viewport.clientWidth,
      horizontalDelta,
    )
  const canScrollVertically =
    axis !== "horizontal" &&
    canScrollInDirection(
      viewport.scrollTop,
      viewport.scrollHeight - viewport.clientHeight,
      verticalDelta,
    )

  if (canScrollHorizontally || canScrollVertically) {
    event.stopPropagation()
  }
}

function ScrollArea({
  axis = "vertical",
  className,
  children,
  type = "auto",
  viewportClassName,
  viewportRef,
  ...props
}: ScrollAreaProps) {
  return (
    <ScrollAreaPrimitive.Root
      data-slot="scroll-area"
      type={type}
      className={cn("relative overflow-hidden", className)}
      {...props}
    >
      <ScrollAreaPrimitive.Viewport
        data-slot="scroll-area-viewport"
        ref={viewportRef}
        onWheel={(event) => stopWheelPropagationWhenScrollable(event, axis)}
        className={cn(
          "size-full rounded-[inherit] transition-[color,box-shadow] outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:outline-1",
          viewportClassName,
        )}
      >
        {children}
      </ScrollAreaPrimitive.Viewport>
      {axis !== "horizontal" && <ScrollBar orientation="vertical" />}
      {axis !== "vertical" && <ScrollBar orientation="horizontal" />}
      <ScrollAreaPrimitive.Corner />
    </ScrollAreaPrimitive.Root>
  )
}

function ScrollBar({
  className,
  orientation = "vertical",
  ...props
}: React.ComponentProps<typeof ScrollAreaPrimitive.ScrollAreaScrollbar>) {
  return (
    <ScrollAreaPrimitive.ScrollAreaScrollbar
      data-slot="scroll-area-scrollbar"
      data-orientation={orientation}
      orientation={orientation}
      className={cn(
        "z-10 flex touch-none bg-transparent transition-colors select-none data-[orientation=horizontal]:h-[var(--scrollbar-size)] data-[orientation=horizontal]:flex-col data-[orientation=vertical]:h-full data-[orientation=vertical]:w-[var(--scrollbar-size)]",
        className
      )}
      {...props}
    >
      <ScrollAreaPrimitive.ScrollAreaThumb
        data-slot="scroll-area-thumb"
        className={scrollbarThumbClassName}
      />
    </ScrollAreaPrimitive.ScrollAreaScrollbar>
  )
}

export {
  ScrollArea,
  ScrollBar,
  scrollbarThumbClassName,
  stopWheelPropagationWhenScrollable,
}
