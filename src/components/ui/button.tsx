import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { Slot } from "radix-ui"

import { cn } from "@/lib/utils"

const standardButtonState = cn(
  "border-[var(--button-standard-border)] bg-[var(--button-standard-bg)] text-foreground",
  "hover:border-[var(--button-standard-hover-border)] hover:bg-[var(--button-standard-hover-bg)] hover:text-foreground",
  "active:border-[var(--button-standard-pressed-border)] active:bg-[var(--button-standard-pressed-bg)]",
  "aria-expanded:border-[var(--button-standard-hover-border)] aria-expanded:bg-[var(--button-standard-hover-bg)] aria-expanded:text-foreground"
)

const subtleButtonState = cn(
  "border-transparent bg-transparent",
  "hover:border-transparent hover:bg-[var(--button-ghost-hover-bg)] hover:text-foreground",
  "active:border-transparent active:bg-[var(--button-ghost-pressed-bg)] active:text-foreground",
  "aria-expanded:border-transparent aria-expanded:bg-[var(--button-ghost-hover-bg)] aria-expanded:text-foreground"
)

const accentButtonState = cn(
  "border-[var(--button-accent-border)] bg-[var(--button-accent-bg)] text-primary-foreground",
  "hover:border-[var(--button-accent-hover-border)] hover:bg-[var(--button-accent-hover-bg)] hover:text-primary-foreground",
  "active:border-[var(--button-accent-pressed-border)] active:bg-[var(--button-accent-pressed-bg)]",
  "aria-expanded:border-[var(--button-accent-hover-border)] aria-expanded:bg-[var(--button-accent-hover-bg)] aria-expanded:text-primary-foreground"
)

const buttonVariants = cva(
  "group/button inline-flex shrink-0 items-center justify-center rounded-[6px] border border-transparent bg-clip-padding text-sm font-medium whitespace-nowrap transition-[background-color,color,box-shadow,border-color] duration-[80ms] ease-out outline-none select-none active:duration-[60ms] focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 dark:aria-invalid:border-destructive/50 dark:aria-invalid:ring-destructive/40 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default: accentButtonState,
        accent: accentButtonState,
        outline: standardButtonState,
        secondary: standardButtonState,
        ghost: subtleButtonState,
        destructive:
          cn(
            "border-destructive/30 bg-destructive/10 text-destructive hover:border-destructive/40 hover:bg-destructive/20 active:border-destructive/35 active:bg-destructive/25",
            "focus-visible:border-destructive/40 focus-visible:ring-destructive/20 dark:bg-destructive/20 dark:hover:bg-destructive/30 dark:active:bg-destructive/25 dark:focus-visible:ring-destructive/40"
          ),
        link: "text-primary underline-offset-4 hover:underline",
      },
      size: {
        default:
          "h-8 gap-1.5 px-2.5 has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2",
        xs: "h-6 gap-1 rounded-[6px] px-2 text-xs in-data-[slot=button-group]:rounded-[6px] has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&_svg:not([class*='size-'])]:size-3",
        sm: "h-7 gap-1 rounded-[6px] px-2.5 text-control in-data-[slot=button-group]:rounded-[6px] has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&_svg:not([class*='size-'])]:size-3.5",
        "control-sm":
          "h-8 gap-1 rounded-[6px] px-2.5 text-control in-data-[slot=button-group]:rounded-[6px] has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&_svg:not([class*='size-'])]:size-3.5",
        lg: "h-9 gap-1.5 px-2.5 has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2",
        icon: "size-8",
        "icon-xs":
          "size-6 rounded-[6px] in-data-[slot=button-group]:rounded-[6px] [&_svg:not([class*='size-'])]:size-3",
        "icon-sm":
          "size-7 rounded-[6px] in-data-[slot=button-group]:rounded-[6px]",
        "icon-lg": "size-9",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function Button({
  className,
  variant = "default",
  size = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean
  }) {
  if (asChild) {
    return (
      <Slot.Root
        data-slot="button"
        data-variant={variant}
        data-size={size}
        className={cn(buttonVariants({ variant, size, className }))}
        {...props}
      />
    )
  }

  return (
    <button
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  )
}

export { Button, buttonVariants }
