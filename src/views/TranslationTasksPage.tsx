import {
  useCallback,
  useEffect,
  useLayoutEffect,
  memo,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import { listen } from "@tauri-apps/api/event";
import {
  ArrowUpDown,
  BookOpen,
  ChevronLeft,
  ChevronRight,
  Download,
  FilePenLine,
  FolderOpen,
  ListChecks,
  Loader2,
  MoreHorizontal,
  Pause,
  Pencil,
  Play,
  RefreshCw,
  RotateCcw,
  Search,
  Trash2,
} from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogField,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useToast } from "@/components/ui/toast-stack";
import { glossaryFileAvailable } from "@/features/glossary/api";
import {
  deleteTranslationTask,
  deleteTranslationTasks,
  exportTranslationTask,
  getTranslationConfig,
  listTranslationTasks,
  openTranslationTaskFolder,
  replaceTaskRuntimeSnapshot,
  updateTranslationConfig,
  updateTranslationTaskInfo,
} from "@/features/translation/api";
import {
  dispatchSchedulerAction,
  type SchedulerAction,
} from "@/features/scheduler/api";
import {
  formatPercent,
  formatTokenK,
  statusLabel,
  unixTimeLabel,
} from "@/features/translation/format";
import { TaskStatsCell } from "@/features/translation/TaskStatsCell";
import type {
  ExportTranslationTaskInput,
  ProgressStep,
  TranslationProgressPayload,
  TranslationTaskExportFormat,
  TranslationTaskStatus,
  TranslationTaskView,
  TaskRuntimeActionRequired,
} from "@/features/translation/types";
import { LanguageCombobox } from "@/features/languages/LanguageCombobox";
import {
  displayLanguage,
  displayLanguagePair,
  normalizeLanguageCode,
  sameLanguage,
} from "@/features/languages/languageOptions";
import { cn } from "@/lib/utils";
import { appSessionCache } from "@/lib/session-cache";

type TaskTab = "running" | "completed" | "unfinished";
type SortMode = "created-desc" | "created-asc" | "az";
type TaskSortField = "name" | "stats" | "tags" | "language";

interface RuntimeActionDialogState {
  requirement: TaskRuntimeActionRequired;
}

const compactAccentButtonClass = cn(
  "!border-[var(--button-accent-border)] !bg-[var(--button-accent-bg)] !text-primary-foreground",
  "hover:!border-[var(--button-accent-hover-border)] hover:!bg-[var(--button-accent-hover-bg)] hover:!text-primary-foreground",
  "active:!border-[var(--button-accent-pressed-border)] active:!bg-[var(--button-accent-pressed-bg)]",
);

const compactStandardButtonClass = cn(
  "!border-[var(--button-standard-border)] !bg-[var(--button-standard-bg)] !text-foreground",
  "hover:!border-[var(--button-standard-hover-border)] hover:!bg-[var(--button-standard-hover-bg)] hover:!text-foreground",
  "active:!border-[var(--button-standard-pressed-border)] active:!bg-[var(--button-standard-pressed-bg)]",
);

interface TaskSortState {
  field: TaskSortField;
  mode: SortMode;
}

type TaskRefreshMode = "visible" | "silent";

interface TaskInfoState {
  task: TranslationTaskView;
  name: string;
  tags: string;
}

interface ExportState {
  task: TranslationTaskView;
  format: TranslationTaskExportFormat;
  outputName: string;
  pageSize: string;
  margin: string;
  scale: number;
}

interface TranslationTasksPageProps {
  onOpenProofreading?: (taskId: string) => void;
  onOpenGlossary?: (glossaryId: string) => void;
}

const ALL_FILTER_VALUE = "__all__";
const DEFAULT_PAGE_SIZE = 20;
const PAGE_SIZE_OPTIONS = [10, 20, 50, 100] as const;
const ACTION_COLUMN_WIDTH = 64;
const TASK_MIN_WIDTHS = [144, 176, 104, 128];
const TASK_INITIAL_WIDTHS = [340, 260, 220, 260];
const TASK_MAX_WIDTHS = [720, 520, 480, 460];
const TASK_FLEX_COLUMNS = [0, 1, 2, 3];
const TASK_HEADERS = ["名称", "统计", "标签", "语言"] as const;
const sortLabels: Record<SortMode, string> = {
  "created-desc": "添加时间倒序",
  "created-asc": "添加时间正序",
  az: "A-Z 排序",
};
const collator = new Intl.Collator(["zh-Hans-u-co-pinyin", "ja", "ko", "en"], {
  numeric: true,
  sensitivity: "base",
});

function isTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

function getErrorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

function splitTags(value: string): string[] {
  return value
    .split(/[，,]/)
    .map((tag) => tag.trim())
    .filter(Boolean);
}

function taskTab(status: TranslationTaskStatus): TaskTab {
  if (status === "success") return "completed";
  if (status === "failed") return "unfinished";
  return "running";
}

function startableTaskStatus(status: TranslationTaskStatus): boolean {
  return status === "pending" || status === "interrupted";
}

function sameTaskSignature(left: TranslationTaskView, right: TranslationTaskView): boolean {
  return left.id === right.id
    && left.name === right.name
    && left.status === right.status
    && left.enableTranslation === right.enableTranslation
    && left.glossaryId === right.glossaryId
    && left.progress === right.progress
    && left.completedChunks === right.completedChunks
    && left.failedChunks === right.failedChunks
    && left.interruptedChunks === right.interruptedChunks
    && left.totalChunks === right.totalChunks
    && left.updatedAt === right.updatedAt
    && left.tokenStats.totalTokens === right.tokenStats.totalTokens
    && left.textTokenStats.totalTokens === right.textTokenStats.totalTokens
    && left.lastError === right.lastError
    && left.rateLimitStatus === right.rateLimitStatus
    && left.activeRetry?.current === right.activeRetry?.current
    && left.activeRetry?.message === right.activeRetry?.message
    && sameProgressDetail(left.progressDetail, right.progressDetail)
    && left.tags.join("\u0000") === right.tags.join("\u0000");
}

function sameProgressStep(left: ProgressStep, right: ProgressStep): boolean {
  return left.state === right.state
    && left.current === right.current
    && left.total === right.total
    && left.percent === right.percent
    && left.label === right.label;
}

function sameProgressDetail(
  left: TranslationTaskView["progressDetail"],
  right: TranslationTaskView["progressDetail"],
): boolean {
  if (left === right) return true;
  if (!left || !right) return false;
  return sameProgressStep(left.ast, right.ast)
    && sameProgressStep(left.chunking, right.chunking)
    && sameProgressStep(left.glossary, right.glossary)
    && sameProgressStep(left.translating, right.translating)
    && sameProgressStep(left.restore, right.restore);
}

function mergeTaskList(
  current: TranslationTaskView[],
  updates: Iterable<TranslationTaskView>,
): TranslationTaskView[] {
  let changed = false;
  const byId = new Map(current.map((task) => [task.id, task]));
  for (const incoming of updates) {
    const previous = byId.get(incoming.id);
    if (!previous) {
      byId.set(incoming.id, incoming);
      changed = true;
    } else if (!sameTaskSignature(previous, incoming)) {
      byId.set(incoming.id, incoming);
      changed = true;
    }
  }
  if (!changed) return current;
  const merged = current.map((task) => byId.get(task.id) ?? task);
  const known = new Set(current.map((task) => task.id));
  for (const incoming of byId.values()) {
    if (!known.has(incoming.id)) merged.unshift(incoming);
  }
  return merged;
}

function mergeTaskProgress(
  current: TranslationTaskView[],
  updates: Iterable<TranslationTaskView>,
): TranslationTaskView[] {
  const statusById = new Map(current.map((task) => [task.id, task.status]));
  return mergeTaskList(
    current,
    Array.from(updates, (task) => ({
      ...task,
      status: statusById.get(task.id) ?? task.status,
    })),
  );
}

