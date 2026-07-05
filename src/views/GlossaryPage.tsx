import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import {
  ArrowLeft,
  ArrowUpDown,
  BookOpen,
  ChevronLeft,
  ChevronRight,
  Download,
  Edit3,
  FileJson,
  FileSpreadsheet,
  FolderOpen,
  Loader2,
  MoreHorizontal,
  Pencil,
  Plus,
  Search,
  Trash2,
  Upload,
} from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTriggerItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogField,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
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
  createGlossaryEntry,
  deleteGlossary,
  deleteGlossaryEntry,
  exportGlossary,
  getGlossaryEntries,
  importGlossary,
  listGlossaries,
  openGlossaryFolder,
  pickGlossaryFile,
  updateGlossary,
  updateGlossaryEntry,
} from "@/features/glossary/api";
import {
  displayLanguage,
  displayLanguagePair,
  GLOSSARY_LANGUAGES,
} from "@/features/glossary/languages";
import { LanguageCombobox } from "@/features/languages/LanguageCombobox";
import { normalizeLanguageCode } from "@/features/languages/languageOptions";
import type {
  GlossaryEntrySortField,
  GlossaryEntryView,
  GlossaryExportFormat,
  GlossarySortField,
  GlossaryView,
  SortMode,
} from "@/features/glossary/types";
import { SECONDARY_PAGE_FADE_UP_STYLE } from "@/lib/motion";
import { appSessionCache } from "@/lib/session-cache";
import { cn } from "@/lib/utils";

const ALL_VALUE = "__all__";
const DEFAULT_PAGE_SIZE = 20;
const PAGE_SIZE_OPTIONS = [10, 20, 50, 100] as const;
const LOADING_INDICATOR_DELAY_MS = 120;

const SORT_LABELS: Record<SortMode, string> = {
  "created-desc": "添加时间倒序",
  "created-asc": "添加时间正序",
  az: "A-Z 排序",
};

const LIST_HEADERS = ["名称", "数量", "标签", "语言"] as const;
const DETAIL_HEADERS = ["原始语言", "目标语言"] as const;
const LIST_MIN_WIDTHS = [132, 76, 104, 116];
const LIST_INITIAL_WIDTHS = [320, 84, 220, 260];
const LIST_MAX_WIDTHS = [680, 140, 480, 420];
const LIST_FLEX_COLUMNS = [0, 2, 3];
const DETAIL_MIN_WIDTHS = [128, 128];
const DETAIL_INITIAL_WIDTHS = [430, 430];
const DETAIL_MAX_WIDTHS = [760, 760];
const DETAIL_FLEX_COLUMNS = [0, 1];
const ACTION_COLUMN_WIDTH = 64;

interface ListSortState {
  field: GlossarySortField;
  mode: SortMode;
}

interface DetailSortState {
  field: GlossaryEntrySortField;
  mode: SortMode;
}

interface EntryEditorState {
  mode: "create" | "edit";
  entry: GlossaryEntryView | null;
}

interface GlossaryEditorState {
  glossary: GlossaryView;
}

function isTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

function getErrorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

function fileStem(path: string): string {
  return fileName(path).replace(/\.[^.]+$/, "");
}

