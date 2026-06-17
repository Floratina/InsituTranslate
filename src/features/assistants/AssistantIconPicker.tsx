import { useState } from "react";
import { Check } from "lucide-react";

import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";

import { ASSISTANT_EMOJIS, ASSISTANT_LUCIDE_ICONS } from "./constants";
import { AssistantIcon } from "./AssistantIcon";
import type { AssistantIconKind } from "./types";

interface AssistantIconPickerProps {
  kind: AssistantIconKind;
  value: string;
  onChange: (kind: AssistantIconKind, value: string) => void;
}

export function AssistantIconPicker({
  kind,
  value,
  onChange,
}: AssistantIconPickerProps) {
  const [open, setOpen] = useState(false);

  function choose(nextKind: AssistantIconKind, nextValue: string): void {
    onChange(nextKind, nextValue);
    setOpen(false);
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label="选择助手图标"
          className="rounded-[6px] outline-none transition-colors duration-150 hover:bg-accent/45 focus-visible:ring-3 focus-visible:ring-ring/40"
        >
          <AssistantIcon kind={kind} value={value} className="size-8 text-sm" />
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-72 p-2">
        <Tabs defaultValue={kind} className="gap-2">
          <TabsList className="w-full">
            <TabsTrigger value="emoji">Emoji</TabsTrigger>
            <TabsTrigger value="lucide">Lucide</TabsTrigger>
          </TabsList>
          <TabsContent value="emoji">
            <div className="grid grid-cols-6 gap-1">
              {ASSISTANT_EMOJIS.map((emoji) => (
                <button
                  key={emoji}
                  type="button"
                  className={cn(
                    "relative flex size-8 items-center justify-center rounded-[6px] border font-emoji text-base transition-colors duration-150 hover:bg-accent",
                    kind === "emoji" && value === emoji && "border-primary bg-accent",
                  )}
                  onClick={() => choose("emoji", emoji)}
                >
                  {emoji}
                  {kind === "emoji" && value === emoji && (
                    <Check className="absolute right-0.5 bottom-0.5 size-2.5 text-primary" />
                  )}
                </button>
              ))}
            </div>
          </TabsContent>
          <TabsContent value="lucide">
            <div className="grid grid-cols-6 gap-1">
              {ASSISTANT_LUCIDE_ICONS.map((icon) => (
                <button
                  key={icon}
                  type="button"
                  title={icon}
                  className={cn(
                    "relative flex size-8 items-center justify-center rounded-[6px] border transition-colors duration-150 hover:bg-accent",
                    kind === "lucide" && value === icon && "border-primary bg-accent",
                  )}
                  onClick={() => choose("lucide", icon)}
                >
                  <AssistantIcon
                    kind="lucide"
                    value={icon}
                    className="size-6 border-0 bg-transparent"
                  />
                  {kind === "lucide" && value === icon && (
                    <Check className="absolute right-0.5 bottom-0.5 size-2.5 text-primary" />
                  )}
                </button>
              ))}
            </div>
          </TabsContent>
        </Tabs>
      </PopoverContent>
    </Popover>
  );
}
