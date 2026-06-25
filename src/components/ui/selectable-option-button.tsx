import * as React from "react";
import { Check } from "lucide-react";
import { motion } from "motion/react";

import { APP_MOTION_EASE } from "@/lib/motion";
import { cn } from "@/lib/utils";

type SelectableOptionButtonProps = Omit<
  React.ComponentProps<typeof motion.button>,
  "children"
> & {
  label: React.ReactNode;
  description?: React.ReactNode;
  leading?: React.ReactNode;
  selected?: boolean;
  indicatorVariant?: "radio" | "checkbox";
};

const SELECTABLE_OPTION_PRESS_SCALE = {
  scaleX: 0.985,
  scaleY: 0.982,
} as const;

const SELECTABLE_OPTION_PRESS_TRANSITION = {
  duration: 0.12,
  ease: APP_MOTION_EASE,
} as const;

function SelectableOptionButton({
  label,
  description,
  leading,
  selected = false,
  indicatorVariant = "radio",
  className,
  disabled,
  whileTap,
  transition,
  ...props
}: SelectableOptionButtonProps) {
  return (
    <motion.button
      type="button"
      aria-pressed={selected}
      whileTap={whileTap ?? SELECTABLE_OPTION_PRESS_SCALE}
      transition={transition ?? SELECTABLE_OPTION_PRESS_TRANSITION}
      disabled={disabled}
      className={cn(
        "relative flex min-h-14 w-full min-w-0 origin-center transform-gpu items-center gap-3 rounded-[6px] border bg-background px-3 py-2 text-left outline-none transition-[background-color,border-color,box-shadow] duration-150 hover:bg-muted/60 focus-visible:ring-3 focus-visible:ring-ring/40 disabled:pointer-events-none disabled:opacity-60",
        selected && "border-primary bg-background ring-1 ring-primary/35",
        className,
      )}
      {...props}
    >
      {leading && (
        <span className="shrink-0">
          {leading}
        </span>
      )}
      <span className="min-w-0 flex-1">
        <span className="block text-sm font-medium">{label}</span>
        {description && (
          <span
            className={cn(
              "mt-0.5 block text-xs leading-snug text-muted-foreground",
            )}
          >
            {description}
          </span>
        )}
      </span>
      <span
        className={cn(
          "ml-auto flex size-5 shrink-0 items-center justify-center border transition-[background-color,border-color,color,opacity] duration-150",
          indicatorVariant === "radio" ? "rounded-full" : "rounded-[6px]",
          selected
            ? "border-primary bg-primary text-primary-foreground"
            : "border-muted-foreground/35 bg-transparent text-transparent opacity-60",
        )}
        aria-hidden="true"
      >
        <Check className="size-3" />
      </span>
    </motion.button>
  );
}

export { SelectableOptionButton };
