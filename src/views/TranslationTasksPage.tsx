import { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  BookOpen,
  Filter,
  Folder,
  ListChecks,
  MoreHorizontal,
  Pencil,
  Play,
  RefreshCw,
  RotateCcw,
  Search,
  Trash2,
} from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useToast } from "@/components/ui/toast-stack";
import {
  deleteTranslationTask,
  listTranslationTasks,
  resumeTranslationTask,
  retranslateTranslationTask,
  startTranslationTask,
  updateTranslationTaskTags,
} from "@/features/translation/api";
import {
  formatErrorRate,
  formatPercent,
  formatTokenK,
  statusLabel,
  unixTimeLabel,
} from "@/features/translation/format";
import type {
  TranslationProgressPayload,
  TranslationTaskStatus,
  TranslationTaskView,
} from "@/features/translation/types";
import { LanguageCombobox } from "@/features/languages/LanguageCombobox";
import {
  displayLanguage,
  normalizeLanguageCode,
  sameLanguage,
} from "@/features/languages/languageOptions";
import { cn } from "@/lib/utils";

type TaskTab = "running" | "completed" | "unfinished";

const ALL_FILTER_VALUE = "__all__";
const tagSplitPattern = /[,，\n]/;

function getErrorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

function isTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

function tabForStatus(status: TranslationTaskStatus): TaskTab {
  if (status === "running") return "running";
  if (status === "success") return "completed";
  return "unfinished";
}

function statusBadgeClass(status: TranslationTaskStatus): string {
  const classes: Record<TranslationTaskStatus, string> = {
    pending: "border-slate-400/25 bg-slate-500/10 text-slate-600 dark:text-slate-300",
    running: "border-blue-400/25 bg-blue-500/10 text-blue-600 dark:text-blue-300",
    interrupted: "border-amber-400/35 bg-amber-500/15 text-amber-700 dark:text-amber-300",
    failed: "border-destructive/30 bg-destructive/10 text-destructive",
    success: "border-emerald-400/30 bg-emerald-500/12 text-emerald-700 dark:text-emerald-300",
  };
  return classes[status];
}

function uniqueValues(values: string[]): string[] {
  return Array.from(
    new Set(values.map((value) => value.trim()).filter(Boolean)),
  ).sort((left, right) => displayLanguage(left).localeCompare(displayLanguage(right)));
}

function uniqueLanguageValues(values: string[]): string[] {
  const normalized = new Map<string, string>();
  for (const value of values) {
    const trimmed = value.trim();
    if (!trimmed) continue;
    normalized.set(normalizeLanguageCode(trimmed) ?? trimmed.toLowerCase(), trimmed);
  }
  return Array.from(normalized.values()).sort((left, right) => (
    displayLanguage(left).localeCompare(displayLanguage(right))
  ));
}

function normalizeTagDraft(value: string): string[] {
  const normalized: string[] = [];
  for (const rawTag of value.split(tagSplitPattern)) {
    const tag = rawTag.trim();
    if (!tag) continue;
    if (!normalized.some((item) => item.toLowerCase() === tag.toLowerCase())) {
      normalized.push(tag);
    }
  }
  return normalized;
}

function filtersAreActive(tag: string, sourceLanguage: string, targetLanguage: string): boolean {
  return [tag, sourceLanguage, targetLanguage].some((value) => value !== ALL_FILTER_VALUE);
}

interface TaskCardProps {
  task: TranslationTaskView;
  busyId: string;
  onStart: (task: TranslationTaskView) => void;
  onResume: (task: TranslationTaskView) => void;
  onRetranslate: (task: TranslationTaskView) => void;
  onDelete: (task: TranslationTaskView) => void;
  onEditTags: (task: TranslationTaskView) => void;
  onOpenGlossary: (task: TranslationTaskView) => void;
}

