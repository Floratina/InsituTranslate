import { useEffect, useRef, useState } from "react";
import { Copy, GripVertical, Pencil, Trash2 } from "lucide-react";
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
import { cn } from "@/lib/utils";

import { PURPOSES } from "./constants";
import { isMinerUProvider } from "./mineru";
import { ProviderAvatar } from "./ProviderAvatar";
import type { ProviderPurpose, ProviderView } from "./types";

interface ProviderListItemProps {
  provider: ProviderView;
  selected: boolean;
  onSelect: () => void;
  onEdit: () => void;
  onDelete: () => void;
  onCopy: (purpose: ProviderPurpose) => void;
  onDragComplete: () => void;
}

const itemTransition = { type: "spring" as const, stiffness: 300, damping: 30 };

export function ProviderListItem({
  provider,
  selected,
  onSelect,
  onEdit,
  onDelete,
  onCopy,
  onDragComplete,
}: ProviderListItemProps) {
  const [dragging, setDragging] = useState(false);
  const draggedAt = useRef(0);
  const persistTimer = useRef<number | null>(null);
  const copyTargets = isMinerUProvider(provider)
    ? PURPOSES.filter((item) => item.value === "document-parsing")
    : PURPOSES;
  const showCopyMenu = copyTargets.length > 0;
  const showDeleteAction = !provider.isBuiltin;

  useEffect(() => {
    return () => {
      if (persistTimer.current !== null) {
        window.clearTimeout(persistTimer.current);
      }
    };
  }, []);

  useEffect(() => {
    if (!dragging) return;
    function finishWindowPointerInteraction(): void {
      finishDragVisuals();
      schedulePersistence();
    }
    window.addEventListener("pointerup", finishWindowPointerInteraction);
    window.addEventListener("pointercancel", finishWindowPointerInteraction);
    return () => {
      window.removeEventListener("pointerup", finishWindowPointerInteraction);
      window.removeEventListener("pointercancel", finishWindowPointerInteraction);
    };
  }, [dragging]);

  function finishDragVisuals(): void {
    draggedAt.current = Date.now();
    window.requestAnimationFrame(() => setDragging(false));
  }

  function schedulePersistence(): void {
    if (persistTimer.current !== null) {
      window.clearTimeout(persistTimer.current);
    }
    persistTimer.current = window.setTimeout(onDragComplete, 320);
  }

  function finishPointerInteraction(): void {
    if (!dragging) return;
    finishDragVisuals();
    schedulePersistence();
  }

  return (
    <Reorder.Item
      as="div"
      value={provider.id}
      dragElastic={0.04}
      dragMomentum={false}
      transition={itemTransition}
      onDragStart={() => setDragging(true)}
      onDragEnd={() => {
        finishDragVisuals();
        schedulePersistence();
      }}
      onPointerUp={finishPointerInteraction}
      onPointerCancel={finishPointerInteraction}
      onLostPointerCapture={finishPointerInteraction}
      className={cn(
        "relative rounded-[6px]",
        dragging && "z-20 shadow-[0_3px_8px_rgba(15,23,42,0.10)]",
      )}
    >
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <div
            onClick={() => {
              if (!dragging && Date.now() - draggedAt.current > 180) onSelect();
            }}
            className={cn(
              "relative flex cursor-grab items-center gap-2 rounded-[6px] p-2 text-left hover:bg-accent/70 active:cursor-grabbing",
              selected && "bg-accent",
            )}
          >
            <ProviderAvatar name={provider.name} avatar={provider.avatar} className="size-8 text-3xs" />
            <div className="min-w-0 flex-1">
              <div className="truncate text-sm font-medium">{provider.name}</div>
              <div className="text-2xs text-muted-foreground">{provider.models.length} 个模型</div>
            </div>
            {provider.enabled && (
              <span className="inline-flex h-5 shrink-0 self-center items-center rounded-[6px] bg-enabled-accent/15 px-1.5 text-3xs leading-none font-medium text-enabled-accent">
                已启用
              </span>
            )}
            <GripVertical className="size-3.5 text-muted-foreground/50" />
          </div>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem onSelect={onEdit}>
            <Pencil className="size-3.5" />
            编辑提供商
          </ContextMenuItem>
          {(showCopyMenu || showDeleteAction) && <ContextMenuSeparator />}
          {showCopyMenu && (
            <ContextMenuSub>
              <ContextMenuSubTriggerItem>
                <Copy className="size-3.5" />
                复制到
              </ContextMenuSubTriggerItem>
              <ContextMenuSubContent>
                {copyTargets.map((item) => {
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
          )}
          {showCopyMenu && showDeleteAction && <ContextMenuSeparator />}
          {showDeleteAction && (
            <>
              <ContextMenuItem
                className="text-destructive focus:bg-destructive/10 focus:text-destructive"
                onSelect={onDelete}
              >
                <Trash2 className="size-3.5" />
                删除提供商
              </ContextMenuItem>
            </>
          )}
        </ContextMenuContent>
      </ContextMenu>
    </Reorder.Item>
  );
}
