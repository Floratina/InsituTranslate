import * as React from "react"
import { motion, type HTMLMotionProps } from "motion/react"

import { cn } from "@/lib/utils"

function Textarea({ className, ...props }: React.ComponentProps<"textarea">) {
  return (
    <motion.textarea
      data-slot="textarea"
      className={cn(
        "scrollbar-subtle flex field-sizing-content min-h-16 w-full overflow-y-auto rounded-[6px] border border-input bg-transparent px-2.5 py-2 text-base outline-none overscroll-contain placeholder:font-placeholder-mono placeholder:font-medium placeholder:tracking-normal placeholder:text-muted-foreground disabled:cursor-not-allowed disabled:bg-input/50 disabled:opacity-50 aria-invalid:border-destructive md:text-sm dark:bg-input/30 dark:disabled:bg-input/80",
        className
      )}
      whileHover={{ borderColor: "var(--ring)" }}
      whileFocus={{
        borderColor: "var(--ring)",
        boxShadow: "0 0 0 3px color-mix(in oklch, var(--ring) 22%, transparent)",
      }}
      transition={{ duration: 0.2, ease: [0.03, 0.59, 0.19, 1] }}
      {...(props as HTMLMotionProps<"textarea">)}
    />
  )
}

export { Textarea }