function TaskCard({
  task,
  busyId,
  onStart,
  onResume,
  onRetranslate,
  onDelete,
  onEditTags,
  onOpenGlossary,
}: TaskCardProps) {
  const busy = busyId === task.id;
  const progress = Math.round(Math.max(0, Math.min(1, task.progress)) * 100);

  return (
    <Card size="sm" className="rounded-[6px] py-3">
      <CardHeader className="px-3">
        <div className="flex min-w-0 items-start gap-2">
          <div className="min-w-0 flex-1">
            <CardTitle className="truncate">{task.name}</CardTitle>
            <div className="mt-0.5 truncate text-2xs text-muted-foreground">
              {displayLanguage(task.sourceLanguage)} → {displayLanguage(task.targetLanguage)} · {task.modelRequestName}
            </div>
          </div>
          <Badge variant="outline" className={cn("rounded-[6px]", statusBadgeClass(task.status))}>
            {statusLabel(task.status)}
          </Badge>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon-sm">
                <MoreHorizontal className="size-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem>
                <BookOpen className="size-4" />
                自动生成术语表
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => onEditTags(task)}>
                <Pencil className="size-4" />
                编辑标签
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => onOpenGlossary(task)}>
                <Search className="size-4" />
                选择已有术语表
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </CardHeader>
      <CardContent className="grid gap-3 px-3">
        {task.tags.length > 0 && (
          <div className="flex flex-wrap gap-1.5">
            {task.tags.map((tag) => (
              <Badge
                key={tag}
                variant="outline"
                className="max-w-40 rounded-[6px] bg-muted/35 text-2xs"
              >
                <Folder className="size-3" />
                <span className="truncate">{tag}</span>
              </Badge>
            ))}
          </div>
        )}
        <div className="grid gap-1.5">
          <div className="flex items-center justify-between text-2xs text-muted-foreground">
            <span>
              {task.completedChunks}/{task.totalChunks} 块完成
            </span>
            <span>{formatPercent(task.progress)}</span>
          </div>
          <Progress value={progress} />
        </div>
        <div className="grid grid-cols-[repeat(auto-fit,minmax(7rem,1fr))] gap-2 text-xs">
          <Stat label="输入" value={formatTokenK(task.tokenStats.inputTokens)} />
          <Stat label="输出" value={formatTokenK(task.tokenStats.outputTokens)} />
          <Stat label="缓存" value={formatTokenK(task.tokenStats.cachedTokens)} />
          <Stat label="思考" value={formatTokenK(task.tokenStats.thinkingTokens)} />
          <Stat label="总计" value={formatTokenK(task.tokenStats.totalTokens)} />
          <Stat label="错误率" value={formatErrorRate(task.errorRate)} />
        </div>
        {task.rateLimitStatus && (
          <div className="rounded-[6px] border bg-muted/35 px-2 py-1 text-2xs text-muted-foreground">
            {task.rateLimitStatus}
          </div>
        )}
        {task.lastError && (
          <div className="rounded-[6px] border border-destructive/25 bg-destructive/10 px-2 py-1 text-2xs text-destructive">
            {task.lastError}
          </div>
        )}
        <div className="flex flex-wrap items-center justify-between gap-2">
          <span className="text-2xs text-muted-foreground">
            更新于 {unixTimeLabel(task.updatedAt)}
          </span>
          <div className="flex flex-wrap justify-end gap-2">
            {task.status === "pending" && (
              <Button size="sm" disabled={busy} onClick={() => onStart(task)}>
                <Play className="size-4" />
                开始
              </Button>
            )}
            {task.status === "interrupted" && (
              <Button size="sm" disabled={busy} onClick={() => onResume(task)}>
                <RefreshCw className="size-4" />
                继续
              </Button>
            )}
            {(task.status === "success" || task.status === "failed") && (
              <Button size="sm" variant="outline" disabled={busy} onClick={() => onRetranslate(task)}>
                <RotateCcw className="size-4" />
                重新翻译
              </Button>
            )}
            <Button size="sm" variant="outline" disabled={busy || task.status === "running"} onClick={() => onDelete(task)}>
              <Trash2 className="size-4" />
              删除
            </Button>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-[6px] border bg-muted/25 px-2 py-1">
      <div className="text-3xs text-muted-foreground">{label}</div>
      <div className="font-medium">{value}</div>
    </div>
  );
}

