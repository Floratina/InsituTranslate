import * as React from "react"
import { motion, type HTMLMotionProps } from "motion/react"

import { cn } from "@/lib/utils"

function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <motion.input
      type={type}
      data-slot="input"
      className={cn(
        "h-8 w-full min-w-0 rounded-[6px] border border-input bg-transparent px-2.5 py-1 text-base outline-none file:inline-flex file:h-6 file:border-0 file:bg-transparent file:text-sm file:font-medium file:text-foreground placeholder:font-placeholder-mono placeholder:font-medium placeholder:tracking-normal placeholder:text-muted-foreground disabled:pointer-events-none disabled:cursor-not-allowed disabled:bg-input/50 disabled:opacity-50 aria-invalid:border-destructive md:text-sm dark:bg-input/30 dark:disabled:bg-input/80",
        className
      )}
      whileHover={{ borderColor: "var(--ring)" }}
      whileFocus={{
        borderColor: "var(--ring)",
        boxShadow: "0 0 0 3px color-mix(in oklch, var(--ring) 22%, transparent)",
      }}
      transition={{ duration: 0.2, ease: [0.03, 0.59, 0.19, 1] }}
      {...(props as HTMLMotionProps<"input">)}
    />
  )
}

export { Input }
