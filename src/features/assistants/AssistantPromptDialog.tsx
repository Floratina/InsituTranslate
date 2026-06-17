import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Textarea } from "@/components/ui/textarea";

interface AssistantPromptDialogProps {
  open: boolean;
  value: string;
  saving: boolean;
  onOpenChange: (open: boolean) => void;
  onSave: (value: string) => Promise<boolean>;
}

export function AssistantPromptDialog({
  open,
  value,
  saving,
  onOpenChange,
  onSave,
}: AssistantPromptDialogProps) {
  const [draft, setDraft] = useState(value);

  useEffect(() => {
    if (open) setDraft(value);
  }, [open, value]);

  async function save(nextValue = draft): Promise<void> {
    if (await onSave(nextValue)) {
      onOpenChange(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent open={open}>
        <DialogHeader>
          <DialogTitle>系统提示词</DialogTitle>
        </DialogHeader>
        <Textarea
          className="min-h-40 resize-y"
          value={draft}
          placeholder="填写该助手使用的系统提示词"
          onChange={(event) => setDraft(event.target.value)}
        />
        <DialogFooter className="justify-between">
          <Button
            variant="destructive"
            disabled={saving || (!draft && !value)}
            onClick={() => void save("")}
          >
            清除全部
          </Button>
          <Button disabled={saving} onClick={() => void save()}>
            保存提示词
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