function reconcileTaskList(
  current: TranslationTaskView[],
  refreshed: TranslationTaskView[],
): TranslationTaskView[] {
  let changed = current.length !== refreshed.length;
  const currentById = new Map(current.map((task) => [task.id, task]));
  const next = refreshed.map((task) => {
    const previous = currentById.get(task.id);
    if (previous && sameTaskSignature(previous, task)) return previous;
    changed = true;
    return task;
  });
  return changed ? next : current;
}

function nextSortMode(mode: SortMode): SortMode {
  if (mode === "created-desc") return "created-asc";
  if (mode === "created-asc") return "az";
  return "created-desc";
}

function sum(values: number[]): number {
  return values.reduce((total, value) => total + value, 0);
}

function measureTextWidth(text: string): number {
  const canvas = document.createElement("canvas");
  const context = canvas.getContext("2d");
  if (!context) return text.length * 8;
  context.font = "12px Inter, Segoe UI, sans-serif";
  return context.measureText(text).width;
}

function autoWidth(values: string[], min: number, max: number): number {
  const widest = values.reduce(
    (current, value) => Math.max(current, measureTextWidth(value)),
    0,
  );
  return Math.max(min, Math.min(max, Math.ceil(widest + 44)));
}

function fitColumnWidths(
  widths: number[],
  minWidths: number[],
  containerWidth: number,
  flexColumns: number[],
): number[] {
  const next = widths.map((width, index) => Math.max(width, minWidths[index] ?? 88));
  if (containerWidth <= 0) return next;
  const target = Math.max(Math.floor(containerWidth), sum(minWidths));
  let current = sum(next);
  if (current > target) {
    let overflow = current - target;
    while (overflow > 0.5) {
      const shrinkable = next
        .map((width, index) => ({ index, capacity: width - (minWidths[index] ?? 88) }))
        .filter((item) => item.capacity > 0.5);
      const totalCapacity = shrinkable.reduce((total, item) => total + item.capacity, 0);
      if (totalCapacity <= 0) break;
      shrinkable.forEach(({ index, capacity }) => {
        const shrink = Math.min(capacity, overflow * (capacity / totalCapacity));
        next[index] -= shrink;
      });
      const adjusted = current - sum(next);
      if (adjusted <= 0.5) break;
      overflow -= adjusted;
      current = sum(next);
    }
  } else if (current < target) {
    const growColumns = flexColumns.filter((index) => index < next.length);
    const totalBase = growColumns.reduce((total, index) => total + Math.max(next[index], 1), 0);
    if (growColumns.length > 0 && totalBase > 0) {
      const extra = target - current;
      growColumns.forEach((index) => {
        next[index] += extra * (Math.max(next[index], 1) / totalBase);
      });
    }
  }
  return next.map((width, index) => Math.max(minWidths[index] ?? 88, Math.round(width)));
}

function useElementWidth<T extends HTMLElement>() {
  const ref = useRef<T | null>(null);
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const updateWidth = (): void => setWidth(element.clientWidth);
    updateWidth();
    if (typeof ResizeObserver === "undefined") {
      window.addEventListener("resize", updateWidth);
      return () => window.removeEventListener("resize", updateWidth);
    }
    const observer = new ResizeObserver(updateWidth);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return [ref, width] as const;
}

function useAdaptiveColumnWidths<T extends HTMLElement>(
  widths: number[],
  minWidths: number[],
  flexColumns: number[],
  reservedWidth = 0,
) {
  const [ref, containerWidth] = useElementWidth<T>();
  const availableWidth = Math.max(0, containerWidth - reservedWidth);
  const adaptiveWidths = useMemo(
    () => fitColumnWidths(widths, minWidths, availableWidth, flexColumns),
    [availableWidth, flexColumns, minWidths, widths],
  );
  return [ref, adaptiveWidths, containerWidth] as const;
}

function startResize(
  event: ReactPointerEvent<HTMLButtonElement>,
  columnIndex: number,
  widths: number[],
  minWidths: number[],
  setWidths: (next: number[]) => void,
): void {
  event.preventDefault();
  const partnerIndex = columnIndex + 1;
  if (partnerIndex >= widths.length) return;
  const startX = event.clientX;
  const startWidth = widths[columnIndex];
  const partnerStartWidth = widths[partnerIndex];
  const onPointerMove = (moveEvent: PointerEvent): void => {
    const minWidth = minWidths[columnIndex] ?? 88;
    const partnerMinWidth = minWidths[partnerIndex] ?? 88;
    const rawDelta = moveEvent.clientX - startX;
    const delta = Math.max(
      minWidth - startWidth,
      Math.min(rawDelta, partnerStartWidth - partnerMinWidth),
    );
    setWidths(widths.map((width, index) => {
      if (index === columnIndex) return Math.round(startWidth + delta);
      if (index === partnerIndex) return Math.round(partnerStartWidth - delta);
      return width;
    }));
  };
  const onPointerUp = (): void => {
    window.removeEventListener("pointermove", onPointerMove);
    window.removeEventListener("pointerup", onPointerUp);
  };
  window.addEventListener("pointermove", onPointerMove);
  window.addEventListener("pointerup", onPointerUp, { once: true });
}

function uniqueValues(values: string[]): string[] {
  return Array.from(new Set(values.map((value) => value.trim()).filter(Boolean))).sort(
    (left, right) => collator.compare(left, right),
  );
}

function uniqueLanguageValues(values: string[]): string[] {
  const normalized = new Map<string, string>();
  for (const value of values) {
    const trimmed = value.trim();
    if (!trimmed) continue;
    normalized.set(normalizeLanguageCode(trimmed) ?? trimmed.toLowerCase(), trimmed);
  }
  return Array.from(normalized.values()).sort((left, right) =>
    collator.compare(displayLanguage(left), displayLanguage(right)),
  );
}

function taskSearchText(task: TranslationTaskView): string {
  return [
    task.name,
    task.sourcePath,
    task.modelRequestName,
    statusLabel(task.status),
    displayLanguagePair(task.sourceLanguage, task.targetLanguage),
    task.tags.join(" "),
  ].join(" ").toLocaleLowerCase();
}

function taskStatsLabel(task: TranslationTaskView): string {
  return [
    statusLabel(task.status),
    `${task.completedChunks}/${task.totalChunks}`,
    formatPercent(task.status === "success" ? 1 : task.progress),
    formatTokenK(task.tokenStats.totalTokens),
  ].join(" ");
}

function sortKey(task: TranslationTaskView, field: TaskSortField): string {
  if (field === "name") return task.name;
  if (field === "stats") return taskStatsLabel(task);
  if (field === "tags") return task.tags.join(" ");
  return displayLanguagePair(task.sourceLanguage, task.targetLanguage);
}

function timestampMillis(value: string): number {
  const timestamp = Number(value);
  if (!Number.isFinite(timestamp) || timestamp <= 0) return 0;
  return timestamp >= 1_000_000_000_000 ? timestamp : timestamp * 1000;
}

function compareTaskTimestamps(
  left: TranslationTaskView,
  right: TranslationTaskView,
): number {
  return timestampMillis(left.createdAt) - timestampMillis(right.createdAt)
    || timestampMillis(left.updatedAt) - timestampMillis(right.updatedAt)
    || left.id.localeCompare(right.id);
}

function sortTasks(tasks: TranslationTaskView[], sort: TaskSortState): TranslationTaskView[] {
  const next = [...tasks];
  if (sort.mode === "created-desc") {
    return next.sort((left, right) => compareTaskTimestamps(right, left));
  }
  if (sort.mode === "created-asc") {
    return next.sort(compareTaskTimestamps);
  }
  return next.sort((left, right) =>
    collator.compare(sortKey(left, sort.field), sortKey(right, sort.field)) ||
    compareTaskTimestamps(right, left),
  );
}

