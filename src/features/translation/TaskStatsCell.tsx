import { useEffect, useRef } from "react";
import { Loader2 } from "lucide-react";

import { Progress } from "@/components/ui/progress";
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

type TaskStatsLineMode = "normal" | "retry" | "failed" | "interrupted" | "queued";
type TaskStatsLineSeverity = "muted" | "warning" | "danger";

interface TaskStatsLine {
  mode: TaskStatsLineMode;
  text: string;
  severity: TaskStatsLineSeverity;
}

interface TaskStatsCellProps {
  task: TranslationTaskView;
}

type TaskStatsLineMotion = "fade" | "float-up" | "float-down";

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

  if (task.status === "queued") {
    return {
      mode: "queued",
      text: status.text || "排队中，等待当前任务完成",
      severity: "muted",
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

function abnormalLineMode(mode: TaskStatsLineMode): boolean {
  return mode === "retry" || mode === "failed" || mode === "interrupted";
}

function taskStatsLineMotion(
  previousMode: TaskStatsLineMode,
  nextMode: TaskStatsLineMode,
): TaskStatsLineMotion {
  const previousAbnormal = abnormalLineMode(previousMode);
  const nextAbnormal = abnormalLineMode(nextMode);
  if (!previousAbnormal && nextAbnormal) return "float-up";
  if (previousAbnormal && !nextAbnormal) return "float-down";
  return "fade";
}

export function TaskStatsCell({ task }: TaskStatsCellProps) {
  const line = taskStatsLine(task);
  const previousModeRef = useRef<TaskStatsLineMode>(line.mode);
  const motion = taskStatsLineMotion(previousModeRef.current, line.mode);

  useEffect(() => {
    previousModeRef.current = line.mode;
  }, [line.mode]);

  return (
    <div className="grid min-w-0 gap-1.5">
      <div className="grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-1.5 overflow-hidden">
        <div className="relative h-4 min-w-0 flex-1 overflow-hidden">
          <div
            key={line.mode}
            className={cn(
              "task-stats-line-fade absolute inset-x-0 top-0 truncate text-xs leading-4 transition-colors duration-150",
              motion === "float-up" && "task-stats-line-float-up",
              motion === "float-down" && "task-stats-line-float-down",
              taskStatsLineClass(line.severity),
            )}
            title={line.text}
          >
            {line.text}
          </div>
        </div>
        {liveTaskStatus(task.status) && (
          <Loader2 className="size-3 shrink-0 animate-spin text-muted-foreground" />
        )}
      </div>
      <div className="flex min-w-0 items-center gap-2">
        <Progress
          value={Math.round(Math.max(0, Math.min(1, task.progress)) * 100)}
          className="h-1.5 min-w-16 flex-1"
        />
        <span className="shrink-0 text-xs text-muted-foreground">
          {formatTokenK(task.tokenStats.totalTokens)} · {formatPercent(task.progress)}
        </span>
      </div>
    </div>
  );
}
