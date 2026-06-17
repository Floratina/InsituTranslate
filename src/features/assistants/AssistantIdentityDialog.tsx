import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogField,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

import { AssistantIconPicker } from "./AssistantIconPicker";
import type { AssistantSettingsDraft } from "./types";

interface AssistantIdentityDialogProps {
  open: boolean;
  settings: AssistantSettingsDraft;
  onOpenChange: (open: boolean) => void;
  onSave: (settings: AssistantSettingsDraft) => void;
}

export function AssistantIdentityDialog({
  open,
  settings,
  onOpenChange,
  onSave,
}: AssistantIdentityDialogProps) {
  const [draft, setDraft] = useState(settings);

  useEffect(() => {
    if (open) setDraft(settings);
  }, [open, settings]);

  function save(): void {
    const name = draft.name.trim();
    if (!name) return;
    onSave({ ...draft, name });
    onOpenChange(false);
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent open={open}>
        <DialogHeader>
          <DialogTitle>编辑助手</DialogTitle>
        </DialogHeader>
        <DialogField>
          <Label>名称</Label>
          <div className="flex items-center gap-2">
            <AssistantIconPicker
              kind={draft.iconKind}
              value={draft.iconValue}
              onChange={(iconKind, iconValue) =>
                setDraft((current) => ({ ...current, iconKind, iconValue }))
              }
            />
            <Input
              value={draft.name}
              autoFocus
              onChange={(event) =>
                setDraft((current) => ({
                  ...current,
                  name: event.target.value,
                }))
              }
              onKeyDown={(event) => {
                if (event.key === "Enter") save();
              }}
            />
          </div>
        </DialogField>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button disabled={!draft.name.trim()} onClick={save}>
            保存
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