function sourceFormatLabel(task: TranslationTaskView): string {
  const extension = task.sourcePath.split(".").pop()?.toLowerCase();
  if (extension === "md") return "Markdown";
  if (extension === "txt") return "TXT";
  return extension ? extension.toUpperCase() : "源格式";
}

function exportBaseName(task: TranslationTaskView): string {
  return task.name.replace(/\.[^.]+$/, "");
}

export default function TranslationTasksPage({ onOpenProofreading, onOpenGlossary }: TranslationTasksPageProps) {
  const { pushToast } = useToast();
  const [tasks, setTasks] = useState<TranslationTaskView[]>([]);
  const [tab, setTab] = useState<TaskTab>("running");
  const [search, setSearch] = useState("");
  const [tagFilter, setTagFilter] = useState(ALL_FILTER_VALUE);
  const [sourceLanguageFilter, setSourceLanguageFilter] = useState(ALL_FILTER_VALUE);
  const [targetLanguageFilter, setTargetLanguageFilter] = useState(ALL_FILTER_VALUE);
  const [sort, setSort] = useState<TaskSortState>({ field: "name", mode: "created-desc" });
  const [sortLoading, setSortLoading] = useState<TaskSortField | null>(null);
  const [widths, setWidths] = useState(TASK_INITIAL_WIDTHS);
  const [page, setPage] = useState(0);
  const [pageSize, setPageSize] = useState(DEFAULT_PAGE_SIZE);
  const [loading, setLoading] = useState(true);
  const [busyId, setBusyId] = useState("");
  const [batchBusy, setBatchBusy] = useState(false);
  const [taskInfoState, setTaskInfoState] = useState<TaskInfoState | null>(null);
  const [exportState, setExportState] = useState<ExportState | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<TranslationTaskView | null>(null);
  const [missingGlossaryTask, setMissingGlossaryTask] = useState<TranslationTaskView | null>(null);
  const [clearTargets, setClearTargets] = useState<TranslationTaskView[] | null>(null);
  const [runtimeActionState, setRuntimeActionState] = useState<RuntimeActionDialogState | null>(null);
  const silentRefreshPromiseRef = useRef<Promise<void> | null>(null);

  const filteredTasks = useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    return tasks.filter((task) => {
      if (query && !taskSearchText(task).includes(query)) return false;
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
    });
  }, [search, sourceLanguageFilter, tagFilter, targetLanguageFilter, tasks]);

  const grouped = useMemo(() => {
    const groups: Record<TaskTab, TranslationTaskView[]> = {
      running: [],
      completed: [],
      unfinished: [],
    };
    for (const task of filteredTasks) groups[taskTab(task.status)].push(task);
    return groups;
  }, [filteredTasks]);

  const sortedTasks = useMemo(() => sortTasks(grouped[tab], sort), [grouped, sort, tab]);
  const totalPages = Math.max(1, Math.ceil(sortedTasks.length / pageSize));
  const visibleStartableTasks = useMemo(
    () => sortedTasks.filter((task) => startableTaskStatus(task.status)),
    [sortedTasks],
  );
  const visibleRetranslatableTasks = useMemo(
    () => sortedTasks.filter((task) => task.status === "success"),
    [sortedTasks],
  );
  const pagedTasks = useMemo(() => {
    const start = page * pageSize;
    return sortedTasks.slice(start, start + pageSize);
  }, [page, pageSize, sortedTasks]);

  const tagOptions = useMemo(() => uniqueValues(tasks.flatMap((task) => task.tags)), [tasks]);
  const sourceLanguageOptions = useMemo(
    () => uniqueLanguageValues(tasks.map((task) => task.sourceLanguage)),
    [tasks],
  );
  const targetLanguageOptions = useMemo(
    () => uniqueLanguageValues(tasks.map((task) => task.targetLanguage)),
    [tasks],
  );

  const refresh = useCallback((mode: TaskRefreshMode = "visible"): Promise<void> => {
    if (mode === "silent" && silentRefreshPromiseRef.current) {
      return silentRefreshPromiseRef.current;
    }

    const request = (async (): Promise<void> => {
      if (mode === "visible") setLoading(true);
      if (!isTauriRuntime()) {
        setTasks([]);
        if (mode === "visible") setLoading(false);
        return;
      }
      try {
        const refreshed = await listTranslationTasks();
        setTasks((current) => reconcileTaskList(current, refreshed));
      } catch (error) {
        pushToast(getErrorMessage(error), "error");
      } finally {
        if (mode === "visible") {
          setLoading(false);
          setSortLoading(null);
        }
      }
    })();

    if (mode === "silent") {
      silentRefreshPromiseRef.current = request;
      void request.finally(() => {
        if (silentRefreshPromiseRef.current === request) {
          silentRefreshPromiseRef.current = null;
        }
      });
    }
    return request;
  }, [pushToast]);

  useEffect(() => {
    void refresh("visible");
  }, [refresh]);

  useEffect(() => {
    if (!isTauriRuntime()) return undefined;
    let disposed = false;
    let disposeProgress: (() => void) | undefined;
    let disposeStatus: (() => void) | undefined;
    const pending = new Map<string, TranslationTaskView>();
    let flushTimer: number | null = null;
    const flush = (): void => {
      flushTimer = null;
      const updates = Array.from(pending.values());
      pending.clear();
      setTasks((current) => mergeTaskProgress(current, updates));
    };
    void listen<TranslationProgressPayload>("translation-progress", (event) => {
      pending.set(event.payload.task.id, event.payload.task);
      if (flushTimer === null) {
        flushTimer = window.setTimeout(flush, 250);
      }
    }).then((unlisten) => {
      if (disposed) unlisten();
      else disposeProgress = unlisten;
    });
    void listen<TranslationProgressPayload>("task-status-changed", (event) => {
      setTasks((current) => mergeTaskList(current, [event.payload.task]));
    }).then((unlisten) => {
      if (disposed) unlisten();
      else disposeStatus = unlisten;
    });
    return () => {
      disposed = true;
      disposeProgress?.();
      disposeStatus?.();
      if (flushTimer !== null) window.clearTimeout(flushTimer);
    };
  }, []);

  useEffect(() => {
    if (!isTauriRuntime()) return undefined;
    const syncOnFocus = (): void => {
      if (document.visibilityState === "visible") void refresh("silent");
    };
    window.addEventListener("focus", syncOnFocus);
    document.addEventListener("visibilitychange", syncOnFocus);
    return () => {
      window.removeEventListener("focus", syncOnFocus);
      document.removeEventListener("visibilitychange", syncOnFocus);
    };
  }, [refresh]);

  useEffect(() => {
    setPage(0);
  }, [search, tagFilter, sourceLanguageFilter, targetLanguageFilter, tab, sort, pageSize]);

  useEffect(() => {
    setPage((current) => Math.min(current, totalPages - 1));
  }, [totalPages]);

  function mergeTask(updated: TranslationTaskView): void {
    setTasks((current) => current.map((task) => (task.id === updated.id ? updated : task)));
  }

  async function runTaskAction(
    task: TranslationTaskView,
    action: SchedulerAction,
  ): Promise<void> {
    setBusyId(task.id);
    try {
      const ack = await dispatchSchedulerAction(action);
      if (ack.actionRequired) {
        setRuntimeActionState({ requirement: ack.actionRequired });
        return;
      }
      if (!ack.success) {
        const message = ack.message ?? "调度指令未被接受";
        if (
          action.type === "retranslate"
          && !task.enableTranslation
          && message.includes("Glossary not found")
        ) {
          setMissingGlossaryTask(task);
          return;
        }
        pushToast(message, "error");
      }
    } catch (error) {
      const message = getErrorMessage(error);
      if (
        action.type === "retranslate"
        && !task.enableTranslation
        && message.includes("Glossary not found")
      ) {
        setMissingGlossaryTask(task);
        return;
      }
      pushToast(message, "error");
    } finally {
      setBusyId("");
    }
  }

  async function replaceRuntimeAndRetranslate(): Promise<void> {
    if (!runtimeActionState) return;
    const taskId = runtimeActionState.requirement.taskId;
    setBusyId(taskId);
    try {
      const draft = appSessionCache.startDraft.read();
      const currentConfig = draft
        ? {
            ...draft.config,
            sourceLanguage: draft.sourceLanguage,
            targetLanguage: draft.targetLanguage,
          }
        : await getTranslationConfig();
      const config = await updateTranslationConfig(currentConfig);
      appSessionCache.translationConfig.set(config);
      await replaceTaskRuntimeSnapshot({ taskId, config });
      const ack = await dispatchSchedulerAction({ type: "retranslate", taskId });
      if (!ack.success) {
        pushToast(ack.message ?? "重新翻译指令未被接受", "error");
        return;
      }
      setRuntimeActionState(null);
      pushToast("任务已使用当前开始页配置重新开始", "success");
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    } finally {
      setBusyId("");
    }
  }

  async function saveTaskInfo(): Promise<void> {
    if (!taskInfoState) return;
    if (!taskInfoState.name.trim()) {
      pushToast("任务名称不能为空", "warning");
      return;
    }
    setBusyId(taskInfoState.task.id);
    try {
      mergeTask(await updateTranslationTaskInfo({
        id: taskInfoState.task.id,
        name: taskInfoState.name,
        tags: splitTags(taskInfoState.tags),
      }));
      setTaskInfoState(null);
      pushToast("任务信息已更新", "success");
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    } finally {
      setBusyId("");
    }
  }

  async function runExport(): Promise<void> {
    if (!exportState) return;
    const input: ExportTranslationTaskInput = {
      id: exportState.task.id,
      format: exportState.format,
      outputName: exportState.outputName,
      pdfOptions: exportState.format === "source"
        ? null
        : {
          pageSize: exportState.pageSize,
          margin: exportState.margin,
          scale: exportState.scale,
        },
    };
    setBusyId(exportState.task.id);
    try {
      await exportTranslationTask(input);
      setExportState(null);
      pushToast("任务已导出", "success");
    } catch (error) {
      const message = getErrorMessage(error);
      if (message !== "Export cancelled") {
        pushToast(message === "PDF export is not implemented yet" ? "PDF 导出暂未实现" : message, "error");
      }
    } finally {
      setBusyId("");
    }
  }

  async function removeTask(task: TranslationTaskView): Promise<boolean> {
    setBusyId(task.id);
    try {
      await deleteTranslationTask(task.id);
      setTasks((current) => current.filter((item) => item.id !== task.id));
      pushToast("任务已删除", "success");
      return true;
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
      return false;
    } finally {
      setBusyId("");
    }
  }

  async function deleteOne(): Promise<void> {
    if (!deleteTarget) return;
    if (await removeTask(deleteTarget)) setDeleteTarget(null);
  }

  async function deleteMissingGlossaryTask(): Promise<void> {
    if (!missingGlossaryTask) return;
    if (await removeTask(missingGlossaryTask)) setMissingGlossaryTask(null);
  }

  async function openTaskGlossary(task: TranslationTaskView): Promise<void> {
    if (!task.glossaryId) return;
    try {
      if (!await glossaryFileAvailable(task.glossaryId)) {
        setMissingGlossaryTask(task);
        return;
      }
      onOpenGlossary?.(task.glossaryId);
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    }
  }

  async function retranslateTask(task: TranslationTaskView): Promise<void> {
    if (!task.enableTranslation && task.glossaryId) {
      try {
        if (!await glossaryFileAvailable(task.glossaryId)) {
          setMissingGlossaryTask(task);
          return;
        }
      } catch (error) {
        pushToast(getErrorMessage(error), "error");
        return;
      }
    }
    await runTaskAction(task, { type: "retranslate", taskId: task.id });
  }

  async function clearVisibleTasks(): Promise<void> {
    if (!clearTargets) return;
    setBatchBusy(true);
    try {
      const ids = clearTargets.map((task) => task.id);
      await deleteTranslationTasks({ ids });
      setTasks((current) => current.filter((task) => !ids.includes(task.id)));
      setClearTargets(null);
      pushToast("当前列表任务已清空", "success");
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    } finally {
      setBatchBusy(false);
    }
  }

  async function startVisibleTasks(): Promise<void> {
    const ids = visibleStartableTasks.map((task) => task.id);
    if (ids.length === 0) {
      pushToast("当前列表没有可开始的任务", "warning");
      return;
    }
    setBatchBusy(true);
    try {
      const ack = await dispatchSchedulerAction({ type: "enqueueBatch", taskIds: ids });
      if (ack.actionRequired) {
        setRuntimeActionState({ requirement: ack.actionRequired });
        return;
      }
      if (!ack.success) {
        pushToast(ack.message ?? "批量开始指令未被接受", "error");
        return;
      }
      pushToast("已加入顺序开始队列", "success");
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    } finally {
      setBatchBusy(false);
    }
  }

  async function retranslateVisibleTasks(): Promise<void> {
    const ids = visibleRetranslatableTasks.map((task) => task.id);
    if (ids.length === 0) {
      pushToast("当前列表没有可重新执行的任务", "warning");
      return;
    }
    setBatchBusy(true);
    try {
      const ack = await dispatchSchedulerAction({ type: "retranslateBatch", taskIds: ids });
      if (ack.actionRequired) {
        setRuntimeActionState({ requirement: ack.actionRequired });
        return;
      }
      if (!ack.success) {
        pushToast(ack.message ?? "批量重新执行指令未被接受", "error");
        return;
      }
      pushToast("已加入重新执行队列", "success");
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    } finally {
      setBatchBusy(false);
    }
  }

  async function pauseVisibleTasks(): Promise<void> {
    setBatchBusy(true);
    try {
      const ack = await dispatchSchedulerAction({ type: "pauseAll" });
      if (!ack.success) {
        pushToast(ack.message ?? "全部暂停指令未被接受", "error");
        return;
      }
      pushToast("已请求暂停任务", "success");
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    } finally {
      setBatchBusy(false);
    }
  }

  function openExport(task: TranslationTaskView): void {
    setExportState({
      task,
      format: "source",
      outputName: exportBaseName(task),
      pageSize: "A4",
      margin: "normal",
      scale: 1,
    });
  }

  function updateSort(field: TaskSortField): void {
    setSortLoading(field);
    setSort((current) => ({
      field,
      mode: current.field === field ? nextSortMode(current.mode) : "az",
    }));
    window.setTimeout(() => setSortLoading(null), 120);
  }

  function autoFitColumn(columnIndex: number): void {
    const values = pagedTasks.map((task) => {
      if (columnIndex === 0) return task.name;
      if (columnIndex === 1) return taskStatsLabel(task);
      if (columnIndex === 2) return task.tags.join(" ") || "无标签";
      return displayLanguagePair(task.sourceLanguage, task.targetLanguage);
    });
    setWidths((current) => current.map((width, index) => (
      index === columnIndex
        ? autoWidth([TASK_HEADERS[columnIndex], ...values], TASK_MIN_WIDTHS[columnIndex], TASK_MAX_WIDTHS[columnIndex])
        : width
    )));
  }

  return (
    <main className="flex min-w-0 flex-1 flex-col overflow-hidden p-3">
      <header className="mb-3 shrink-0">
        <div className="flex items-center gap-2">
          <ListChecks className="size-5 text-primary" />
          <h1 className="text-xl font-medium tracking-tight">任务</h1>
          <Badge variant="secondary" className="ml-1 rounded-[6px]">
            {tasks.length} 个
          </Badge>
          <Button
            variant="outline"
            size="sm"
            className="ml-auto"
            onClick={() => void refresh("visible")}
            disabled={loading}
          >
            <RefreshCw className="size-4" />
            刷新
          </Button>
        </div>
        <p className="mt-0.5 text-xs text-muted-foreground">
          管理 INP 文档任务和查看进度
        </p>
      </header>

      <div className="mb-3 grid shrink-0 gap-2 lg:grid-cols-[minmax(16rem,1fr)_11rem_11rem_11rem]">
        <div className="relative">
          <Search className="pointer-events-none absolute top-1/2 left-2 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            className="pl-8"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder="检索任务名称、标签、语言或模型"
          />
        </div>
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
      </div>

      <Tabs value={tab} onValueChange={(value) => setTab(value as TaskTab)} className="mb-2 shrink-0">
        <div className="flex flex-wrap items-center gap-2">
          <TabsList>
            <TabsTrigger value="running">进行中 {grouped.running.length}</TabsTrigger>
            <TabsTrigger value="completed">已完成 {grouped.completed.length}</TabsTrigger>
            <TabsTrigger value="unfinished">未完成 {grouped.unfinished.length}</TabsTrigger>
          </TabsList>
          <div className="ml-auto flex flex-wrap items-center gap-2">
            <Button
              size="sm"
              className={compactAccentButtonClass}
              onClick={() => void (tab === "completed" ? retranslateVisibleTasks() : startVisibleTasks())}
              disabled={
                batchBusy
                || (tab === "completed"
                  ? visibleRetranslatableTasks.length === 0
                  : visibleStartableTasks.length === 0)
              }
            >
              {tab === "completed" ? <RotateCcw className="size-4" /> : <Play className="size-4" />}
              {tab === "completed" ? "重新执行" : "全部开始"}
            </Button>
            <Button
              size="sm"
              variant="outline"
              className={compactStandardButtonClass}
              onClick={() => void pauseVisibleTasks()}
              disabled={
                batchBusy
                || !tasks.some((task) =>
                  task.status === "queued"
                  || task.status === "running"
                  || task.status === "interrupted-pending"
                )
              }
            >
              <Pause className="size-4" />
              全部暂停
            </Button>
            <Button
              size="sm"
              variant="destructive"
              onClick={() => setClearTargets(sortedTasks)}
              disabled={batchBusy || sortedTasks.length === 0}
            >
              <Trash2 className="size-4" />
              清空任务
            </Button>
          </div>
        </div>
      </Tabs>

      <TasksTable
        tasks={pagedTasks}
        loading={loading}
        page={page}
        pageSize={pageSize}
        totalItems={sortedTasks.length}
        totalPages={totalPages}
        sort={sort}
        sortLoading={sortLoading}
        widths={widths}
        busyId={busyId}
        onSort={updateSort}
        onPageChange={setPage}
        onPageSizeChange={setPageSize}
        onResize={(event, index, renderedWidths) => startResize(event, index, renderedWidths, TASK_MIN_WIDTHS, setWidths)}
        onAutoFit={autoFitColumn}
        onStart={(task) => void runTaskAction(task, { type: "enqueue", taskId: task.id })}
        onResume={(task) => void runTaskAction(task, { type: "enqueue", taskId: task.id })}
        onPause={(task) => void runTaskAction(task, { type: "pause", taskId: task.id })}
        onRetranslate={(task) => void retranslateTask(task)}
        onProofread={(task) => onOpenProofreading?.(task.id)}
        onOpenGlossary={(task) => void openTaskGlossary(task)}
        onEditInfo={(task) => setTaskInfoState({ task, name: task.name, tags: task.tags.join("，") })}
        onOpenFolder={(task) => {
          void openTranslationTaskFolder(task.id).catch((error: unknown) => {
            pushToast(getErrorMessage(error), "error");
          });
        }}
        onExport={openExport}
        onDelete={setDeleteTarget}
      />

      <TaskInfoDialog
        state={taskInfoState}
        saving={busyId === taskInfoState?.task.id}
        onOpenChange={(open) => {
          if (!open) setTaskInfoState(null);
        }}
        onNameChange={(name) => setTaskInfoState((current) => current ? { ...current, name } : current)}
        onTagsChange={(tags) => setTaskInfoState((current) => current ? { ...current, tags } : current)}
        onSubmit={() => void saveTaskInfo()}
      />
      <ExportDialog
        state={exportState}
        exporting={busyId === exportState?.task.id}
        onOpenChange={(open) => {
          if (!open) setExportState(null);
        }}
        onChange={setExportState}
        onSubmit={() => void runExport()}
      />
      <ConfirmDialog
        open={runtimeActionState !== null}
        title="请采取行动"
        description="执行该任务的模型或助手被删除，任务将以当前“开始”页面的配置重新开始。"
        confirmText="确定"
        destructive={false}
        onOpenChange={(open) => {
          if (!open) setRuntimeActionState(null);
        }}
        onConfirm={() => void replaceRuntimeAndRetranslate()}
      />
      <ConfirmDialog
        open={deleteTarget !== null}
        title="删除任务"
        description={`确认删除“${deleteTarget?.name ?? ""}”？对应的 INP 文件也会被删除。`}
        confirmText="删除"
        onOpenChange={(open) => {
          if (!open) setDeleteTarget(null);
        }}
        onConfirm={() => void deleteOne()}
      />
      <Dialog
        open={missingGlossaryTask !== null}
        onOpenChange={(open) => {
          if (!open) setMissingGlossaryTask(null);
        }}
      >
        <DialogContent open={missingGlossaryTask !== null} className="max-w-sm">
          <DialogHeader>
            <DialogTitle>未找到术语表文件</DialogTitle>
            <DialogDescription>
              术语表对应的 ING 文件已被删除或移动，您可以删除任务并重新上传文件以自动建立术语表。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter className="justify-between">
            <Button
              type="button"
              variant="outline"
              disabled={busyId === missingGlossaryTask?.id}
              onClick={() => setMissingGlossaryTask(null)}
            >
              取消
            </Button>
            <Button
              type="button"
              variant="destructive"
              disabled={busyId === missingGlossaryTask?.id}
              onClick={() => void deleteMissingGlossaryTask()}
            >
              {busyId === missingGlossaryTask?.id && <Loader2 className="size-4 animate-spin" />}
              删除任务
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <ConfirmDialog
        open={clearTargets !== null}
        title="清空当前列表任务"
        description={`此操作会直接删除当前列表下的 ${clearTargets?.length ?? 0} 个 INP 任务文件。`}
        confirmText="清空"
        onOpenChange={(open) => {
          if (!open) setClearTargets(null);
        }}
        onConfirm={() => void clearVisibleTasks()}
      />
    </main>
  );
}