export default function TranslationTasksPage() {
  const [tasks, setTasks] = useState<TranslationTaskView[]>([]);
  const [tab, setTab] = useState<TaskTab>("running");
  const [tagFilter, setTagFilter] = useState(ALL_FILTER_VALUE);
  const [sourceLanguageFilter, setSourceLanguageFilter] = useState(ALL_FILTER_VALUE);
  const [targetLanguageFilter, setTargetLanguageFilter] = useState(ALL_FILTER_VALUE);
  const [loading, setLoading] = useState(true);
  const [busyId, setBusyId] = useState("");
  const [tagEditorTask, setTagEditorTask] = useState<TranslationTaskView | null>(null);
  const [tagDraft, setTagDraft] = useState("");
  const [glossaryTask, setGlossaryTask] = useState<TranslationTaskView | null>(null);
  const [retranslateTarget, setRetranslateTarget] = useState<TranslationTaskView | null>(null);
  const { pushToast } = useToast();

  const activeFilters = useMemo(
    () => filtersAreActive(tagFilter, sourceLanguageFilter, targetLanguageFilter),
    [sourceLanguageFilter, tagFilter, targetLanguageFilter],
  );

  const visibleTasks = useMemo(
    () =>
      tasks.filter((task) => {
        if (
          tagFilter !== ALL_FILTER_VALUE &&
          !task.tags.some((tag) => tag.toLowerCase() === tagFilter.toLowerCase())
        ) {
          return false;
        }
        if (
          sourceLanguageFilter !== ALL_FILTER_VALUE &&
          !sameLanguage(task.sourceLanguage, sourceLanguageFilter)
        ) {
          return false;
        }
        if (
          targetLanguageFilter !== ALL_FILTER_VALUE &&
          !sameLanguage(task.targetLanguage, targetLanguageFilter)
        ) {
          return false;
        }
        return true;
      }),
    [sourceLanguageFilter, tagFilter, targetLanguageFilter, tasks],
  );

  const grouped = useMemo(() => {
    const groups: Record<TaskTab, TranslationTaskView[]> = {
      running: [],
      completed: [],
      unfinished: [],
    };
    for (const task of visibleTasks) {
      groups[tabForStatus(task.status)].push(task);
    }
    return groups;
  }, [visibleTasks]);

  const tagOptions = useMemo(
    () => uniqueValues(tasks.flatMap((task) => task.tags)),
    [tasks],
  );
  const sourceLanguageOptions = useMemo(
    () => uniqueLanguageValues(tasks.map((task) => task.sourceLanguage)),
    [tasks],
  );
  const targetLanguageOptions = useMemo(
    () => uniqueLanguageValues(tasks.map((task) => task.targetLanguage)),
    [tasks],
  );

  const refresh = useCallback(async (): Promise<void> => {
    setLoading(true);
    if (!isTauriRuntime()) {
      setTasks([]);
      setLoading(false);
      return;
    }
    try {
      setTasks(await listTranslationTasks());
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    } finally {
      setLoading(false);
    }
  }, [pushToast]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (!isTauriRuntime()) return undefined;
    let dispose: (() => void) | undefined;
    void listen<TranslationProgressPayload>("translation-progress", (event) => {
      setTasks((current) => {
        const incoming = event.payload.task;
        const exists = current.some((task) => task.id === incoming.id);
        if (!exists) return [incoming, ...current];
        return current.map((task) => (task.id === incoming.id ? incoming : task));
      });
    }).then((unlisten) => {
      dispose = unlisten;
    });
    return () => dispose?.();
  }, []);

  async function runAction(
    task: TranslationTaskView,
    action: (id: string) => Promise<TranslationTaskView>,
  ): Promise<void> {
    setBusyId(task.id);
    try {
      const updated = await action(task.id);
      setTasks((current) => current.map((item) => (item.id === updated.id ? updated : item)));
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    } finally {
      setBusyId("");
    }
  }

  async function removeTask(task: TranslationTaskView): Promise<void> {
    setBusyId(task.id);
    try {
      await deleteTranslationTask(task.id);
      setTasks((current) => current.filter((item) => item.id !== task.id));
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    } finally {
      setBusyId("");
    }
  }

  async function saveTaskTags(): Promise<void> {
    if (!tagEditorTask) return;
    const tags = normalizeTagDraft(tagDraft);
    setBusyId(tagEditorTask.id);
    try {
      const updated = await updateTranslationTaskTags({
        id: tagEditorTask.id,
        tags,
      });
      setTasks((current) => current.map((item) => (item.id === updated.id ? updated : item)));
      setTagEditorTask(null);
      pushToast("标签已更新", "success");
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    } finally {
      setBusyId("");
    }
  }

  function openTagEditor(task: TranslationTaskView): void {
    setTagEditorTask(task);
    setTagDraft(task.tags.join(", "));
  }

  function clearFilters(): void {
    setTagFilter(ALL_FILTER_VALUE);
    setSourceLanguageFilter(ALL_FILTER_VALUE);
    setTargetLanguageFilter(ALL_FILTER_VALUE);
  }

  return (
    <main className="flex min-w-0 flex-1 flex-col overflow-hidden p-3">
      <header className="mb-3 flex shrink-0 items-start justify-between gap-3">
        <div>
          <div className="flex items-center gap-2">
            <ListChecks className="size-5 text-primary" />
            <h1 className="text-xl font-medium tracking-tight">任务</h1>
          </div>
          <p className="mt-0.5 text-xs text-muted-foreground">
            查看翻译进度、token 统计和中断/失败任务。
          </p>
        </div>
        <Button variant="outline" size="sm" onClick={refresh} disabled={loading}>
          <RefreshCw className="size-4" />
          刷新
        </Button>
      </header>

      <div className="mb-3 grid shrink-0 gap-2 rounded-[6px] border bg-muted/20 p-2 md:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1fr)_auto]">
        <Select value={tagFilter} onValueChange={setTagFilter}>
          <SelectTrigger>
            <SelectValue placeholder="标签" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ALL_FILTER_VALUE}>全部标签</SelectItem>
            {tagOptions.map((tag) => (
              <SelectItem key={tag} value={tag}>
                {tag}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <LanguageCombobox
          value={sourceLanguageFilter}
          allValue={ALL_FILTER_VALUE}
          allLabel="全部原始语言"
          onValueChange={setSourceLanguageFilter}
          placeholder="原始语言"
          searchPlaceholder="搜索原始语言"
        />
        <LanguageCombobox
          value={targetLanguageFilter}
          allValue={ALL_FILTER_VALUE}
          allLabel="全部目标语言"
          onValueChange={setTargetLanguageFilter}
          placeholder="目标语言"
          searchPlaceholder="搜索目标语言"
        />
        <Button
          variant="outline"
          size="sm"
          className="md:w-24"
          onClick={clearFilters}
          disabled={!activeFilters}
        >
          <Filter className="size-4" />
          清空
        </Button>
      </div>

      <Tabs value={tab} onValueChange={(value) => setTab(value as TaskTab)} className="min-h-0 flex-1">
        <TabsList>
          <TabsTrigger value="running">进行中 {grouped.running.length}</TabsTrigger>
          <TabsTrigger value="completed">已完成 {grouped.completed.length}</TabsTrigger>
          <TabsTrigger value="unfinished">未完成 {grouped.unfinished.length}</TabsTrigger>
        </TabsList>
        {(["running", "completed", "unfinished"] as const).map((value) => (
          <TabsContent key={value} value={value} className="min-h-0 overflow-y-auto pr-1">
            <div className="grid gap-2">
              {grouped[value].length === 0 ? (
                <div className="rounded-[6px] border border-dashed p-6 text-center text-sm text-muted-foreground">
                  {loading
                    ? "正在读取任务..."
                    : activeFilters
                      ? "没有符合当前筛选的任务"
                      : "这里暂时没有任务"}
                </div>
              ) : (
                grouped[value].map((task) => (
                  <TaskCard
                    key={task.id}
                    task={task}
                    busyId={busyId}
                    onStart={(item) => void runAction(item, startTranslationTask)}
                    onResume={(item) => void runAction(item, resumeTranslationTask)}
                    onRetranslate={setRetranslateTarget}
                    onDelete={(item) => void removeTask(item)}
                    onEditTags={openTagEditor}
                    onOpenGlossary={setGlossaryTask}
                  />
                ))
              )}
            </div>
          </TabsContent>
        ))}
      </Tabs>

      <Dialog open={glossaryTask !== null} onOpenChange={(open) => !open && setGlossaryTask(null)}>
        <DialogContent open={glossaryTask !== null} className="max-w-xl">
          <DialogHeader>
            <DialogTitle>选择已有术语表</DialogTitle>
            <DialogDescription>
              术语表库稍后接入；这里先保留检索入口和任务上下文。
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-3">
            <Input placeholder="搜索术语表（占位）" />
            <div className="rounded-[6px] border border-dashed p-6 text-center text-sm text-muted-foreground">
              暂无可选术语表。当前任务：{glossaryTask?.name ?? "-"}
            </div>
          </div>
        </DialogContent>
      </Dialog>

      <Dialog open={tagEditorTask !== null} onOpenChange={(open) => !open && setTagEditorTask(null)}>
        <DialogContent open={tagEditorTask !== null} className="max-w-xl">
          <DialogHeader>
            <DialogTitle>编辑任务标签</DialogTitle>
            <DialogDescription>
              标签会作为任务分组使用，可用逗号或换行分隔。
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-3">
            <Textarea
              value={tagDraft}
              onChange={(event) => setTagDraft(event.target.value)}
              className="min-h-20"
              placeholder="例如：项目A, 客户资料, 待校对"
            />
            <div className="flex flex-wrap gap-1.5">
              {normalizeTagDraft(tagDraft).length === 0 ? (
                <span className="text-xs text-muted-foreground">当前任务未设置标签</span>
              ) : (
                normalizeTagDraft(tagDraft).map((tag) => (
                  <Badge key={tag} variant="outline" className="rounded-[6px] bg-muted/35">
                    <Folder className="size-3" />
                    {tag}
                  </Badge>
                ))
              )}
            </div>
            <div className="flex justify-end gap-2">
              <Button variant="outline" size="sm" onClick={() => setTagEditorTask(null)}>
                取消
              </Button>
              <Button
                size="sm"
                onClick={() => void saveTaskTags()}
                disabled={busyId === tagEditorTask?.id}
              >
                保存
              </Button>
            </div>
          </div>
        </DialogContent>
      </Dialog>

      <AlertDialog open={retranslateTarget !== null} onOpenChange={(open) => !open && setRetranslateTarget(null)}>
        <AlertDialogContent open={retranslateTarget !== null}>
          <AlertDialogTitle className="text-base font-semibold">确认重新翻译</AlertDialogTitle>
          <AlertDialogDescription className="text-sm text-muted-foreground">
            重新翻译将覆盖已有的译文，是否继续？
          </AlertDialogDescription>
          <div className="flex justify-end gap-2">
            <AlertDialogCancel className="inline-flex h-8 items-center rounded-[6px] border px-3 text-sm">
              取消
            </AlertDialogCancel>
            <AlertDialogAction
              className="inline-flex h-8 items-center rounded-[6px] bg-primary px-3 text-sm text-primary-foreground"
              onClick={() => {
                if (retranslateTarget) {
                  void runAction(retranslateTarget, retranslateTranslationTask);
                }
                setRetranslateTarget(null);
              }}
            >
              继续
            </AlertDialogAction>
          </div>
        </AlertDialogContent>
      </AlertDialog>
    </main>
  );
}
