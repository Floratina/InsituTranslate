import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { FilePenLine, RefreshCw } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useToast } from "@/components/ui/toast-stack";
import {
  getTranslationTaskDetail,
  listTranslationTasks,
} from "@/features/translation/api";
import { statusLabel } from "@/features/translation/format";
import type {
  TranslationTaskDetail,
  TranslationTaskView,
} from "@/features/translation/types";
import { appSessionCache } from "@/lib/session-cache";

function getErrorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

export default function ProofreadingPage() {
  const cachedTasks = appSessionCache.proofreadingTasks.read();
  const cachedSelectedTaskId = appSessionCache.proofreadingSelectedTaskId;
  const initialSelectedTaskId =
    cachedTasks?.some((task) => task.id === cachedSelectedTaskId)
      ? cachedSelectedTaskId
      : (cachedTasks?.[0]?.id ?? "");
  const cachedDetail = initialSelectedTaskId
    ? appSessionCache.proofreadingDetail(initialSelectedTaskId).read()
    : undefined;
  const [tasks, setTasks] = useState<TranslationTaskView[]>(cachedTasks ?? []);
  const [selectedTaskId, setSelectedTaskId] = useState(initialSelectedTaskId);
  const [detail, setDetail] = useState<TranslationTaskDetail | null>(cachedDetail ?? null);
  const [tasksLoading, setTasksLoading] = useState(!cachedTasks);
  const [detailLoading, setDetailLoading] = useState(Boolean(initialSelectedTaskId && !cachedDetail));
  const skipInitialTasksRefresh = useRef(Boolean(cachedTasks));
  const { pushToast } = useToast();
  const loading = tasksLoading || detailLoading;

  const selectedTask = useMemo(
    () => tasks.find((task) => task.id === selectedTaskId) ?? null,
    [selectedTaskId, tasks],
  );

  const refreshTasks = useCallback(async (force = false): Promise<void> => {
    const cached = force ? undefined : appSessionCache.proofreadingTasks.read();
    if (cached) {
      setTasks(cached);
      setSelectedTaskId((current) =>
        cached.some((task) => task.id === current) ? current : (cached[0]?.id ?? ""),
      );
      setTasksLoading(false);
      return;
    }

    setTasksLoading(true);
    try {
      const result = await (force
        ? appSessionCache.proofreadingTasks.refresh(listTranslationTasks)
        : appSessionCache.proofreadingTasks.loadOnce(listTranslationTasks));
      setTasks(result);
      setSelectedTaskId((current) =>
        result.some((task) => task.id === current) ? current : (result[0]?.id ?? ""),
      );
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    } finally {
      setTasksLoading(false);
    }
  }, [pushToast]);

  useEffect(() => {
    if (skipInitialTasksRefresh.current) {
      skipInitialTasksRefresh.current = false;
      return;
    }
    void refreshTasks();
  }, [refreshTasks]);

  useEffect(() => {
    if (!selectedTaskId) {
      setDetail(null);
      setDetailLoading(false);
      appSessionCache.proofreadingSelectedTaskId = "";
      return;
    }
    appSessionCache.proofreadingSelectedTaskId = selectedTaskId;
    const cached = appSessionCache.proofreadingDetail(selectedTaskId).read();
    if (cached) {
      setDetail(cached);
      setDetailLoading(false);
      return;
    }
    void refreshDetail(selectedTaskId);
  }, [selectedTaskId]);

  async function refreshDetail(taskId = selectedTaskId, force = false): Promise<void> {
    if (!taskId) return;
    const resource = appSessionCache.proofreadingDetail(taskId);
    const cached = force ? undefined : resource.read();
    if (cached) {
      setDetail(cached);
      setDetailLoading(false);
      return;
    }

    setDetailLoading(true);
    try {
      const nextDetail = await (force
        ? resource.refresh(() => getTranslationTaskDetail(taskId))
        : resource.loadOnce(() => getTranslationTaskDetail(taskId)));
      setDetail(nextDetail);
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    } finally {
      setDetailLoading(false);
    }
  }

  const sourceText = detail?.chunks.map((chunk) => chunk.sourceText).join("") ?? "";
  const translatedText = detail?.chunks.map((chunk) => chunk.translatedText).join("") ?? "";

  return (
    <main className="flex min-w-0 flex-1 flex-col overflow-hidden p-3">
      <header className="mb-3 flex shrink-0 items-start justify-between gap-3">
        <div>
          <div className="flex items-center gap-2">
            <FilePenLine className="size-5 text-primary" />
            <h1 className="text-xl font-medium tracking-tight">校对</h1>
          </div>
          <p className="mt-0.5 text-xs text-muted-foreground">
            V1 先按 chunks 顺序显示纯文本，后续再接编辑器和格式化预览。
          </p>
        </div>
        <Button variant="outline" size="sm" disabled={loading} onClick={() => void refreshDetail(selectedTaskId, true)}>
          <RefreshCw className="size-4" />
          刷新
        </Button>
      </header>

      <div className="mb-3 grid max-w-xl gap-2">
        {tasksLoading ? (
          <Skeleton className="h-10 w-full" />
        ) : (
        <Select value={selectedTaskId} onValueChange={setSelectedTaskId}>
          <SelectTrigger>
            <SelectValue placeholder="选择任务" />
          </SelectTrigger>
          <SelectContent>
            {tasks.map((task) => (
              <SelectItem key={task.id} value={task.id}>
                {task.name} · {statusLabel(task.status)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        )}
      </div>

      {tasksLoading ? (
        <div className="grid min-h-0 flex-1 grid-cols-1 gap-3 overflow-hidden lg:grid-cols-2">
          <TextPanelSkeleton />
          <TextPanelSkeleton />
        </div>
      ) : !selectedTask ? (
        <div className="rounded-[6px] border border-dashed p-6 text-center text-sm text-muted-foreground">
          暂无可校对任务
        </div>
      ) : detailLoading ? (
        <div className="grid min-h-0 flex-1 grid-cols-1 gap-3 overflow-hidden lg:grid-cols-2">
          <TextPanelSkeleton />
          <TextPanelSkeleton />
        </div>
      ) : (
        <div className="grid min-h-0 flex-1 grid-cols-1 gap-3 overflow-hidden lg:grid-cols-2">
          <TextPanel title="原文" text={sourceText} />
          <TextPanel title="译文" text={translatedText} />
        </div>
      )}
    </main>
  );
}

function TextPanel({ title, text }: { title: string; text: string }) {
  return (
    <Card size="sm" className="min-h-0 rounded-[6px] py-3">
      <CardHeader className="px-3">
        <div className="flex items-center gap-2">
          <FilePenLine className="size-4 text-primary" />
          <CardTitle>{title}</CardTitle>
        </div>
      </CardHeader>
      <CardContent className="min-h-0 overflow-y-auto px-3">
        <pre className="min-h-[20rem] whitespace-pre-wrap break-words rounded-[6px] border bg-muted/25 p-3 text-xs leading-relaxed">
          {text || "暂无内容"}
        </pre>
      </CardContent>
    </Card>
  );
}

function TextPanelSkeleton() {
  return (
    <Card size="sm" className="min-h-0 rounded-[6px] py-3">
      <CardHeader className="px-3">
        <div className="flex items-center gap-2">
          <Skeleton className="size-4" />
          <Skeleton className="h-5 w-20" />
        </div>
      </CardHeader>
      <CardContent className="min-h-0 overflow-y-auto px-3">
        <div className="grid min-h-[20rem] gap-2 rounded-[6px] border bg-muted/25 p-3">
          {Array.from({ length: 9 }).map((_, index) => (
            <Skeleton
              key={index}
              className={index % 3 === 2 ? "h-4 w-2/3" : "h-4 w-full"}
            />
          ))}
        </div>
      </CardContent>
    </Card>
  );
}