interface TasksTableProps {
  tasks: TranslationTaskView[];
  loading: boolean;
  page: number;
  pageSize: number;
  totalItems: number;
  totalPages: number;
  sort: TaskSortState;
  sortLoading: TaskSortField | null;
  widths: number[];
  busyId: string;
  onSort: (field: TaskSortField) => void;
  onPageChange: (page: number) => void;
  onPageSizeChange: (pageSize: number) => void;
  onResize: (
    event: ReactPointerEvent<HTMLButtonElement>,
    index: number,
    widths: number[],
  ) => void;
  onAutoFit: (index: number) => void;
  onStart: (task: TranslationTaskView) => void;
  onResume: (task: TranslationTaskView) => void;
  onPause: (task: TranslationTaskView) => void;
  onRetranslate: (task: TranslationTaskView) => void;
  onProofread: (task: TranslationTaskView) => void;
  onOpenGlossary: (task: TranslationTaskView) => void;
  onEditInfo: (task: TranslationTaskView) => void;
  onOpenFolder: (task: TranslationTaskView) => void;
  onExport: (task: TranslationTaskView) => void;
  onDelete: (task: TranslationTaskView) => void;
}

function TasksTable({
  tasks,
  loading,
  page,
  pageSize,
  totalItems,
  totalPages,
  sort,
  sortLoading,
  widths,
  busyId,
  onSort,
  onPageChange,
  onPageSizeChange,
  onResize,
  onAutoFit,
  onStart,
  onResume,
  onPause,
  onRetranslate,
  onProofread,
  onOpenGlossary,
  onEditInfo,
  onOpenFolder,
  onExport,
  onDelete,
}: TasksTableProps) {
  const [contextTask, setContextTask] = useState<TranslationTaskView | null>(null);
  const [tableViewportRef, adaptiveWidths, tableViewportWidth] = useAdaptiveColumnWidths<HTMLDivElement>(
    widths,
    TASK_MIN_WIDTHS,
    TASK_FLEX_COLUMNS,
    ACTION_COLUMN_WIDTH,
  );
  const tableWidth = sum(adaptiveWidths) + ACTION_COLUMN_WIDTH;
  const tableNeedsHorizontalScroll = tableWidth > tableViewportWidth + 1;

  return (
    <section className="relative min-h-0 flex-1 overflow-hidden rounded-[6px] border bg-card">
      <ScrollArea
        axis={tableNeedsHorizontalScroll ? "both" : "vertical"}
        className="h-full"
        viewportClassName="overscroll-contain pb-20"
        viewportRef={tableViewportRef}
      >
        <table
          className="table-fixed text-left text-sm"
          style={{
            minWidth: `${sum(TASK_MIN_WIDTHS) + ACTION_COLUMN_WIDTH}px`,
            width: `${tableWidth}px`,
          }}
        >
          <colgroup>
            {adaptiveWidths.map((width, index) => (
              <col key={index} style={{ width }} />
            ))}
            <col style={{ width: ACTION_COLUMN_WIDTH }} />
          </colgroup>
          <thead className="sticky top-0 z-10 bg-card">
            <tr>
              <ResizableHeader
                title="名称"
                field="name"
                sort={sort}
                loadingField={sortLoading}
                columnIndex={0}
                widths={adaptiveWidths}
                onSort={onSort}
                onResize={onResize}
                onAutoFit={onAutoFit}
              />
              <ResizableHeader
                title="统计"
                field="stats"
                sort={sort}
                loadingField={sortLoading}
                columnIndex={1}
                widths={adaptiveWidths}
                onSort={onSort}
                onResize={onResize}
                onAutoFit={onAutoFit}
              />
              <ResizableHeader
                title="标签"
                field="tags"
                sort={sort}
                loadingField={sortLoading}
                columnIndex={2}
                widths={adaptiveWidths}
                onSort={onSort}
                onResize={onResize}
                onAutoFit={onAutoFit}
              />
              <ResizableHeader
                title="语言"
                field="language"
                sort={sort}
                loadingField={sortLoading}
                columnIndex={3}
                canResize={false}
                widths={adaptiveWidths}
                onSort={onSort}
                onResize={onResize}
                onAutoFit={onAutoFit}
              />
              <ActionHeader />
            </tr>
          </thead>
          <ContextMenu>
            <ContextMenuTrigger asChild>
              <tbody>
                {loading ? (
                  <TableSkeletonRows columns={5} />
                ) : tasks.length === 0 ? (
                  <TableMessage colSpan={5}>暂无任务</TableMessage>
                ) : (
                  tasks.map((task) => (
                    <TaskRow
                      key={task.id}
                      task={task}
                      busy={busyId === task.id}
                      onContextTask={setContextTask}
                      onStart={onStart}
                      onResume={onResume}
                      onPause={onPause}
                      onRetranslate={onRetranslate}
                      onProofread={onProofread}
                      onOpenGlossary={onOpenGlossary}
                      onEditInfo={onEditInfo}
                      onOpenFolder={onOpenFolder}
                      onExport={onExport}
                      onDelete={onDelete}
                    />
                  ))
                )}
              </tbody>
            </ContextMenuTrigger>
            {contextTask && (
              <TaskContextMenuContent
                task={contextTask}
                busy={busyId === contextTask.id}
                onStart={onStart}
                onResume={onResume}
                onPause={onPause}
                onRetranslate={onRetranslate}
                onProofread={onProofread}
                onOpenGlossary={onOpenGlossary}
                onEditInfo={onEditInfo}
                onOpenFolder={onOpenFolder}
                onExport={onExport}
                onDelete={onDelete}
              />
            )}
          </ContextMenu>
        </table>
      </ScrollArea>
      <PaginationBar
        page={page}
        pageSize={pageSize}
        totalItems={totalItems}
        totalPages={totalPages}
        onPageChange={onPageChange}
        onPageSizeChange={onPageSizeChange}
      />
    </section>
  );
}

