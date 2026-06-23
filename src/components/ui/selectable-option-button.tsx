import * as React from "react";
import { Check } from "lucide-react";
import { motion } from "motion/react";

import { cn } from "@/lib/utils";

type SelectableOptionButtonProps = Omit<
  React.ComponentProps<typeof motion.button>,
  "children"
> & {
  label: React.ReactNode;
  description?: React.ReactNode;
  selected?: boolean;
};

function SelectableOptionButton({
  label,
  description,
  selected = false,
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
      whileTap={whileTap ?? { scale: 0.99 }}
      transition={transition ?? { duration: 0.12, ease: [0.03, 0.59, 0.19, 1] }}
      disabled={disabled}
      className={cn(
        "grid min-h-14 grid-cols-[1fr_auto] items-start gap-3 rounded-[6px] border bg-background/60 px-3 py-2 text-left outline-none transition-colors duration-150 hover:bg-accent/40 focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 disabled:pointer-events-none disabled:opacity-60",
        selected && "border-primary/45 bg-accent text-accent-foreground hover:bg-accent/90",
        className,
      )}
      {...props}
    >
      <span className="min-w-0">
        <span className="block text-sm font-medium">{label}</span>
        {description && (
          <span
            className={cn(
              "mt-0.5 block text-2xs leading-snug",
              selected ? "text-accent-foreground/75" : "text-muted-foreground",
            )}
          >
            {description}
          </span>
        )}
      </span>
      {selected && <Check className="mt-0.5 size-4 shrink-0 text-primary" />}
    </motion.button>
  );
}

export { SelectableOptionButton };
