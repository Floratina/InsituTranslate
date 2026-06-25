import * as React from "react"
import { Progress as ProgressPrimitive } from "radix-ui"
import { motion } from "motion/react"

import { CONTROLLED_STATE_MOTION_PROPS } from "@/lib/motion"
import { cn } from "@/lib/utils"

function Progress({
  className,
  value,
  ...props
}: React.ComponentProps<typeof ProgressPrimitive.Root>) {
  return (
    <ProgressPrimitive.Root
      data-slot="progress"
      className={cn(
        "relative flex h-1 w-full items-center overflow-x-hidden rounded-full bg-muted",
        className
      )}
      {...props}
    >
      <ProgressPrimitive.Indicator
        data-slot="progress-indicator"
        asChild
      >
        <motion.div
          {...CONTROLLED_STATE_MOTION_PROPS}
          className="size-full flex-1 origin-left bg-primary"
          animate={{ scaleX: Math.max(0, Math.min(100, value ?? 0)) / 100 }}
        />
      </ProgressPrimitive.Indicator>
    </ProgressPrimitive.Root>
  )
}

export { Progress }