interface TaskRowProps extends TaskMenuProps {
  onContextTask: (task: TranslationTaskView) => void;
}

const TaskRow = memo(function TaskRow({
  task,
  busy,
  onContextTask,
  onStart,
  onResume,
  onPause,
  onRetranslate,
  onProofread,
  onOpenGlossary,
  onEditInfo,
  onOpenFolder,
  onExport,
  onDelete,
}: TaskRowProps) {
  return (
    <tr
      className="cursor-default border-b align-top transition-colors duration-100 hover:bg-accent/35 active:bg-accent/60"
      onContextMenu={() => onContextTask(task)}
    >
      <td className="h-11 min-w-0 px-3 py-2">
        <div className="truncate font-medium text-foreground" title={task.name}>
          {task.name}
        </div>
        <div className="mt-0.5 truncate text-xs text-muted-foreground">
          更新于 {unixTimeLabel(task.updatedAt)}
        </div>
      </td>
      <td className="h-11 min-w-0 px-3 py-2">
        <TaskStatsCell task={task} />
      </td>
      <td className="h-11 min-w-0 px-3 py-2">
        <TaskTags tags={task.tags} />
      </td>
      <td className="h-11 min-w-0 truncate px-3 py-2 text-sm">
        {displayLanguagePair(task.sourceLanguage, task.targetLanguage)}
      </td>
      <td className="h-11 px-2 py-1.5 text-center">
        <TaskActionDropdown
          task={task}
          busy={busy}
          onStart={onStart}
          onResume={onResume}
          onPause={onPause}
          onRetranslate={onRetranslate}
          onProofread={onProofread}
          onOpenGlossary={onOpenGlossary}
          onEditInfo={onEditInfo}
          onOpenFolder={onOpenFolder}
          onExport={onExport}
          onDelete={onDelete}
        />
      </td>
    </tr>
  );
}, (previous, next) => previous.busy === next.busy && sameTaskSignature(previous.task, next.task));

