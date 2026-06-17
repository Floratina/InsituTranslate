import { LoaderCircle } from "lucide-react";
import { motion } from "motion/react";

import { cn } from "@/lib/utils";

interface LoadingStateProps {
  label?: string;
  className?: string;
  compact?: boolean;
}

const dotTransition = {
  duration: 0.28,
  repeat: Infinity,
  repeatType: "reverse" as const,
  ease: [0.03, 0.59, 0.19, 1] as const,
};

function LoadingState({
  label = "加载中",
  className,
  compact = false,
}: LoadingStateProps) {
  return (
    <div
      role="status"
      aria-live="polite"
      className={cn(
        "flex items-center justify-center gap-2 rounded-[6px] text-xs text-muted-foreground",
        compact ? "px-2 py-2" : "px-3 py-6",
        className,
      )}
    >
      <motion.span
        aria-hidden="true"
        className="flex size-3.5 items-center justify-center"
        animate={{ rotate: 360 }}
        transition={{ duration: 0.28, repeat: Infinity, ease: [0.03, 0.59, 0.19, 1] }}
      >
        <LoaderCircle className="size-3.5" strokeWidth={1.8} />
      </motion.span>
      <span>{label}</span>
      <span aria-hidden="true" className="flex items-center gap-0.5">
        {[0, 1, 2].map((index) => (
          <motion.span
            key={index}
            className="size-1 rounded-full bg-current"
            initial={{ opacity: 0.35, scale: 0.85 }}
            animate={{ opacity: 1, scale: 1 }}
            transition={{ ...dotTransition, delay: index * 0.12 }}
          />
        ))}
      </span>
    </div>
  );
}

export { LoadingState };
