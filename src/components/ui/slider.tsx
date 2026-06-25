import * as React from "react"
import { Slider as SliderPrimitive } from "radix-ui"
import { motion } from "motion/react"

import { cn } from "@/lib/utils"

function Slider({
  className,
  disabled,
  ...props
}: React.ComponentProps<typeof SliderPrimitive.Root>) {
  return (
    <SliderPrimitive.Root
      data-slot="slider"
      className={cn(
        "group/slider relative flex h-5 w-full touch-none items-center select-none data-disabled:cursor-not-allowed",
        className,
      )}
      disabled={disabled}
      {...props}
    >
      <SliderPrimitive.Track className="relative h-1 w-full grow overflow-hidden rounded-full bg-input">
        <SliderPrimitive.Range className="absolute h-full bg-primary group-data-disabled/slider:bg-muted-foreground" />
      </SliderPrimitive.Track>
      <SliderPrimitive.Thumb asChild>
        <motion.span
          whileHover={disabled ? undefined : { scale: 1.08 }}
          whileTap={disabled ? undefined : { scale: 0.95 }}
          transition={{ duration: 0.12, ease: [0.03, 0.59, 0.19, 1] }}
          className="block size-4 rounded-full border-2 border-background bg-primary shadow-sm ring-1 ring-primary/40 outline-none focus-visible:ring-3 focus-visible:ring-ring/50 group-data-disabled/slider:bg-muted-foreground group-data-disabled/slider:shadow-none group-data-disabled/slider:ring-muted-foreground/30"
        />
      </SliderPrimitive.Thumb>
    </SliderPrimitive.Root>
  )
}

export { Slider }