function ProgressStepPill({ step }: { step: ProgressStep }) {
  return (
    <Badge
      variant="outline"
      className="max-w-full rounded-[6px] border-border bg-muted/35 px-1.5 py-0 text-2xs font-normal text-muted-foreground"
      title={`${step.label} · ${formatPercent(step.percent)}`}
    >
      <span className="truncate">{step.label}</span>
    </Badge>
  );
}

function TaskTags({ tags }: { tags: string[] }) {
  if (tags.length === 0) return <span className="text-xs text-muted-foreground">无标签</span>;
  return (
    <div className="flex min-w-0 flex-wrap gap-x-1 gap-y-1.5">
      {tags.slice(0, 3).map((tag) => (
        <Badge
          key={tag}
          variant="secondary"
          className="max-w-24 rounded-full border-transparent bg-accent/45 text-accent-foreground dark:bg-accent/35"
        >
          <span className="truncate">{tag}</span>
        </Badge>
      ))}
      {tags.length > 3 && (
        <Badge
          variant="secondary"
          className="rounded-full border-transparent bg-accent/35 text-accent-foreground dark:bg-accent/30"
        >
          +{tags.length - 3}
        </Badge>
      )}
    </div>
  );
}

interface TaskMenuProps {
  task: TranslationTaskView;
  busy: boolean;
  onStart: (task: TranslationTaskView) => void;
  onResume: (task: TranslationTaskView) => void;
  onPause: (task: TranslationTaskView) => void;
  onRetranslate: (task: TranslationTaskView) => void;
  onProofread: (task: TranslationTaskView) => void;
  onOpenGlossary: (task: TranslationTaskView) => void;
  onEditInfo: (task: TranslationTaskView) => void;
  onOpenFolder: (task: TranslationTaskView) => void;
  onExport: (task: TranslationTaskView) => void;
  onDelete: (task: TranslationTaskView) => void;
}