function splitTags(value: string): string[] {
  return value
    .split(/[，,]/)
    .map((tag) => tag.trim())
    .filter(Boolean);
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

  const minTotal = sum(minWidths);
  const target = Math.max(Math.floor(containerWidth), minTotal);
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

    const updateWidth = (): void => {
      setWidth(element.clientWidth);
    };

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

export default function GlossaryPage() {
  const { pushToast } = useToast();
  const cachedIndex = appSessionCache.glossaryIndex.read();
  const cachedSelectedGlossary =
    cachedIndex?.filterSeed.find(
      (glossary) => glossary.id === cachedIndex.selectedGlossaryId,
    ) ?? null;
  const listRequestId = useRef(0);
  const entryRequestId = useRef(0);
  const listHasLoaded = useRef(Boolean(cachedIndex));
  const skipInitialGlossaryRefresh = useRef(Boolean(cachedIndex));
  const entryLoadedGlossaryId = useRef<string | null>(null);
  const [glossaries, setGlossaries] = useState<GlossaryView[]>(
    cachedIndex?.glossaries ?? [],
  );
  const [filterSeed, setFilterSeed] = useState<GlossaryView[]>(
    cachedIndex?.filterSeed ?? [],
  );
  const [selectedGlossary, setSelectedGlossary] = useState<GlossaryView | null>(
    cachedSelectedGlossary,
  );
  const [animateSecondaryView, setAnimateSecondaryView] = useState(false);
  const [search, setSearch] = useState(cachedIndex?.search ?? "");
  const [tagFilter, setTagFilter] = useState(cachedIndex?.tagFilter ?? ALL_VALUE);
  const [sourceFilter, setSourceFilter] = useState(cachedIndex?.sourceFilter ?? ALL_VALUE);
  const [targetFilter, setTargetFilter] = useState(cachedIndex?.targetFilter ?? ALL_VALUE);
  const [listSort, setListSort] = useState<ListSortState>({
    field: cachedIndex?.listSort.field ?? "name",
    mode: cachedIndex?.listSort.mode ?? "created-desc",
  });
  const [listLoading, setListLoading] = useState(!cachedIndex);
  const [listSortLoading, setListSortLoading] = useState<GlossarySortField | null>(null);
  const [listWidths, setListWidths] = useState(cachedIndex?.listWidths ?? LIST_INITIAL_WIDTHS);
  const [listPage, setListPage] = useState(cachedIndex?.listPage ?? 0);
  const [listPageSize, setListPageSize] = useState(
    cachedIndex?.listPageSize ?? DEFAULT_PAGE_SIZE,
  );

  const [entrySearch, setEntrySearch] = useState("");
  const [entrySort, setEntrySort] = useState<DetailSortState>({
    field: "src",
    mode: "created-desc",
  });
  const [entrySortLoading, setEntrySortLoading] = useState<GlossaryEntrySortField | null>(null);
  const [entryLoading, setEntryLoading] = useState(false);
  const [entryPage, setEntryPage] = useState({
    entries: [] as GlossaryEntryView[],
    total: 0,
    page: 0,
    pageSize: DEFAULT_PAGE_SIZE,
  });
  const [entryPageSize, setEntryPageSize] = useState(DEFAULT_PAGE_SIZE);
  const [detailWidths, setDetailWidths] = useState(DETAIL_INITIAL_WIDTHS);
  const [entryRefreshKey, setEntryRefreshKey] = useState(0);

  const [uploadOpen, setUploadOpen] = useState(false);
  const [uploading, setUploading] = useState(false);
  const [uploadFilePath, setUploadFilePath] = useState("");
  const [uploadName, setUploadName] = useState("");
  const [uploadSourceLanguage, setUploadSourceLanguage] = useState("en");
  const [uploadTargetLanguage, setUploadTargetLanguage] = useState("zh-CN");
  const [uploadTags, setUploadTags] = useState("");

  const [glossaryEditor, setGlossaryEditor] = useState<GlossaryEditorState | null>(null);
  const [glossaryEditName, setGlossaryEditName] = useState("");
  const [glossaryEditSourceLanguage, setGlossaryEditSourceLanguage] = useState("en");
  const [glossaryEditTargetLanguage, setGlossaryEditTargetLanguage] = useState("zh-CN");
  const [glossaryEditTags, setGlossaryEditTags] = useState("");
  const [glossarySaving, setGlossarySaving] = useState(false);

  const [entryEditor, setEntryEditor] = useState<EntryEditorState | null>(null);
  const [entrySrc, setEntrySrc] = useState("");
  const [entryDst, setEntryDst] = useState("");
  const [entrySaving, setEntrySaving] = useState(false);
  const [deleteGlossaryTarget, setDeleteGlossaryTarget] = useState<GlossaryView | null>(null);
  const [deleteEntryTarget, setDeleteEntryTarget] = useState<GlossaryEntryView | null>(null);

  const tagOptions = useMemo(() => {
    const tags = filterSeed.flatMap((glossary) => glossary.tags);
    return Array.from(new Set(tags)).sort((left, right) => left.localeCompare(right));
  }, [filterSeed]);

  const listTotalPages = Math.max(1, Math.ceil(glossaries.length / listPageSize));
  const pagedGlossaries = useMemo(() => {
    const start = listPage * listPageSize;
    return glossaries.slice(start, start + listPageSize);
  }, [glossaries, listPage, listPageSize]);
  const selectedEntryTotalPages = Math.max(1, Math.ceil(entryPage.total / entryPageSize));
  const selectedGlossaryId = selectedGlossary?.id ?? null;

  useEffect(() => {
    if (!listHasLoaded.current) return;
    appSessionCache.glossaryIndex.set({
      glossaries,
      filterSeed,
      selectedGlossaryId,
      search,
      tagFilter,
      sourceFilter,
      targetFilter,
      listSort,
      listPage,
      listPageSize,
      listWidths,
    });
  }, [
    filterSeed,
    glossaries,
    listPage,
    listPageSize,
    listSort,
    listWidths,
    search,
    selectedGlossaryId,
    sourceFilter,
    tagFilter,
    targetFilter,
  ]);

  useEffect(() => {
    setListPage(0);
  }, [search, tagFilter, sourceFilter, targetFilter, listSort, listPageSize]);

  useEffect(() => {
    setListPage((current) => Math.min(current, listTotalPages - 1));
  }, [listTotalPages]);

  const refreshGlossaries = useCallback(async (): Promise<void> => {
    const requestId = listRequestId.current + 1;
    listRequestId.current = requestId;
    const showLoadingImmediately = !listHasLoaded.current;
    let loadingTimer: number | null = null;

    if (showLoadingImmediately) {
      setListLoading(true);
    } else {
      setListLoading(false);
      loadingTimer = window.setTimeout(() => {
        if (listRequestId.current === requestId) {
          setListLoading(true);
        }
      }, LOADING_INDICATOR_DELAY_MS);
    }

    try {
      if (!isTauriRuntime()) {
        if (listRequestId.current !== requestId) return;
        setGlossaries([]);
        setFilterSeed([]);
        return;
      }
      const query = {
        search: search.trim() || null,
        tag: tagFilter === ALL_VALUE ? null : tagFilter,
        sourceLanguage: sourceFilter === ALL_VALUE ? null : sourceFilter,
        targetLanguage: targetFilter === ALL_VALUE ? null : targetFilter,
        sort: listSort,
      };
      const [filtered, all] = await Promise.all([
        listGlossaries(query),
        listGlossaries(null),
      ]);
      if (listRequestId.current !== requestId) return;
      setGlossaries(filtered);
      setFilterSeed(all);
      setSelectedGlossary((current) => {
        if (!current) return current;
        return all.find((glossary) => glossary.id === current.id) ?? null;
      });
    } catch (error) {
      if (listRequestId.current === requestId) {
        pushToast(getErrorMessage(error), "error");
      }
    } finally {
      if (loadingTimer !== null) {
        window.clearTimeout(loadingTimer);
      }
      if (listRequestId.current === requestId) {
        listHasLoaded.current = true;
        setListLoading(false);
        setListSortLoading(null);
      }
    }
  }, [listSort, pushToast, search, sourceFilter, tagFilter, targetFilter]);

  useEffect(() => {
    if (skipInitialGlossaryRefresh.current) {
      skipInitialGlossaryRefresh.current = false;
      return;
    }
    void refreshGlossaries();
  }, [refreshGlossaries]);

  useEffect(() => {
    setEntryPage((current) => ({ ...current, page: 0 }));
  }, [entrySearch, entrySort, selectedGlossaryId, entryPageSize]);

  useEffect(() => {
    setEntryPage((current) => ({
      ...current,
      page: Math.min(current.page, selectedEntryTotalPages - 1),
    }));
  }, [selectedEntryTotalPages]);

  useEffect(() => {
    async function loadEntries(): Promise<void> {
      const requestId = entryRequestId.current + 1;
      entryRequestId.current = requestId;

      if (!selectedGlossaryId || !isTauriRuntime()) {
        setEntryPage({ entries: [], total: 0, page: 0, pageSize: entryPageSize });
        setEntryLoading(false);
        setEntrySortLoading(null);
        entryLoadedGlossaryId.current = null;
        return;
      }

      const showLoadingImmediately = entryLoadedGlossaryId.current !== selectedGlossaryId;
      let loadingTimer: number | null = null;

      if (showLoadingImmediately) {
        setEntryLoading(true);
      } else {
        setEntryLoading(false);
        loadingTimer = window.setTimeout(() => {
          if (entryRequestId.current === requestId) {
            setEntryLoading(true);
          }
        }, LOADING_INDICATOR_DELAY_MS);
      }

      try {
        const page = await getGlossaryEntries({
          id: selectedGlossaryId,
          page: entryPage.page,
          pageSize: entryPageSize,
          search: entrySearch.trim() || null,
          sort: entrySort,
        });
        if (entryRequestId.current !== requestId) return;
        setEntryPage({
          entries: page.entries,
          total: page.total,
          page: page.page,
          pageSize: page.pageSize,
        });
      } catch (error) {
        if (entryRequestId.current === requestId) {
          pushToast(getErrorMessage(error), "error");
        }
      } finally {
        if (loadingTimer !== null) {
          window.clearTimeout(loadingTimer);
        }
        if (entryRequestId.current === requestId) {
          entryLoadedGlossaryId.current = selectedGlossaryId;
          setEntryLoading(false);
          setEntrySortLoading(null);
        }
      }
    }
    void loadEntries();
  }, [
    entryPage.page,
    entryPageSize,
    entryRefreshKey,
    entrySearch,
    entrySort,
    pushToast,
    selectedGlossaryId,
  ]);

  function updateListSort(field: GlossarySortField): void {
    setListSortLoading(field);
    setListSort((current) => ({
      field,
      mode: current.field === field ? nextSortMode(current.mode) : "az",
    }));
  }

  function updateEntrySort(field: GlossaryEntrySortField): void {
    setEntrySortLoading(field);
    setEntrySort((current) => ({
      field,
      mode: current.field === field ? nextSortMode(current.mode) : "az",
    }));
  }

  function autoFitListColumn(columnIndex: number): void {
    const values = pagedGlossaries.map((glossary) => {
      if (columnIndex === 0) return glossary.name;
      if (columnIndex === 1) return `${glossary.entryCount} 条`;
      if (columnIndex === 2) return glossary.tags.join("，") || "无标签";
      if (columnIndex === 3) {
        return displayLanguagePair(glossary.sourceLanguage, glossary.targetLanguage);
      }
      return "";
    });
    setListWidths((current) => current.map((width, index) => (
      index === columnIndex
        ? autoWidth([LIST_HEADERS[columnIndex], ...values], LIST_MIN_WIDTHS[columnIndex], LIST_MAX_WIDTHS[columnIndex])
        : width
    )));
  }

  function autoFitDetailColumn(columnIndex: number): void {
    const values = entryPage.entries.map((entry) => {
      if (columnIndex === 0) return entry.src;
      if (columnIndex === 1) return entry.dst;
      return "";
    });
    setDetailWidths((current) => current.map((width, index) => (
      index === columnIndex
        ? autoWidth([DETAIL_HEADERS[columnIndex], ...values], DETAIL_MIN_WIDTHS[columnIndex], DETAIL_MAX_WIDTHS[columnIndex])
        : width
    )));
  }

  async function chooseUploadFile(): Promise<void> {
    try {
      const path = await pickGlossaryFile();
      if (!path) return;
      setUploadFilePath(path);
      if (!uploadName.trim()) setUploadName(fileStem(path));
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    }
  }

  async function submitUpload(): Promise<void> {
    if (!uploadFilePath || !uploadName.trim()) {
      pushToast("请选择文件并填写术语表名称", "warning");
      return;
    }
    setUploading(true);
    try {
      await importGlossary({
        filePath: uploadFilePath,
        name: uploadName,
        sourceLanguage: uploadSourceLanguage,
        targetLanguage: uploadTargetLanguage,
        tags: splitTags(uploadTags),
      });
      pushToast("术语表已导入", "success");
      setUploadOpen(false);
      setUploadFilePath("");
      setUploadName("");
      setUploadTags("");
      await refreshGlossaries();
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    } finally {
      setUploading(false);
    }
  }

  function openGlossaryEditor(glossary: GlossaryView): void {
    setGlossaryEditor({ glossary });
    setGlossaryEditName(glossary.name);
    setGlossaryEditSourceLanguage(glossary.sourceLanguage);
    setGlossaryEditTargetLanguage(glossary.targetLanguage);
    setGlossaryEditTags(glossary.tags.join("，"));
  }

  async function submitGlossaryEditor(): Promise<void> {
    if (!glossaryEditor) return;
    if (!glossaryEditName.trim()) {
      pushToast("术语表名称不能为空", "warning");
      return;
    }
    setGlossarySaving(true);
    try {
      const updated = await updateGlossary({
        glossaryId: glossaryEditor.glossary.id,
        name: glossaryEditName,
        sourceLanguage: glossaryEditSourceLanguage,
        targetLanguage: glossaryEditTargetLanguage,
        tags: splitTags(glossaryEditTags),
      });
      setSelectedGlossary((current) => (current?.id === updated.id ? updated : current));
      setGlossaryEditor(null);
      pushToast("术语表信息已更新", "success");
      await refreshGlossaries();
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    } finally {
      setGlossarySaving(false);
    }
  }

  function openEntryEditor(entry: GlossaryEntryView | null): void {
    setEntryEditor({ mode: entry ? "edit" : "create", entry });
    setEntrySrc(entry?.src ?? "");
    setEntryDst(entry?.dst ?? "");
  }

  async function submitEntryEditor(): Promise<void> {
    if (!selectedGlossary) return;
    if (!entrySrc.trim() || !entryDst.trim()) {
      pushToast("原始语言和目标语言都不能为空", "warning");
      return;
    }
    setEntrySaving(true);
    try {
      if (entryEditor?.mode === "edit" && entryEditor.entry) {
        await updateGlossaryEntry({
          glossaryId: selectedGlossary.id,
          entryId: entryEditor.entry.id,
          src: entrySrc,
          dst: entryDst,
        });
        pushToast("术语已更新", "success");
      } else {
        await createGlossaryEntry({
          glossaryId: selectedGlossary.id,
          src: entrySrc,
          dst: entryDst,
        });
        pushToast("术语已添加", "success");
      }
      setEntryEditor(null);
      setEntryRefreshKey((current) => current + 1);
      await refreshGlossaries();
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    } finally {
      setEntrySaving(false);
    }
  }

  async function confirmDeleteGlossary(): Promise<void> {
    if (!deleteGlossaryTarget) return;
    try {
      await deleteGlossary(deleteGlossaryTarget.id);
      if (selectedGlossary?.id === deleteGlossaryTarget.id) setSelectedGlossary(null);
      setDeleteGlossaryTarget(null);
      pushToast("术语表已删除", "success");
      await refreshGlossaries();
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    }
  }

  async function confirmDeleteEntry(): Promise<void> {
    if (!selectedGlossary || !deleteEntryTarget) return;
    try {
      await deleteGlossaryEntry({
        glossaryId: selectedGlossary.id,
        entryId: deleteEntryTarget.id,
      });
      setDeleteEntryTarget(null);
      pushToast("术语已删除", "success");
      setEntryRefreshKey((current) => current + 1);
      await refreshGlossaries();
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    }
  }

  async function runExport(
    glossary: GlossaryView,
    format: GlossaryExportFormat,
  ): Promise<void> {
    try {
      await exportGlossary({ id: glossary.id, format });
      pushToast("术语表已导出", "success");
    } catch (error) {
      if (getErrorMessage(error) !== "Export cancelled") {
        pushToast(getErrorMessage(error), "error");
      }
    }
  }

  function openGlossaryDetail(glossary: GlossaryView): void {
    setAnimateSecondaryView(true);
    setSelectedGlossary(glossary);
  }

  function closeGlossaryDetail(): void {
    setAnimateSecondaryView(true);
    setSelectedGlossary(null);
  }

  return (
    <main className="flex min-w-0 flex-1 flex-col overflow-hidden p-3">
        {!selectedGlossary ? (
          <GlossaryListView
            key="glossary-index"
            animateEnter={animateSecondaryView}
            glossaries={pagedGlossaries}
            filterSeed={filterSeed}
            totalCount={glossaries.length}
            page={listPage}
            pageSize={listPageSize}
            totalPages={listTotalPages}
            tagOptions={tagOptions}
            search={search}
            tagFilter={tagFilter}
            sourceFilter={sourceFilter}
            targetFilter={targetFilter}
            listSort={listSort}
            listLoading={listLoading}
            listSortLoading={listSortLoading}
            widths={listWidths}
            onSearchChange={setSearch}
            onTagFilterChange={setTagFilter}
            onSourceFilterChange={setSourceFilter}
            onTargetFilterChange={setTargetFilter}
            onUpload={() => setUploadOpen(true)}
            onOpen={openGlossaryDetail}
            onEditInfo={openGlossaryEditor}
            onOpenFolder={(glossary) => {
              void openGlossaryFolder(glossary.id).catch((error: unknown) => {
                pushToast(getErrorMessage(error), "error");
              });
            }}
            onExport={(glossary, format) => void runExport(glossary, format)}
            onDelete={setDeleteGlossaryTarget}
            onSort={updateListSort}
            onPageChange={setListPage}
            onPageSizeChange={setListPageSize}
            onResize={(event, index, renderedWidths) => startResize(event, index, renderedWidths, LIST_MIN_WIDTHS, setListWidths)}
            onAutoFit={autoFitListColumn}
          />
        ) : (
          <GlossaryDetailView
            key={selectedGlossary.id}
            glossary={selectedGlossary}
            entryPage={entryPage}
            entrySearch={entrySearch}
            entrySort={entrySort}
            entryLoading={entryLoading}
            entrySortLoading={entrySortLoading}
            widths={detailWidths}
            totalPages={selectedEntryTotalPages}
            animateEnter={animateSecondaryView}
            onBack={closeGlossaryDetail}
            onSearchChange={setEntrySearch}
            onAdd={() => openEntryEditor(null)}
            onEdit={openEntryEditor}
            onDelete={setDeleteEntryTarget}
            onSort={updateEntrySort}
            onPageChange={(page) => setEntryPage((current) => ({ ...current, page }))}
            onPageSizeChange={setEntryPageSize}
            onResize={(event, index, renderedWidths) => startResize(event, index, renderedWidths, DETAIL_MIN_WIDTHS, setDetailWidths)}
            onAutoFit={autoFitDetailColumn}
          />
        )}

        <UploadGlossaryDialog
          open={uploadOpen}
          filePath={uploadFilePath}
          name={uploadName}
          sourceLanguage={uploadSourceLanguage}
          targetLanguage={uploadTargetLanguage}
          tags={uploadTags}
          uploading={uploading}
          onOpenChange={setUploadOpen}
          onChooseFile={() => void chooseUploadFile()}
          onNameChange={setUploadName}
          onSourceLanguageChange={setUploadSourceLanguage}
          onTargetLanguageChange={setUploadTargetLanguage}
          onTagsChange={setUploadTags}
          onSubmit={() => void submitUpload()}
        />

        <GlossaryInfoDialog
          open={glossaryEditor !== null}
          name={glossaryEditName}
          sourceLanguage={glossaryEditSourceLanguage}
          targetLanguage={glossaryEditTargetLanguage}
          tags={glossaryEditTags}
          saving={glossarySaving}
          onOpenChange={(open) => {
            if (!open) setGlossaryEditor(null);
          }}
          onNameChange={setGlossaryEditName}
          onSourceLanguageChange={setGlossaryEditSourceLanguage}
          onTargetLanguageChange={setGlossaryEditTargetLanguage}
          onTagsChange={setGlossaryEditTags}
          onSubmit={() => void submitGlossaryEditor()}
        />

        <EntryEditorDialog
          open={entryEditor !== null}
          mode={entryEditor?.mode ?? "create"}
          src={entrySrc}
          dst={entryDst}
          saving={entrySaving}
          onOpenChange={(open) => {
            if (!open) setEntryEditor(null);
          }}
          onSrcChange={setEntrySrc}
          onDstChange={setEntryDst}
          onSubmit={() => void submitEntryEditor()}
        />

        <ConfirmDialog
          open={deleteGlossaryTarget !== null}
          title="删除术语表"
          description={`确认删除“${deleteGlossaryTarget?.name ?? ""}”？对应的 .ing 文件也会同步删除。`}
          confirmText="删除"
          onOpenChange={(open) => {
            if (!open) setDeleteGlossaryTarget(null);
          }}
          onConfirm={() => void confirmDeleteGlossary()}
        />

        <ConfirmDialog
          open={deleteEntryTarget !== null}
          title="删除术语"
          description={`确认删除“${deleteEntryTarget?.src ?? ""}”？`}
          confirmText="删除"
          onOpenChange={(open) => {
            if (!open) setDeleteEntryTarget(null);
          }}
          onConfirm={() => void confirmDeleteEntry()}
        />
      </main>
  );
}

interface GlossaryListViewProps {
  animateEnter: boolean;
  glossaries: GlossaryView[];
  filterSeed: GlossaryView[];
  totalCount: number;
  page: number;
  pageSize: number;
  totalPages: number;
  tagOptions: string[];
  search: string;
  tagFilter: string;
  sourceFilter: string;
  targetFilter: string;
  listSort: ListSortState;
  listLoading: boolean;
  listSortLoading: GlossarySortField | null;
  widths: number[];
  onSearchChange: (value: string) => void;
  onTagFilterChange: (value: string) => void;
  onSourceFilterChange: (value: string) => void;
  onTargetFilterChange: (value: string) => void;
  onUpload: () => void;
  onOpen: (glossary: GlossaryView) => void;
  onEditInfo: (glossary: GlossaryView) => void;
  onOpenFolder: (glossary: GlossaryView) => void;
  onExport: (glossary: GlossaryView, format: GlossaryExportFormat) => void;
  onDelete: (glossary: GlossaryView) => void;
  onSort: (field: GlossarySortField) => void;
  onPageChange: (page: number) => void;
  onPageSizeChange: (pageSize: number) => void;
  onResize: (
    event: ReactPointerEvent<HTMLButtonElement>,
    index: number,
    widths: number[],
  ) => void;
  onAutoFit: (index: number) => void;
}

function GlossaryListView({
  animateEnter,
  glossaries,
  filterSeed,
  totalCount,
  page,
  pageSize,
  totalPages,
  tagOptions,
  search,
  tagFilter,
  sourceFilter,
  targetFilter,
  listSort,
  listLoading,
  listSortLoading,
  widths,
  onSearchChange,
  onTagFilterChange,
  onSourceFilterChange,
  onTargetFilterChange,
  onUpload,
  onOpen,
  onEditInfo,
  onOpenFolder,
  onExport,
  onDelete,
  onSort,
  onPageChange,
  onPageSizeChange,
  onResize,
  onAutoFit,
}: GlossaryListViewProps) {
  const languageOptions = useMemo(() => {
    const values = new Map<string, string>();
    filterSeed.forEach((glossary) => {
      const sourceKey = normalizeLanguageCode(glossary.sourceLanguage) ?? glossary.sourceLanguage.toLowerCase();
      const targetKey = normalizeLanguageCode(glossary.targetLanguage) ?? glossary.targetLanguage.toLowerCase();
      values.set(sourceKey, glossary.sourceLanguage);
      values.set(targetKey, glossary.targetLanguage);
    });
    GLOSSARY_LANGUAGES.forEach((language) => values.set(language.code, language.code));
    return Array.from(values.values()).sort((left, right) => (
      displayLanguage(left).localeCompare(displayLanguage(right))
    ));
  }, [filterSeed]);

  return (
    <div
      style={animateEnter ? SECONDARY_PAGE_FADE_UP_STYLE : undefined}
      className={cn(
        "flex min-h-0 flex-1 flex-col",
        animateEnter && "app-fade-up-enter",
      )}
    >
      <header className="mb-3 shrink-0">
        <div className="flex items-center gap-2">
          <BookOpen className="size-5 text-primary" />
          <h1 className="text-xl font-medium tracking-tight">术语表</h1>
          <Badge variant="secondary" className="ml-1 rounded-[6px]">
            {filterSeed.length} 个
          </Badge>
          <Button type="button" className="ml-auto" onClick={onUpload}>
            <Upload className="size-4" />
            上传术语表
          </Button>
        </div>
        <p className="mt-0.5 text-xs text-muted-foreground">
          管理上传或后续自动建立的术语表文件
        </p>
      </header>

      <div className="mb-3 grid shrink-0 gap-2 lg:grid-cols-[minmax(16rem,1fr)_11rem_11rem_11rem]">
        <div className="relative">
          <Search className="pointer-events-none absolute top-1/2 left-2 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            className="pl-8"
            value={search}
            onChange={(event) => onSearchChange(event.target.value)}
            placeholder="检索名称、标签或语言"
          />
        </div>
        <Select value={tagFilter} onValueChange={onTagFilterChange}>
          <SelectTrigger>
            <SelectValue placeholder="标签" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={ALL_VALUE}>全部标签</SelectItem>
            {tagOptions.map((tag) => (
              <SelectItem key={tag} value={tag}>
                {tag}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <LanguageCombobox
          value={sourceFilter}
          allValue={ALL_VALUE}
          allLabel="全部原始语言"
          onValueChange={onSourceFilterChange}
          placeholder="原始语言"
          searchPlaceholder="搜索原始语言"
        />
        <LanguageCombobox
          value={targetFilter}
          allValue={ALL_VALUE}
          allLabel="全部目标语言"
          onValueChange={onTargetFilterChange}
          placeholder="目标语言"
          searchPlaceholder="搜索目标语言"
        />
      </div>

      <GlossaryListTable
        glossaries={glossaries}
        page={page}
        pageSize={pageSize}
        totalCount={totalCount}
        totalPages={totalPages}
        search={search}
        tagFilter={tagFilter}
        sourceFilter={sourceFilter}
        targetFilter={targetFilter}
        listSort={listSort}
        listLoading={listLoading}
        listSortLoading={listSortLoading}
        widths={widths}
        onOpen={onOpen}
        onEditInfo={onEditInfo}
        onOpenFolder={onOpenFolder}
        onExport={onExport}
        onDelete={onDelete}
        onSort={onSort}
        onPageChange={onPageChange}
        onPageSizeChange={onPageSizeChange}
        onResize={onResize}
        onAutoFit={onAutoFit}
      />
    </div>
  );
}

interface GlossaryListTableProps {
  glossaries: GlossaryView[];
  page: number;
  pageSize: number;
  totalCount: number;
  totalPages: number;
  search: string;
  tagFilter: string;
  sourceFilter: string;
  targetFilter: string;
  listSort: ListSortState;
  listLoading: boolean;
  listSortLoading: GlossarySortField | null;
  widths: number[];
  onOpen: (glossary: GlossaryView) => void;
  onEditInfo: (glossary: GlossaryView) => void;
  onOpenFolder: (glossary: GlossaryView) => void;
  onExport: (glossary: GlossaryView, format: GlossaryExportFormat) => void;
  onDelete: (glossary: GlossaryView) => void;
  onSort: (field: GlossarySortField) => void;
  onPageChange: (page: number) => void;
  onPageSizeChange: (pageSize: number) => void;
  onResize: (
    event: ReactPointerEvent<HTMLButtonElement>,
    index: number,
    widths: number[],
  ) => void;
  onAutoFit: (index: number) => void;
}

function GlossaryListTable({
  glossaries,
  page,
  pageSize,
  totalCount,
  totalPages,
  search,
  tagFilter,
  sourceFilter,
  targetFilter,
  listSort,
  listLoading,
  listSortLoading,
  widths,
  onOpen,
  onEditInfo,
  onOpenFolder,
  onExport,
  onDelete,
  onSort,
  onPageChange,
  onPageSizeChange,
  onResize,
  onAutoFit,
}: GlossaryListTableProps) {
  const [tableViewportRef, adaptiveWidths, tableViewportWidth] = useAdaptiveColumnWidths<HTMLDivElement>(
    widths,
    LIST_MIN_WIDTHS,
    LIST_FLEX_COLUMNS,
    ACTION_COLUMN_WIDTH,
  );
  const tableWidth = sum(adaptiveWidths) + ACTION_COLUMN_WIDTH;
  const tableNeedsHorizontalScroll = tableWidth > tableViewportWidth + 1;

  return (
      <section className="relative min-h-0 flex-1 overflow-hidden rounded-[6px] border bg-card">
        <div
          ref={tableViewportRef}
          className={cn(
            "h-full overflow-y-auto pb-20",
            "scrollbar-subtle overscroll-contain",
            tableNeedsHorizontalScroll ? "overflow-x-auto" : "overflow-x-hidden",
          )}
        >
          <table
            className="table-fixed text-left text-sm"
            style={{
              minWidth: `${sum(LIST_MIN_WIDTHS) + ACTION_COLUMN_WIDTH}px`,
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
                  sort={listSort}
                  loadingField={listSortLoading}
                  columnIndex={0}
                  canResize
                  widths={adaptiveWidths}
                  onSort={onSort}
                  onResize={onResize}
                  onAutoFit={onAutoFit}
                />
                <ResizableHeader
                  title="数量"
                  columnIndex={1}
                  canResize
                  widths={adaptiveWidths}
                  onResize={onResize}
                  onAutoFit={onAutoFit}
                />
                <ResizableHeader
                  title="标签"
                  field="tags"
                  sort={listSort}
                  loadingField={listSortLoading}
                  columnIndex={2}
                  canResize
                  widths={adaptiveWidths}
                  onSort={onSort}
                  onResize={onResize}
                  onAutoFit={onAutoFit}
                />
                <ResizableHeader
                  title="语言"
                  field="language"
                  sort={listSort}
                  loadingField={listSortLoading}
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
            <tbody>
              {listLoading ? (
                <TableSkeletonRows columns={5} />
              ) : listLoading ? (
                <TableMessage colSpan={5} icon={<Loader2 className="size-4 animate-spin" />}>
                  正在读取术语表索引
                </TableMessage>
              ) : glossaries.length === 0 ? (
                <TableMessage colSpan={5}>暂无术语表</TableMessage>
              ) : (
                glossaries.map((glossary) => (
                  <ContextMenu key={glossary.id}>
                    <ContextMenuTrigger asChild>
                      <tr
                        className="cursor-pointer border-b align-top transition-colors duration-100 hover:bg-accent/35 active:bg-accent/60"
                        onDoubleClick={() => onOpen(glossary)}
                      >
                        <td className="h-10 min-w-0 px-3 py-2">
                          <div className="truncate font-medium text-foreground" title={glossary.name}>
                            {glossary.name}
                          </div>
                        </td>
                        <td className="h-10 whitespace-nowrap px-3 py-2 text-sm text-muted-foreground">
                          {glossary.entryCount} 条
                        </td>
                        <td className="h-10 min-w-0 px-3 py-2">
                          <TagList tags={glossary.tags} />
                        </td>
                        <td className="h-10 min-w-0 truncate px-3 py-2 text-sm">
                          {displayLanguagePair(glossary.sourceLanguage, glossary.targetLanguage)}
                        </td>
                        <td className="h-10 px-2 py-1.5 text-center">
                          <GlossaryListActionDropdown
                            glossary={glossary}
                            onOpen={onOpen}
                            onEditInfo={onEditInfo}
                            onOpenFolder={onOpenFolder}
                            onExport={onExport}
                            onDelete={onDelete}
                          />
                        </td>
                      </tr>
                    </ContextMenuTrigger>
                    <GlossaryListContextMenuContent
                      glossary={glossary}
                      onOpen={onOpen}
                      onEditInfo={onEditInfo}
                      onOpenFolder={onOpenFolder}
                      onExport={onExport}
                      onDelete={onDelete}
                    />
                  </ContextMenu>
                ))
              )}
            </tbody>
          </table>
        </div>
        <PaginationBar
          page={page}
          pageSize={pageSize}
          totalItems={totalCount}
          totalPages={totalPages}
          onPageChange={onPageChange}
          onPageSizeChange={onPageSizeChange}
        />
      </section>
  );
}

interface GlossaryListActionMenuProps {
  glossary: GlossaryView;
  onOpen: (glossary: GlossaryView) => void;
  onEditInfo: (glossary: GlossaryView) => void;
  onOpenFolder: (glossary: GlossaryView) => void;
  onExport: (glossary: GlossaryView, format: GlossaryExportFormat) => void;
  onDelete: (glossary: GlossaryView) => void;
}

function GlossaryListContextMenuContent({
  glossary,
  onOpen,
  onEditInfo,
  onOpenFolder,
  onExport,
  onDelete,
}: GlossaryListActionMenuProps) {
  return (
    <ContextMenuContent className="w-56">
      <ContextMenuItem onSelect={() => onOpen(glossary)}>
        <BookOpen className="size-3.5" />
        打开术语表
      </ContextMenuItem>
      <ContextMenuItem onSelect={() => onEditInfo(glossary)}>
        <Pencil className="size-3.5" />
        编辑术语表信息
      </ContextMenuItem>
      <ContextMenuSeparator />
      <ContextMenuItem onSelect={() => onOpenFolder(glossary)}>
        <FolderOpen className="size-3.5" />
        打开文件夹
      </ContextMenuItem>
      <ContextMenuSub>
        <ContextMenuSubTriggerItem>
          <Download className="size-3.5" />
          导出术语表
        </ContextMenuSubTriggerItem>
        <ContextMenuSubContent className="w-40">
          <ContextMenuItem onSelect={() => onExport(glossary, "csv")}>
            <FileSpreadsheet className="size-3.5" />
            导出 CSV
          </ContextMenuItem>
          <ContextMenuItem onSelect={() => onExport(glossary, "json")}>
            <FileJson className="size-3.5" />
            导出 JSON
          </ContextMenuItem>
        </ContextMenuSubContent>
      </ContextMenuSub>
      <ContextMenuSeparator />
      <ContextMenuItem
        className="text-destructive focus:bg-destructive/10 focus:text-destructive"
        onSelect={() => onDelete(glossary)}
      >
        <Trash2 className="size-3.5" />
        删除术语表
      </ContextMenuItem>
    </ContextMenuContent>
  );
}

function GlossaryListActionDropdown({
  glossary,
  onOpen,
  onEditInfo,
  onOpenFolder,
  onExport,
  onDelete,
}: GlossaryListActionMenuProps) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="mx-auto size-7 border-0 bg-transparent text-muted-foreground shadow-none hover:bg-[var(--button-ghost-hover-bg)] hover:text-foreground active:bg-[var(--button-ghost-pressed-bg)] active:text-foreground active:duration-[60ms] focus-visible:border-transparent focus-visible:ring-0 aria-expanded:bg-[var(--button-ghost-hover-bg)] aria-expanded:text-foreground"
          aria-label={`${glossary.name} 操作`}
          title="操作"
          onClick={(event) => event.stopPropagation()}
          onDoubleClick={(event) => event.stopPropagation()}
        >
          <MoreHorizontal className="size-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-56">
        <DropdownMenuItem onSelect={() => onOpen(glossary)}>
          <BookOpen className="size-3.5" />
          打开术语表
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => onEditInfo(glossary)}>
          <Pencil className="size-3.5" />
          编辑术语表信息
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => onOpenFolder(glossary)}>
          <FolderOpen className="size-3.5" />
          打开文件夹
        </DropdownMenuItem>
        <DropdownMenuSub>
          <DropdownMenuSubTriggerItem>
            <Download className="size-3.5" />
            导出术语表
          </DropdownMenuSubTriggerItem>
          <DropdownMenuSubContent className="w-40">
            <DropdownMenuItem onSelect={() => onExport(glossary, "csv")}>
              <FileSpreadsheet className="size-3.5" />
              导出 CSV
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => onExport(glossary, "json")}>
              <FileJson className="size-3.5" />
              导出 JSON
            </DropdownMenuItem>
          </DropdownMenuSubContent>
        </DropdownMenuSub>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          className="text-destructive focus:bg-destructive/10 focus:text-destructive"
          onSelect={() => onDelete(glossary)}
        >
          <Trash2 className="size-3.5" />
          删除术语表
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

interface GlossaryDetailViewProps {
  animateEnter: boolean;
  glossary: GlossaryView;
  entryPage: {
    entries: GlossaryEntryView[];
    total: number;
    page: number;
    pageSize: number;
  };
  entrySearch: string;
  entrySort: DetailSortState;
  entryLoading: boolean;
  entrySortLoading: GlossaryEntrySortField | null;
  widths: number[];
  totalPages: number;
  onBack: () => void;
  onSearchChange: (value: string) => void;
  onAdd: () => void;
  onEdit: (entry: GlossaryEntryView) => void;
  onDelete: (entry: GlossaryEntryView) => void;
  onSort: (field: GlossaryEntrySortField) => void;
  onPageChange: (page: number) => void;
  onPageSizeChange: (pageSize: number) => void;
  onResize: (
    event: ReactPointerEvent<HTMLButtonElement>,
    index: number,
    widths: number[],
  ) => void;
  onAutoFit: (index: number) => void;
}

function GlossaryDetailView({
  animateEnter,
  glossary,
  entryPage,
  entrySearch,
  entrySort,
  entryLoading,
  entrySortLoading,
  widths,
  totalPages,
  onBack,
  onSearchChange,
  onAdd,
  onEdit,
  onDelete,
  onSort,
  onPageChange,
  onPageSizeChange,
  onResize,
  onAutoFit,
}: GlossaryDetailViewProps) {
  const [tableViewportRef, adaptiveWidths, tableViewportWidth] = useAdaptiveColumnWidths<HTMLDivElement>(
    widths,
    DETAIL_MIN_WIDTHS,
    DETAIL_FLEX_COLUMNS,
    ACTION_COLUMN_WIDTH,
  );
  const tableWidth = sum(adaptiveWidths) + ACTION_COLUMN_WIDTH;
  const tableNeedsHorizontalScroll = tableWidth > tableViewportWidth + 1;

  return (
    <div
      style={animateEnter ? SECONDARY_PAGE_FADE_UP_STYLE : undefined}
      className={cn(
        "flex min-h-0 flex-1 flex-col",
        animateEnter && "app-fade-up-enter",
      )}
    >
      <header className="mb-3 shrink-0">
        <div className="flex items-center gap-2">
          <Button type="button" variant="ghost" size="icon-sm" onClick={onBack}>
            <ArrowLeft className="size-4" />
          </Button>
          <div className="min-w-0 flex-1">
            <h1 className="truncate text-xl font-medium tracking-tight">{glossary.name}</h1>
            <p className="mt-0.5 text-xs text-muted-foreground">
              {displayLanguagePair(glossary.sourceLanguage, glossary.targetLanguage)} · {glossary.entryCount} 条术语
            </p>
          </div>
          <Button type="button" onClick={onAdd}>
            <Plus className="size-4" />
            增加术语
          </Button>
        </div>
      </header>

      <div className="relative mb-3 shrink-0">
        <Search className="pointer-events-none absolute top-1/2 left-2 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          className="pl-8"
          value={entrySearch}
          onChange={(event) => onSearchChange(event.target.value)}
          placeholder="检索术语内容"
        />
      </div>

      <section className="relative min-h-0 flex-1 overflow-hidden rounded-[6px] border bg-card">
        <div
          ref={tableViewportRef}
          className={cn(
            "h-full overflow-y-auto pb-20",
            "scrollbar-subtle overscroll-contain",
            tableNeedsHorizontalScroll ? "overflow-x-auto" : "overflow-x-hidden",
          )}
        >
          <table
            className="table-fixed text-left text-sm"
            style={{
              minWidth: `${sum(DETAIL_MIN_WIDTHS) + ACTION_COLUMN_WIDTH}px`,
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
                  title="原始语言"
                  field="src"
                  sort={entrySort}
                  loadingField={entrySortLoading}
                  columnIndex={0}
                  canResize
                  widths={adaptiveWidths}
                  onSort={onSort}
                  onResize={onResize}
                  onAutoFit={onAutoFit}
                />
                <ResizableHeader
                  title="目标语言"
                  field="dst"
                  sort={entrySort}
                  loadingField={entrySortLoading}
                  columnIndex={1}
                  canResize={false}
                  widths={adaptiveWidths}
                  onSort={onSort}
                  onResize={onResize}
                  onAutoFit={onAutoFit}
                />
                <ActionHeader />
              </tr>
            </thead>
            <tbody>
              {entryLoading ? (
                <TableMessage colSpan={3} icon={<Loader2 className="size-4 animate-spin" />}>
                  正在加载术语条目
                </TableMessage>
              ) : entryPage.entries.length === 0 ? (
                <TableMessage colSpan={3}>暂无术语条目</TableMessage>
              ) : (
                entryPage.entries.map((entry) => (
                  <ContextMenu key={entry.id}>
                    <ContextMenuTrigger asChild>
                      <tr
                        className="cursor-pointer border-b transition-colors duration-100 hover:bg-accent/35 active:bg-accent/60"
                      >
                        <td className="h-9 min-w-0 truncate px-3 py-2" title={entry.src}>
                          {entry.src}
                        </td>
                        <td className="h-9 min-w-0 truncate px-3 py-2" title={entry.dst}>
                          {entry.dst}
                        </td>
                        <td className="h-9 px-2 py-1 text-center">
                          <GlossaryEntryActionDropdown
                            entry={entry}
                            onEdit={onEdit}
                            onDelete={onDelete}
                          />
                        </td>
                      </tr>
                    </ContextMenuTrigger>
                    <GlossaryEntryContextMenuContent
                      entry={entry}
                      onEdit={onEdit}
                      onDelete={onDelete}
                    />
                  </ContextMenu>
                ))
              )}
            </tbody>
          </table>
        </div>
        <PaginationBar
          page={entryPage.page}
          pageSize={entryPage.pageSize}
          totalItems={entryPage.total}
          totalPages={totalPages}
          onPageChange={onPageChange}
          onPageSizeChange={onPageSizeChange}
        />
      </section>
    </div>
  );
}

interface GlossaryEntryActionMenuProps {
  entry: GlossaryEntryView;
  onEdit: (entry: GlossaryEntryView) => void;
  onDelete: (entry: GlossaryEntryView) => void;
}

function GlossaryEntryContextMenuContent({
  entry,
  onEdit,
  onDelete,
}: GlossaryEntryActionMenuProps) {
  return (
    <ContextMenuContent className="w-40">
      <ContextMenuItem onSelect={() => onEdit(entry)}>
        <Edit3 className="size-3.5" />
        编辑条目
      </ContextMenuItem>
      <ContextMenuSeparator />
      <ContextMenuItem
        className="text-destructive focus:bg-destructive/10 focus:text-destructive"
        onSelect={() => onDelete(entry)}
      >
        <Trash2 className="size-3.5" />
        删除条目
      </ContextMenuItem>
    </ContextMenuContent>
  );
}

function GlossaryEntryActionDropdown({
  entry,
  onEdit,
  onDelete,
}: GlossaryEntryActionMenuProps) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="mx-auto size-7 border-0 bg-transparent text-muted-foreground shadow-none hover:bg-[var(--button-ghost-hover-bg)] hover:text-foreground active:bg-[var(--button-ghost-pressed-bg)] active:text-foreground active:duration-[60ms] focus-visible:border-transparent focus-visible:ring-0 aria-expanded:bg-[var(--button-ghost-hover-bg)] aria-expanded:text-foreground"
          aria-label={`${entry.src} 操作`}
          title="操作"
          onClick={(event) => event.stopPropagation()}
          onDoubleClick={(event) => event.stopPropagation()}
        >
          <MoreHorizontal className="size-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-40">
        <DropdownMenuItem onSelect={() => onEdit(entry)}>
          <Edit3 className="size-3.5" />
          编辑条目
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          className="text-destructive focus:bg-destructive/10 focus:text-destructive"
          onSelect={() => onDelete(entry)}
        >
          <Trash2 className="size-3.5" />
          删除条目
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function ActionHeader() {
  return (
    <th className="h-9 border-b px-2 text-center text-xs font-medium whitespace-nowrap text-muted-foreground">
      操作
    </th>
  );
}

interface ResizableHeaderProps<Field extends string> {
  title: string;
  field?: Field;
  sort?: { field: Field; mode: SortMode };
  loadingField?: Field | null;
  columnIndex: number;
  canResize: boolean;
  isLastColumn?: boolean;
  onSort?: (field: Field) => void;
  onResize: (
    event: ReactPointerEvent<HTMLButtonElement>,
    index: number,
    widths: number[],
  ) => void;
  onAutoFit: (index: number) => void;
  widths: number[];
}

function ResizableHeader<Field extends string>({
  title,
  field,
  sort,
  loadingField,
  columnIndex,
  canResize,
  isLastColumn = false,
  onSort,
  onResize,
  onAutoFit,
  widths,
}: ResizableHeaderProps<Field>) {
  const active = Boolean(field && sort?.field === field);
  const label = active ? SORT_LABELS[sort?.mode ?? "created-desc"] : SORT_LABELS["created-desc"];
  const loading = Boolean(field && loadingField === field);
  return (
    <th className="relative h-9 border-b px-0 text-xs font-medium whitespace-nowrap text-muted-foreground">
      {field && onSort ? (
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
      ) : (
        <div className="flex h-9 items-center px-3 whitespace-nowrap">{title}</div>
      )}
      {canResize && (
        <button
          type="button"
          aria-label="调整列宽"
          className={cn(
            "absolute top-0 right-0 h-full w-2 cursor-col-resize touch-none transition-colors",
            !isLastColumn && "border-r border-transparent hover:border-primary/70",
          )}
          onPointerDown={(event) => onResize(event, columnIndex, widths)}
          onDoubleClick={() => onAutoFit(columnIndex)}
        />
      )}
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

function TagList({ tags }: { tags: string[] }) {
  if (tags.length === 0) {
    return <span className="text-xs text-muted-foreground">无标签</span>;
  }
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

function TableSkeletonRows({ columns }: { columns: number }) {
  return (
    <>
      {Array.from({ length: 6 }).map((_, index) => (
        <tr key={index} className="border-b">
          <td colSpan={columns} className="h-10 px-3 py-2">
            <div className="grid grid-cols-[minmax(8rem,1fr)_5rem_minmax(7rem,0.75fr)_minmax(7rem,0.75fr)] gap-3">
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

interface UploadGlossaryDialogProps {
  open: boolean;
  filePath: string;
  name: string;
  sourceLanguage: string;
  targetLanguage: string;
  tags: string;
  uploading: boolean;
  onOpenChange: (open: boolean) => void;
  onChooseFile: () => void;
  onNameChange: (value: string) => void;
  onSourceLanguageChange: (value: string) => void;
  onTargetLanguageChange: (value: string) => void;
  onTagsChange: (value: string) => void;
  onSubmit: () => void;
}

function UploadGlossaryDialog({
  open,
  filePath,
  name,
  sourceLanguage,
  targetLanguage,
  tags,
  uploading,
  onOpenChange,
  onChooseFile,
  onNameChange,
  onSourceLanguageChange,
  onTargetLanguageChange,
  onTagsChange,
  onSubmit,
}: UploadGlossaryDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent open={open} className="max-w-xl">
        <DialogHeader>
          <DialogTitle>上传术语表</DialogTitle>
          <DialogDescription>
            仅支持 CSV 和 JSON，内容必须只有 src 和 dst 两个字段。
          </DialogDescription>
        </DialogHeader>
        <DialogField>
          <Label>文件</Label>
          <div className="flex gap-2">
            <Input readOnly value={filePath ? fileName(filePath) : ""} placeholder="选择 .csv 或 .json 文件" />
            <Button type="button" variant="outline" onClick={onChooseFile}>
              <Upload className="size-4" />
              选择
            </Button>
          </div>
        </DialogField>
        <DialogField>
          <Label>名称</Label>
          <Input value={name} onChange={(event) => onNameChange(event.target.value)} />
        </DialogField>
        <div className="grid gap-3 sm:grid-cols-2">
          <DialogField>
            <Label>原始语言</Label>
            <LanguageSelect value={sourceLanguage} onValueChange={onSourceLanguageChange} />
          </DialogField>
          <DialogField>
            <Label>目标语言</Label>
            <LanguageSelect value={targetLanguage} onValueChange={onTargetLanguageChange} />
          </DialogField>
        </div>
        <DialogField>
          <Label>标签</Label>
          <Input
            value={tags}
            onChange={(event) => onTagsChange(event.target.value)}
            placeholder="多个标签用逗号分隔"
          />
        </DialogField>
        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            取消
          </Button>
          <Button type="button" disabled={uploading} onClick={onSubmit}>
            {uploading && <Loader2 className="size-4 animate-spin" />}
            导入
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

interface GlossaryInfoDialogProps {
  open: boolean;
  name: string;
  sourceLanguage: string;
  targetLanguage: string;
  tags: string;
  saving: boolean;
  onOpenChange: (open: boolean) => void;
  onNameChange: (value: string) => void;
  onSourceLanguageChange: (value: string) => void;
  onTargetLanguageChange: (value: string) => void;
  onTagsChange: (value: string) => void;
  onSubmit: () => void;
}

function GlossaryInfoDialog({
  open,
  name,
  sourceLanguage,
  targetLanguage,
  tags,
  saving,
  onOpenChange,
  onNameChange,
  onSourceLanguageChange,
  onTargetLanguageChange,
  onTagsChange,
  onSubmit,
}: GlossaryInfoDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent open={open} className="max-w-xl">
        <DialogHeader>
          <DialogTitle>编辑术语表信息</DialogTitle>
          <DialogDescription>
            仅更新术语表元信息和索引，不会重命名磁盘上的 .ing 文件。
          </DialogDescription>
        </DialogHeader>
        <DialogField>
          <Label>名称</Label>
          <Input value={name} onChange={(event) => onNameChange(event.target.value)} />
        </DialogField>
        <div className="grid gap-3 sm:grid-cols-2">
          <DialogField>
            <Label>原始语言</Label>
            <LanguageSelect value={sourceLanguage} onValueChange={onSourceLanguageChange} />
          </DialogField>
          <DialogField>
            <Label>目标语言</Label>
            <LanguageSelect value={targetLanguage} onValueChange={onTargetLanguageChange} />
          </DialogField>
        </div>
        <DialogField>
          <Label>标签</Label>
          <Input
            value={tags}
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

function LanguageSelect({
  value,
  onValueChange,
  disabled,
}: {
  value: string;
  onValueChange: (value: string) => void;
  disabled?: boolean;
}) {
  return (
    <LanguageCombobox
      value={value}
      disabled={disabled}
      onValueChange={onValueChange}
      placeholder="选择语言"
      searchPlaceholder="搜索语言"
    />
  );
}

interface EntryEditorDialogProps {
  open: boolean;
  mode: "create" | "edit";
  src: string;
  dst: string;
  saving: boolean;
  onOpenChange: (open: boolean) => void;
  onSrcChange: (value: string) => void;
  onDstChange: (value: string) => void;
  onSubmit: () => void;
}

function EntryEditorDialog({
  open,
  mode,
  src,
  dst,
  saving,
  onOpenChange,
  onSrcChange,
  onDstChange,
  onSubmit,
}: EntryEditorDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent open={open} className="max-w-lg">
        <DialogHeader>
          <DialogTitle>{mode === "edit" ? "编辑术语" : "增加术语"}</DialogTitle>
        </DialogHeader>
        <DialogField>
          <Label>原始语言</Label>
          <Input value={src} onChange={(event) => onSrcChange(event.target.value)} />
        </DialogField>
        <DialogField>
          <Label>目标语言</Label>
          <Input value={dst} onChange={(event) => onDstChange(event.target.value)} />
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

interface ConfirmDialogProps {
  open: boolean;
  title: string;
  description: string;
  confirmText: string;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}

function ConfirmDialog({
  open,
  title,
  description,
  confirmText,
  onOpenChange,
  onConfirm,
}: ConfirmDialogProps) {
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
          <Button type="button" variant="destructive" onClick={onConfirm}>
            {confirmText}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
