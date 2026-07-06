import { Loader2 } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";

import { Progress } from "@/components/ui/progress";
import { APP_MOTION_EASE } from "@/lib/motion";
import { cn } from "@/lib/utils";

import {
  formatPercent,
  formatTokenK,
  taskStatusMessage,
} from "./format";
import type {
  TranslationTaskStatus,
  TranslationTaskView,
} from "./types";

type TaskStatsLineMode = "normal" | "retry" | "failed" | "interrupted";
type TaskStatsLineSeverity = "muted" | "warning" | "danger";

interface TaskStatsLine {
  mode: TaskStatsLineMode;
  text: string;
  severity: TaskStatsLineSeverity;
}

interface TaskStatsCellProps {
  task: TranslationTaskView;
}

const lineTransition = {
  duration: 0.15,
  ease: APP_MOTION_EASE,
} as const;

function liveTaskStatus(status: TranslationTaskStatus): boolean {
  return status === "running" || status === "interrupted-pending";
}

function taskStatsLineClass(severity: TaskStatsLineSeverity): string {
  if (severity === "danger") return "text-[var(--task-status-danger)]";
  if (severity === "warning") return "text-[var(--task-status-warning)]";
  return "text-muted-foreground";
}

function taskStatsLine(task: TranslationTaskView): TaskStatsLine {
  const status = taskStatusMessage(task);

  if (task.status === "failed") {
    return {
      mode: "failed",
      text: `失败：${status.text || "任务失败，请检查任务详情"}`,
      severity: "danger",
    };
  }

  if (task.status === "interrupted" || task.status === "interrupted-pending") {
    return {
      mode: "interrupted",
      text: `中断：${status.text || "任务已中断，可继续"}`,
      severity: "warning",
    };
  }

  if (task.activeRetry) {
    return {
      mode: "retry",
      text: `重试中 (${task.activeRetry.current}/${task.activeRetry.max})：${task.activeRetry.message}`,
      severity: "muted",
    };
  }

  return {
    mode: "normal",
    text: `翻译进度 (${task.completedChunks}/${task.totalChunks}) · 失败 (${task.failedChunks})`,
    severity: "muted",
  };
}

export function TaskStatsCell({ task }: TaskStatsCellProps) {
  const line = taskStatsLine(task);

  return (
    <div className="grid min-w-0 gap-1.5">
      <div className="flex min-w-0 items-center gap-1.5 overflow-hidden">
        {liveTaskStatus(task.status) && (
          <Loader2 className="size-3 shrink-0 animate-spin text-muted-foreground" />
        )}
        <div className="relative h-4 min-w-0 flex-1 overflow-hidden">
          <AnimatePresence initial={false} mode="wait">
            <motion.div
              key={line.mode}
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={lineTransition}
              className={cn(
                "absolute inset-x-0 top-0 truncate text-2xs leading-4",
                taskStatsLineClass(line.severity),
              )}
              title={line.text}
            >
              {line.text}
            </motion.div>
          </AnimatePresence>
        </div>
      </div>
      <div className="flex min-w-0 items-center gap-2">
        <Progress
          value={Math.round(Math.max(0, Math.min(1, task.progress)) * 100)}
          className="h-1.5 min-w-16 flex-1"
        />
        <span className="shrink-0 text-2xs text-muted-foreground">
          {formatTokenK(task.tokenStats.totalTokens)} · {formatPercent(task.progress)}
        </span>
      </div>
    </div>
  );
}