function TaskContextMenuContent(props: TaskMenuProps) {
  return (
    <ContextMenuContent className="w-56">
      <TaskMenuItems kind="context" {...props} />
    </ContextMenuContent>
  );
}

function TaskActionDropdown(props: TaskMenuProps) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="mx-auto size-7 border-0 bg-transparent text-muted-foreground shadow-none hover:bg-[var(--button-ghost-hover-bg)] hover:text-foreground active:bg-[var(--button-ghost-pressed-bg)] active:text-foreground active:duration-[60ms] focus-visible:border-transparent focus-visible:ring-0 aria-expanded:bg-[var(--button-ghost-hover-bg)] aria-expanded:text-foreground"
          aria-label={`${props.task.name} 操作`}
          title="操作"
          onClick={(event) => event.stopPropagation()}
          onDoubleClick={(event) => event.stopPropagation()}
        >
          <MoreHorizontal className="size-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-56">
        <TaskMenuItems kind="dropdown" {...props} />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function TaskMenuItems({ kind, task, busy, onStart, onResume, onPause, onRetranslate, onProofread, onOpenGlossary, onEditInfo, onOpenFolder, onExport, onDelete }: TaskMenuProps & { kind: "context" | "dropdown" }) {
  const Item = kind === "context" ? ContextMenuItem : DropdownMenuItem;
  const Separator = kind === "context" ? ContextMenuSeparator : DropdownMenuSeparator;
  return (
    <>
      {task.status === "pending" && (
        <Item disabled={busy} onSelect={() => onStart(task)}>
          <Play className="size-3.5" />
          开始任务
        </Item>
      )}
      {task.status === "queued" && (
        <Item disabled={busy} onSelect={() => onPause(task)}>
          <Pause className="size-3.5" />
          暂停任务
        </Item>
      )}
      {task.status === "interrupted" && (
        <Item disabled={busy} onSelect={() => onResume(task)}>
          <RefreshCw className="size-3.5" />
          继续任务
        </Item>
      )}
      {task.status === "running" && (
        <Item disabled={busy} onSelect={() => onPause(task)}>
          <Pause className="size-3.5" />
          暂停任务
        </Item>
      )}
      {task.status === "interrupted-pending" && (
        <Item disabled>
          <Loader2 className="size-3.5 animate-spin" />
          正在中断
        </Item>
      )}
      {(task.status === "success" || task.status === "failed") && (
        <Item disabled={busy} onSelect={() => onRetranslate(task)}>
          <RotateCcw className="size-3.5" />
          {task.enableTranslation ? "重新翻译" : "重新建立术语表"}
        </Item>
      )}
      <Separator />
      <Item disabled={!task.enableTranslation} onSelect={() => onProofread(task)}>
        <FilePenLine className="size-3.5" />
        译后编辑和校对
      </Item>
      {task.glossaryId && (
        <Item onSelect={() => onOpenGlossary(task)}>
          <BookOpen className="size-3.5" />
          打开术语表
        </Item>
      )}
      <Item onSelect={() => onEditInfo(task)}>
        <Pencil className="size-3.5" />
        编辑任务信息
      </Item>
      <Separator />
      <Item onSelect={() => onOpenFolder(task)}>
        <FolderOpen className="size-3.5" />
        打开文件夹
      </Item>
      <Item disabled={!task.enableTranslation} onSelect={() => onExport(task)}>
        <Download className="size-3.5" />
        导出任务
      </Item>
      <Separator />
      <Item
        className="text-destructive focus:bg-destructive/10 focus:text-destructive"
        onSelect={() => onDelete(task)}
      >
        <Trash2 className="size-3.5" />
        删除任务
      </Item>
    </>
  );
}

interface ResizableHeaderProps {
  title: string;
  field: TaskSortField;
  sort: TaskSortState;
  loadingField: TaskSortField | null;
  columnIndex: number;
  canResize?: boolean;
  onSort: (field: TaskSortField) => void;
  onResize: (
    event: ReactPointerEvent<HTMLButtonElement>,
    index: number,
    widths: number[],
  ) => void;
  onAutoFit: (index: number) => void;
  widths: number[];
}

function ResizableHeader({
  title,
  field,
  sort,
  loadingField,
  columnIndex,
  canResize = true,
  onSort,
  onResize,
  onAutoFit,
  widths,
}: ResizableHeaderProps) {
  const active = sort.field === field;
  const label = active ? sortLabels[sort.mode] : sortLabels["created-desc"];
  const loading = loadingField === field;
  return (
    <th className="relative h-9 border-b px-0 text-xs font-medium whitespace-nowrap text-muted-foreground">
      <button
        type="button"
        className={cn(
          "group/header flex h-9 w-full min-w-0 items-center gap-2 px-3 text-left whitespace-nowrap transition-colors duration-150 hover:bg-accent/45 active:bg-accent/70",
          active && "text-foreground",
        )}
        onClick={() => onSort(field)}
      >
        <span className="shrink-0">{title}</span>
        <span
          className={cn(
            "ml-auto inline-flex min-w-0 shrink items-center gap-1 opacity-0 transition-opacity duration-150 group-hover/header:opacity-100 group-focus-visible/header:opacity-100",
            loading && "opacity-100",
          )}
        >
          {loading ? (
            <Loader2 className="size-3.5 shrink-0 animate-spin" />
          ) : (
            <>
              <ArrowUpDown className="size-3.5 shrink-0" />
              <span className="truncate">{label}</span>
            </>
          )}
        </span>
      </button>
      {canResize && (
        <button
          type="button"
          aria-label="调整列宽"
          className="absolute top-0 right-0 h-full w-2 cursor-col-resize touch-none border-r border-transparent transition-colors hover:border-primary/70"
          onPointerDown={(event) => onResize(event, columnIndex, widths)}
          onDoubleClick={() => onAutoFit(columnIndex)}
        />
      )}
    </th>
  );
}

function ActionHeader() {
  return (
    <th className="h-9 border-b px-2 text-center text-xs font-medium whitespace-nowrap text-muted-foreground">
      操作
    </th>
  );
}

interface PaginationBarProps {
  page: number;
  pageSize: number;
  totalItems: number;
  totalPages: number;
  onPageChange: (page: number) => void;
  onPageSizeChange: (pageSize: number) => void;
}

