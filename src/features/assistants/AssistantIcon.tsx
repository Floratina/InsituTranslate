import {
  Bot,
  BookOpen,
  Brain,
  Braces,
  FileCheck2,
  Languages,
  MessagesSquare,
  PenLine,
  ScanText,
  Search,
  Sparkles,
  WandSparkles,
  type LucideIcon,
} from "lucide-react";

import { cn } from "@/lib/utils";

import type { AssistantIconKind } from "./types";

const LUCIDE_ASSISTANT_ICONS: Record<string, LucideIcon> = {
  bot: Bot,
  "book-open": BookOpen,
  brain: Brain,
  braces: Braces,
  "file-check": FileCheck2,
  languages: Languages,
  "messages-square": MessagesSquare,
  "pen-line": PenLine,
  "scan-text": ScanText,
  search: Search,
  sparkles: Sparkles,
  "wand-sparkles": WandSparkles,
};

interface AssistantIconProps {
  kind: AssistantIconKind;
  value: string;
  className?: string;
  glyphClassName?: string;
}

export function AssistantIcon({
  kind,
  value,
  className,
  glyphClassName,
}: AssistantIconProps) {
  const LucideGlyph = kind === "lucide" ? LUCIDE_ASSISTANT_ICONS[value] ?? Bot : null;
  return (
    <span
      className={cn(
        "inline-flex shrink-0 items-center justify-center rounded-[6px] border bg-background text-sm",
        className,
      )}
    >
      {LucideGlyph ? (
        <LucideGlyph className={cn("size-4", glyphClassName)} strokeWidth={1.8} />
      ) : (
        <span
          className={cn(
            "flex h-full w-full items-center justify-center font-emoji text-base leading-none",
            glyphClassName,
          )}
        >
          {value || "🤖"}
        </span>
      )}
    </span>
  );
}

export { LUCIDE_ASSISTANT_ICONS };
