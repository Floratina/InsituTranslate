import { Loader2 } from "lucide-react";
import { AnimatePresence, motion, type Variants } from "motion/react";

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
  key: string;
  mode: TaskStatsLineMode;
  text: string;
  severity: TaskStatsLineSeverity;
}

interface TaskStatsCellProps {
  task: TranslationTaskView;
}

function glossaryProgressStep(task: TranslationTaskView) {
  if (task.status !== "running" || !task.progressDetail) return null;
  const { glossary, translating } = task.progressDetail;
  if (
    glossary.state === "running"
    || (glossary.state === "pending" && translating.state === "pending")
  ) {
    return glossary;
  }
  return null;
}

const TASK_STATS_ROLL_DISTANCE = 22;

const TASK_STATS_ROLL_TRANSITION = {
  duration: 0.24,
  ease: [0.22, 0.61, 0.36, 0.99],
} as const;

const TASK_STATS_ROLL_EXIT_VARIANTS: Variants = {
  exit: (direction: number) => ({
    opacity: 0,
    y: direction * -TASK_STATS_ROLL_DISTANCE,
  }),
};

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
  const glossary = glossaryProgressStep(task);

  if (task.status === "failed") {
    return {
      key: "failed",
      mode: "failed",
      text: `失败：${status.text || "任务失败，请检查任务详情"}`,
      severity: "danger",
    };
  }

  if (task.status === "interrupted" || task.status === "interrupted-pending") {
    return {
      key: task.status,
      mode: "interrupted",
      text: `中断：${status.text || "任务已中断，可继续"}`,
      severity: "warning",
    };
  }

  if (task.status === "queued") {
    return {
      key: "queued",
      mode: "queued",
      text: status.text || "任务排队中，等待开始",
      severity: "muted",
    };
  }

  if (task.activeRetry) {
    return {
      key: "retry",
      mode: "retry",
      text: `重试中 (${task.activeRetry.current}/${task.activeRetry.max})：${task.activeRetry.message}`,
      severity: "muted",
    };
  }

  if (glossary) {
    return {
      key: "glossary",
      mode: "normal",
      text: `正在生成自动术语表... (${glossary.current}/${glossary.total})`,
      severity: "muted",
    };
  }

  if (!task.enableTranslation && task.status === "success") {
    return {
      key: "glossary-success",
      mode: "normal",
      text: "术语表建立完成",
      severity: "muted",
    };
  }

  if (!task.enableTranslation) {
    return {
      key: "glossary-only",
      mode: "normal",
      text: "仅建立自动术语表",
      severity: "muted",
    };
  }

  return {
    key: "normal",
    mode: "normal",
    text: `翻译进度 (${task.completedChunks}/${task.totalChunks}) · 失败 (${task.failedChunks})`,
    severity: "muted",
  };
}

function abnormalLineMode(mode: TaskStatsLineMode): boolean {
  return mode === "retry" || mode === "failed" || mode === "interrupted";
}

function taskStatsRollDirection(
  nextMode: TaskStatsLineMode,
): number {
  return abnormalLineMode(nextMode) ? 1 : -1;
}

export function TaskStatsCell({ task }: TaskStatsCellProps) {
  const line = taskStatsLine(task);
  const glossary = glossaryProgressStep(task);
  const displayedProgress = task.status === "success"
    || (!task.enableTranslation && task.progressDetail?.glossary.state === "success")
    ? 1
    : (glossary?.percent ?? task.progress);
  const direction = taskStatsRollDirection(line.mode);

  return (
    <div className="grid min-w-0 gap-1.5">
      <div className="grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-1.5 overflow-hidden">
        <div className="relative h-4 min-w-0 flex-1 overflow-hidden">
          <AnimatePresence initial={false} mode="sync" custom={direction}>
            <motion.div
              key={line.key}
              variants={TASK_STATS_ROLL_EXIT_VARIANTS}
              initial={{
                opacity: 0,
                y: direction * TASK_STATS_ROLL_DISTANCE,
              }}
              animate={{ opacity: 1, y: 0 }}
              exit="exit"
              transition={TASK_STATS_ROLL_TRANSITION}
              className={cn(
                "absolute inset-x-0 top-0 truncate text-xs leading-4 will-change-[transform,opacity]",
                taskStatsLineClass(line.severity),
              )}
              title={line.text}
            >
              {line.text}
            </motion.div>
          </AnimatePresence>
        </div>
        {liveTaskStatus(task.status) && (
          <Loader2 className="size-3 shrink-0 animate-spin text-muted-foreground" />
        )}
      </div>
      <div className="flex min-w-0 items-center gap-2">
        <Progress
          value={Math.round(Math.max(0, Math.min(1, displayedProgress)) * 100)}
          className="h-1.5 min-w-16 flex-1"
        />
        <span className="shrink-0 text-xs text-muted-foreground">
          {glossary
            ? formatPercent(displayedProgress)
            : `${formatTokenK(task.tokenStats.totalTokens)} · ${formatPercent(displayedProgress)}`}
        </span>
      </div>
    </div>
  );
}
