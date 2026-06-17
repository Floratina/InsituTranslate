import { useEffect, useRef, useState } from "react";
import { Copy, GripVertical, Trash2 } from "lucide-react";
import { Reorder } from "motion/react";

import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTriggerItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { PURPOSES } from "@/features/providers/constants";
import type { ProviderPurpose } from "@/features/providers/types";
import { cn } from "@/lib/utils";

import { AssistantIcon } from "./AssistantIcon";
import type { AssistantView } from "./types";

interface AssistantListItemProps {
  assistant: AssistantView;
  selected: boolean;
  onSelect: () => void;
  onDelete: () => void;
  onCopy: (purpose: ProviderPurpose) => void;
  onDragComplete: () => void;
}

const itemTransition = { type: "spring" as const, stiffness: 300, damping: 30 };

export function AssistantListItem({
  assistant,
  selected,
  onSelect,
  onDelete,
  onCopy,
  onDragComplete,
}: AssistantListItemProps) {
  const [dragging, setDragging] = useState(false);
  const draggedAt = useRef(0);
  const persistTimer = useRef<number | null>(null);

  useEffect(() => {
    return () => {
      if (persistTimer.current !== null) window.clearTimeout(persistTimer.current);
    };
  }, []);

  function finishDrag(): void {
    draggedAt.current = Date.now();
    setDragging(false);
    if (persistTimer.current !== null) window.clearTimeout(persistTimer.current);
    persistTimer.current = window.setTimeout(onDragComplete, 320);
  }

  return (
    <Reorder.Item
      as="div"
      value={assistant.id}
      dragElastic={0.04}
      dragMomentum={false}
      transition={itemTransition}
      onDragStart={() => setDragging(true)}
      onDragEnd={finishDrag}
      className={cn(
        "relative rounded-[6px]",
        dragging && "z-20 shadow-[0_3px_8px_rgba(15,23,42,0.10)]",
      )}
    >
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <button
            type="button"
            onClick={() => {
              if (!dragging && Date.now() - draggedAt.current > 180) onSelect();
            }}
            className={cn(
              "relative flex w-full cursor-grab items-center gap-2 rounded-[6px] p-2 text-left hover:bg-accent/70 active:cursor-grabbing",
              selected && "bg-accent",
            )}
          >
            <AssistantIcon
              kind={assistant.iconKind}
              value={assistant.iconValue}
              className="size-8 text-sm"
              glyphClassName="size-4 text-base"
            />
            <span className="min-w-0 flex-1">
              <span className="block truncate text-sm font-medium">
                {assistant.name}
              </span>
            </span>
            <GripVertical className="size-3.5 text-muted-foreground/50" />
          </button>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuSub>
            <ContextMenuSubTriggerItem>
              <Copy className="size-3.5" />
              复制到
            </ContextMenuSubTriggerItem>
            <ContextMenuSubContent>
              {PURPOSES.map((item) => {
                const PurposeIcon = item.icon;
                return (
                  <ContextMenuItem key={item.value} onSelect={() => onCopy(item.value)}>
                    <PurposeIcon className="size-3.5" />
                    {item.label}
                  </ContextMenuItem>
                );
              })}
            </ContextMenuSubContent>
          </ContextMenuSub>
          <ContextMenuSeparator />
          <ContextMenuItem
            className="text-destructive focus:bg-destructive/10 focus:text-destructive"
            onSelect={onDelete}
          >
            <Trash2 className="size-3.5" />
            删除助手
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
    </Reorder.Item>
  );
}