function PaginationBar({
  page,
  pageSize,
  totalItems,
  totalPages,
  onPageChange,
  onPageSizeChange,
}: PaginationBarProps) {
  const safeTotalPages = Math.max(1, totalPages);
  const isFirstPage = page <= 0;
  const isLastPage = page + 1 >= safeTotalPages;

  function changePageSize(value: string): void {
    const parsed = Number(value);
    if (!PAGE_SIZE_OPTIONS.some((option) => option === parsed)) return;
    onPageSizeChange(parsed);
    onPageChange(0);
  }

  return (
    <>
      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={isFirstPage}
        className="absolute bottom-4 left-4 z-20 h-9 border-border bg-card px-3 text-muted-foreground shadow-[0_6px_18px_rgba(0,0,0,0.14)] hover:bg-muted hover:text-foreground disabled:border-border/70 disabled:bg-card disabled:text-muted-foreground/55 disabled:opacity-100 dark:bg-card dark:shadow-[0_6px_20px_rgba(0,0,0,0.34)]"
        onClick={() => onPageChange(Math.max(0, page - 1))}
      >
        <ChevronLeft className="size-4" />
        上一页
      </Button>

      <div className="absolute bottom-4 left-1/2 z-20 flex h-9 -translate-x-1/2 items-center gap-3 rounded-[6px] border border-border bg-card px-3 text-xs text-muted-foreground shadow-[0_6px_18px_rgba(0,0,0,0.14)] dark:bg-card dark:shadow-[0_6px_20px_rgba(0,0,0,0.34)]">
        <div className="flex items-center gap-2">
          <span>每页显示</span>
          <Select value={String(pageSize)} onValueChange={changePageSize}>
            <SelectTrigger className="h-7 w-20 bg-background">
              <SelectValue />
            </SelectTrigger>
            <SelectContent side="top" align="center" viewportClassName="max-h-56">
              {PAGE_SIZE_OPTIONS.map((option) => (
                <SelectItem key={option} value={String(option)}>
                  {option} 条
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="min-w-20 text-center text-foreground">
          {totalItems === 0 ? 0 : page + 1} / {safeTotalPages} 页
        </div>
      </div>

      <Button
        type="button"
        variant="outline"
        size="sm"
        disabled={isLastPage}
        className="absolute right-4 bottom-4 z-20 h-9 border-border bg-card px-3 text-muted-foreground shadow-[0_6px_18px_rgba(0,0,0,0.14)] hover:bg-muted hover:text-foreground disabled:border-border/70 disabled:bg-card disabled:text-muted-foreground/55 disabled:opacity-100 dark:bg-card dark:shadow-[0_6px_20px_rgba(0,0,0,0.34)]"
        onClick={() => onPageChange(Math.min(safeTotalPages - 1, page + 1))}
      >
        下一页
        <ChevronRight className="size-4" />
      </Button>
    </>
  );
}

function TableSkeletonRows({ columns }: { columns: number }) {
  return (
    <>
      {Array.from({ length: 6 }).map((_, index) => (
        <tr key={index} className="border-b">
          <td colSpan={columns} className="h-11 px-3 py-2">
            <div className="grid grid-cols-[minmax(9rem,1fr)_minmax(10rem,0.8fr)_minmax(7rem,0.6fr)_minmax(8rem,0.7fr)] gap-3">
              <Skeleton className="h-4 w-full" />
              <Skeleton className="h-4 w-full" />
              <Skeleton className="h-4 w-full" />
              <Skeleton className="h-4 w-full" />
            </div>
          </td>
        </tr>
      ))}
    </>
  );
}

function TableMessage({
  colSpan,
  icon,
  children,
}: {
  colSpan: number;
  icon?: ReactNode;
  children: ReactNode;
}) {
  return (
    <tr>
      <td colSpan={colSpan} className="h-40 px-3 text-center text-sm text-muted-foreground">
        <span className="inline-flex items-center gap-2">
          {icon}
          {children}
        </span>
      </td>
    </tr>
  );
}

function TaskInfoDialog({
  state,
  saving,
  onOpenChange,
  onNameChange,
  onTagsChange,
  onSubmit,
}: {
  state: TaskInfoState | null;
  saving: boolean;
  onOpenChange: (open: boolean) => void;
  onNameChange: (value: string) => void;
  onTagsChange: (value: string) => void;
  onSubmit: () => void;
}) {
  return (
    <Dialog open={state !== null} onOpenChange={onOpenChange}>
      <DialogContent open={state !== null} className="max-w-md">
        <DialogHeader>
          <DialogTitle>编辑任务信息</DialogTitle>
        </DialogHeader>
        <DialogField>
          <Label>名称</Label>
          <Input value={state?.name ?? ""} onChange={(event) => onNameChange(event.target.value)} />
        </DialogField>
        <DialogField>
          <Label>标签</Label>
          <Input
            value={state?.tags ?? ""}
            onChange={(event) => onTagsChange(event.target.value)}
            placeholder="多个标签用逗号分隔"
          />
        </DialogField>
        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button type="button" disabled={saving} onClick={onSubmit}>
            {saving && <Loader2 className="size-4 animate-spin" />}
            保存
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function ExportDialog({
  state,
  exporting,
  onOpenChange,
  onChange,
  onSubmit,
}: {
  state: ExportState | null;
  exporting: boolean;
  onOpenChange: (open: boolean) => void;
  onChange: (state: ExportState | null) => void;
  onSubmit: () => void;
}) {
  const pdfSelected = state?.format === "pdf" || state?.format === "pdf-bilingual";
  function patch(next: Partial<ExportState>): void {
    onChange(state ? { ...state, ...next } : state);
  }
  return (
    <Dialog open={state !== null} onOpenChange={onOpenChange}>
      <DialogContent open={state !== null} className="max-w-lg">
        <DialogHeader>
          <DialogTitle>导出任务</DialogTitle>
          <DialogDescription>
            PDF 与 PDF 中英对照导出接口已保留，本轮先支持源格式导出。
          </DialogDescription>
        </DialogHeader>
        <DialogField>
          <Label>导出名称</Label>
          <Input value={state?.outputName ?? ""} onChange={(event) => patch({ outputName: event.target.value })} />
        </DialogField>
        <DialogField>
          <Label>格式</Label>
          <Select value={state?.format ?? "source"} onValueChange={(value) => patch({ format: value as TranslationTaskExportFormat })}>
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {state && (
                <SelectItem value="source">{sourceFormatLabel(state.task)}</SelectItem>
              )}
              <SelectItem value="pdf">PDF</SelectItem>
              <SelectItem value="pdf-bilingual">PDF (中英对照)</SelectItem>
            </SelectContent>
          </Select>
        </DialogField>
        {pdfSelected && (
          <div className="grid gap-3 sm:grid-cols-3">
            <DialogField>
              <Label>页面大小</Label>
              <Select value={state?.pageSize ?? "A4"} onValueChange={(value) => patch({ pageSize: value })}>
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="A4">A4</SelectItem>
                  <SelectItem value="Letter">Letter</SelectItem>
                </SelectContent>
              </Select>
            </DialogField>
            <DialogField>
              <Label>页边距</Label>
              <Select value={state?.margin ?? "normal"} onValueChange={(value) => patch({ margin: value })}>
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="narrow">窄</SelectItem>
                  <SelectItem value="normal">标准</SelectItem>
                  <SelectItem value="wide">宽</SelectItem>
                </SelectContent>
              </Select>
            </DialogField>
            <DialogField>
              <Label>缩放比例</Label>
              <Input
                type="number"
                min={0.5}
                max={2}
                step={0.1}
                value={state?.scale ?? 1}
                onChange={(event) => patch({ scale: Number(event.target.value) || 1 })}
              />
            </DialogField>
          </div>
        )}
        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button type="button" disabled={exporting} onClick={onSubmit}>
            {exporting && <Loader2 className="size-4 animate-spin" />}
            导出
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function ConfirmDialog({
  open,
  title,
  description,
  confirmText,
  destructive = true,
  onOpenChange,
  onConfirm,
}: {
  open: boolean;
  title: string;
  description: string;
  confirmText: string;
  destructive?: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent open={open} className="max-w-sm">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button type="button" variant={destructive ? "destructive" : "default"} onClick={onConfirm}>
            {confirmText}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
