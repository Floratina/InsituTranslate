import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type DragEvent,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  FileText,
  LayoutDashboard,
  PlayCircle,
  Save,
  UploadCloud,
  X,
} from "lucide-react";
import { motion } from "motion/react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { useToast } from "@/components/ui/toast-stack";
import { ScrollArea } from "@/components/ui/scroll-area";
import type { AssistantView } from "@/features/assistants/types";
import { listGlossaries } from "@/features/glossary/api";
import type { GlossaryView } from "@/features/glossary/types";
import {
  AUTO_LANGUAGE_CODE,
  normalizeLanguageCode,
} from "@/features/languages/languageOptions";
import type { ProviderView } from "@/features/providers/types";
import {
  cancelTranslationTaskCreation,
  getTranslationConfig,
  publishTranslationTaskCreation,
  startTranslationTaskCreation,
  updateTranslationConfig,
} from "@/features/translation/api";
import {
  StartSettingsPanel,
  StartSettingsSkeleton,
  type StartSettingsNumberKey,
} from "@/features/translation/StartSettingsPanel";
import type {
  ContextHandlingMode,
  ProgressStep,
  TranslationConfigView,
  TranslationTaskCreationProgressPayload,
  TranslationTaskCreationStage,
  TranslationTaskCreationStatus,
} from "@/features/translation/types";
import { appSessionCache, type StartCreationJob } from "@/lib/session-cache";
import { cn } from "@/lib/utils";

interface StartPageProps {
  onTaskCreated: () => void;
}

const DEFAULT_CONFIG: TranslationConfigView = {
  sourceLanguage: AUTO_LANGUAGE_CODE,
  customSourceLanguage: "",
  targetLanguage: "zh-CN",
  customTargetLanguage: "",
  providerId: "",
  modelId: "",
  assistantId: "__none__",
  chunkTokenLimit: 800,
  maxConcurrency: 5,
  maxRetries: 5,
  maxFailurePercentage: 20,
  rateLimitStrategy: "dynamic",
  maxRequestsPerMinute: 60,
  maxTokensPerMinute: 60_000,
  contextHandlingMode: "off",
  enableTranslation: true,
  useGlossary: false,
  glossaryMode: "existing",
  glossaryId: null,
  thinkingEffort: "none",
  useWebSearch: false,
  useCustomParameters: false,
  glossaryGenerationConfig: {
    providerId: "",
    modelId: "",
    assistantId: null,
    thinkingEffort: "none",
    useWebSearch: false,
    useCustomParameters: false,
    maxFailurePercentage: 20,
  },
  confidenceMode: "off",
  pdfParsingMode: "local-first",
};

const START_GLOSSARY_ALL_VALUE = "__all__";
const START_GLOSSARY_WIDTHS = [320, 84, 220, 260];
const SUPPORTED_EXTENSIONS = new Set([
  "pdf",
  "md",
  "epub",
  "html",
  "htm",
  "txt",
  "docx",
  "xlsx",
  "json",
  "srt",
  "ass",
  "lrc",
]);

const CREATION_STAGES: TranslationTaskCreationStage[] = ["ast", "chunking"];
const activeCreationStatuses = new Set<TranslationTaskCreationStatus>(["queued", "running"]);
const ignoredCreationIds = new Set<string>();
const ignoredCreationPaths = new Set<string>();
let creationProgressListenerStarted = false;

type NativeDragDropPayload =
  | { type: "enter"; paths: string[]; position: { x: number; y: number } }
  | { type: "over"; position: { x: number; y: number } }
  | { type: "drop"; paths: string[]; position: { x: number; y: number } }
  | { type: "leave" };

function isTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

function getErrorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

function supportedFile(path: string): boolean {
  const extension = path.split(".").pop()?.toLowerCase() ?? "";
  return SUPPORTED_EXTENSIONS.has(extension);
}

function progressStep(
  state: ProgressStep["state"],
  current: number,
  total: number,
  label: string,
): ProgressStep {
  const percent = total > 0 ? Math.max(0, Math.min(1, current / total)) : state === "success" ? 1 : 0;
  return { state, current, total, percent, label };
}

function createCreationJob(
  filePath: string,
  clientTaskId: string,
  _config?: TranslationConfigView,
): StartCreationJob {
  return {
    clientTaskId,
    filePath,
    status: "queued",
    stages: {
      ast: progressStep("pending", 0, 0, "AST 等待中"),
      chunking: progressStep("pending", 0, 0, "分块等待中"),
      glossary: progressStep("success", 1, 1, "术语表将在任务运行时处理"),
    },
    taskId: null,
    error: null,
  };
}

function temporaryCreationId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return `local_${crypto.randomUUID()}`;
  }
  return `local_${Date.now()}_${Math.random().toString(16).slice(2)}`;
}

function creationStatusActive(status: TranslationTaskCreationStatus): boolean {
  return activeCreationStatuses.has(status);
}

function nativeDropInsideElement(
  position: { x: number; y: number },
  element: HTMLElement | null,
): boolean {
  if (!element) return false;
  const rect = element.getBoundingClientRect();
  const scale = window.devicePixelRatio || 1;
  const x = position.x / scale;
  const y = position.y / scale;
  return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
}

function applyCreationProgressPayload(payload: TranslationTaskCreationProgressPayload): void {
  if (
    ignoredCreationIds.has(payload.clientTaskId)
    || ignoredCreationPaths.has(payload.filePath)
  ) {
    return;
  }
  const existing = appSessionCache.startCreationJobs
    .read()
    .find(
      (job) => job.clientTaskId === payload.clientTaskId || job.filePath === payload.filePath,
    );
  const base = existing ?? createCreationJob(payload.filePath, payload.clientTaskId);
  const jobStatus = (
    payload.task
    || payload.status === "failed"
    || payload.status === "cancelled"
    || payload.status === "queued"
  )
    ? payload.status
    : "running";
  appSessionCache.startCreationJobs.upsert({
    ...base,
    clientTaskId: payload.clientTaskId,
    status: jobStatus,
    stages: {
      ...base.stages,
      [payload.stage]: payload.step,
    },
    taskId: payload.task?.id ?? base.taskId,
    error: payload.error ?? (payload.status === "failed" ? base.error : null),
  });
}

function ensureCreationProgressListener(): void {
  if (creationProgressListenerStarted || !isTauriRuntime()) return;
  creationProgressListenerStarted = true;
  void listen<TranslationTaskCreationProgressPayload>(
    "translation-task-creation-progress",
    (event) => applyCreationProgressPayload(event.payload),
  );
}

function IdleFileRow({
  path,
  onRemove,
}: {
  path: string;
  onRemove: (path: string) => void;
}) {
  return (
    <div
      className="grid h-8 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-2 rounded-[6px] px-2"
      title={path}
    >
      <FileText className="size-3.5 shrink-0 text-primary" />
      <span className="min-w-0 truncate text-xs">{fileName(path)}</span>
      <Button
        type="button"
        variant="ghost"
        size="icon-xs"
        aria-label={`移除 ${fileName(path)}`}
        onClick={() => onRemove(path)}
      >
        <X className="size-3.5" />
      </Button>
    </div>
  );
}

function activeCreationStage(job: StartCreationJob): TranslationTaskCreationStage {
  return CREATION_STAGES.find((stage) => job.stages[stage].state === "failed")
    ?? CREATION_STAGES.find((stage) => job.stages[stage].state === "running")
    ?? CREATION_STAGES.find((stage) => job.stages[stage].state === "pending")
    ?? "chunking";
}

function creationStepWidth(step: ProgressStep): number {
  if (step.state === "success") return 100;
  return Math.round(Math.max(0, Math.min(1, step.percent)) * 100);
}

function creationStageFillClass(stage: TranslationTaskCreationStage, step: ProgressStep): string {
  if (step.state === "failed") return "bg-destructive/20";
  if (stage === "ast") return "bg-primary/20 dark:bg-primary/25";
  if (stage === "chunking") return "bg-emerald-500/20 dark:bg-emerald-400/20";
  return "bg-green-500/20 dark:bg-green-400/20";
}

function CreationFileRow({
  job,
  onRemove,
}: {
  job: StartCreationJob;
  onRemove: (job: StartCreationJob) => void;
}) {
  const activeStage = activeCreationStage(job);
  const activeStep = job.stages[activeStage];
  const width = creationStepWidth(activeStep);
  return (
    <div
      className={cn(
        "relative grid min-h-8 grid-cols-[auto_minmax(0,1fr)_auto_auto] items-center gap-2 overflow-hidden rounded-[6px] px-2",
        job.status === "failed" && "bg-destructive/5",
      )}
      title={job.error ? `${job.filePath}\n${job.error}` : job.filePath}
    >
      <span
        className={cn(
          "pointer-events-none absolute inset-y-0 left-0 transition-[width] duration-150 ease-out",
          creationStageFillClass(activeStage, activeStep),
        )}
        style={{ width: `${width}%` }}
      />
      <FileText className="relative z-10 size-3.5 shrink-0 text-primary" />
      <span className="relative z-10 min-w-0 truncate text-xs">{fileName(job.filePath)}</span>
      <span className="relative z-10 max-w-[46vw] shrink-0 truncate text-2xs text-muted-foreground">
        {activeStep.label} · {width}%
      </span>
      <Button
        type="button"
        variant="ghost"
        size="icon-xs"
        aria-label={`移除 ${fileName(job.filePath)}`}
        onClick={() => onRemove(job)}
        className="relative z-10"
      >
        <X className="size-3.5" />
      </Button>
    </div>
  );
}

function normalizeGlossaryConfig(
  config: TranslationConfigView,
  glossaries: GlossaryView[],
): TranslationConfigView {
  if (config.glossaryMode !== "existing") return config;
  if (config.glossaryId && glossaries.some((glossary) => glossary.id === config.glossaryId)) {
    return config;
  }
  return {
    ...config,
    glossaryId: null,
  };
}

function initializeEmptyRuntimeConfig(
  config: TranslationConfigView,
  translationProviders: ProviderView[],
  glossaryProviders: ProviderView[],
): TranslationConfigView {
  const translationProvider = !config.providerId ? translationProviders[0] : undefined;
  const glossaryProvider = !config.glossaryGenerationConfig.providerId
    ? glossaryProviders[0]
    : undefined;
  return {
    ...config,
    providerId: translationProvider?.id ?? config.providerId,
    modelId: translationProvider?.models[0]?.id ?? config.modelId,
    glossaryGenerationConfig: {
      ...config.glossaryGenerationConfig,
      providerId: glossaryProvider?.id ?? config.glossaryGenerationConfig.providerId,
      modelId: glossaryProvider?.models[0]?.id ?? config.glossaryGenerationConfig.modelId,
    },
  };
}

function normalizeContextHandlingMode(
  mode: ContextHandlingMode | "sliding-window" | undefined,
  useGlobalBackground?: boolean,
): ContextHandlingMode {
  if (mode === "sliding-window") {
    return "sliding-window-target";
  }
  return mode ?? (useGlobalBackground ? "global-background" : "off");
}

function normalizeStartConfig(
  config: TranslationConfigView,
  glossaries?: GlossaryView[],
): TranslationConfigView {
  const { useGlobalBackground, ...configWithoutLegacyBackground } = config;
  const contextHandlingMode = normalizeContextHandlingMode(
    config.contextHandlingMode as ContextHandlingMode | "sliding-window" | undefined,
    useGlobalBackground,
  );
  const withDefaults: TranslationConfigView = {
    ...configWithoutLegacyBackground,
    contextHandlingMode,
    enableTranslation: config.enableTranslation ?? true,
    glossaryMode: config.useGlossary ? config.glossaryMode : "existing",
    maxFailurePercentage: config.maxFailurePercentage ?? DEFAULT_CONFIG.maxFailurePercentage,
    thinkingEffort: config.thinkingEffort ?? "none",
    useWebSearch: config.useWebSearch ?? false,
    useCustomParameters: config.useCustomParameters ?? false,
    glossaryGenerationConfig: {
      ...DEFAULT_CONFIG.glossaryGenerationConfig,
      ...config.glossaryGenerationConfig,
    },
    confidenceMode: config.confidenceMode ?? "off",
    pdfParsingMode: config.pdfParsingMode ?? "local-first",
  };
  return glossaries ? normalizeGlossaryConfig(withDefaults, glossaries) : withDefaults;
}

function executionModeValidationError(config: TranslationConfigView): string | null {
  const autoGlossaryEnabled = config.useGlossary && config.glossaryMode === "auto";
  if (!config.enableTranslation && !autoGlossaryEnabled) {
    return config.useGlossary
      ? "在仅术语表模式下，必须启用自动建立术语表才能创建任务。"
      : "翻译和自动建立术语表必须至少启用一项。";
  }
  if (
    config.enableTranslation
    && config.useGlossary
    && config.glossaryMode === "existing"
    && !config.glossaryId
  ) {
    return "启用术语表时，请选择有效的已有术语表。";
  }
  return null;
}

export default function StartPage({ onTaskCreated }: StartPageProps) {
  const cachedDraft = appSessionCache.startDraft.read();
  const cachedTranslationProviderOptions = appSessionCache.providers("translation").read();
  const cachedGlossaryProviderOptions = appSessionCache.providers("glossary").read();
  const cachedTranslationAssistantOptions = appSessionCache.assistants("translation").read();
  const cachedGlossaryAssistantOptions = appSessionCache.assistants("glossary").read();
  const cachedGlossaryIndex = appSessionCache.glossaryIndex.read();
  const cachedGlossaries = cachedGlossaryIndex?.filterSeed;
  const cachedConfig = cachedDraft?.config ?? appSessionCache.translationConfig.read();
  const normalizedInitialConfig = cachedConfig
    ? normalizeStartConfig(cachedConfig, cachedGlossaries)
    : normalizeStartConfig(cachedDraft?.config ?? DEFAULT_CONFIG);
  const initialConfig = initializeEmptyRuntimeConfig(
    normalizedInitialConfig,
    (cachedTranslationProviderOptions ?? []).filter((provider) => provider.enabled),
    (cachedGlossaryProviderOptions ?? []).filter((provider) => provider.enabled),
  );
  const hasCachedOptions = Boolean(
    cachedTranslationProviderOptions
      && cachedGlossaryProviderOptions
      && cachedTranslationAssistantOptions
      && cachedGlossaryAssistantOptions
      && cachedGlossaries
      && cachedConfig,
  );

  const [filePaths, setFilePaths] = useState<string[]>(cachedDraft?.filePaths ?? []);
  const [dragActive, setDragActive] = useState(false);
  const [sourceLanguage, setSourceLanguage] = useState(
    cachedDraft?.sourceLanguage ?? initialConfig.sourceLanguage,
  );
  const [detectedSourceLanguage, setDetectedSourceLanguage] = useState<string | null>(
    cachedDraft?.detectedSourceLanguage ?? null,
  );
  const [targetLanguage, setTargetLanguage] = useState(
    cachedDraft?.targetLanguage ?? initialConfig.targetLanguage,
  );
  const [translationProviders, setTranslationProviders] = useState<ProviderView[]>(
    (cachedTranslationProviderOptions ?? []).filter((provider) => provider.enabled),
  );
  const [glossaryProviders, setGlossaryProviders] = useState<ProviderView[]>(
    (cachedGlossaryProviderOptions ?? []).filter((provider) => provider.enabled),
  );
  const [translationAssistants, setTranslationAssistants] = useState<AssistantView[]>(
    cachedTranslationAssistantOptions ?? [],
  );
  const [glossaryAssistants, setGlossaryAssistants] = useState<AssistantView[]>(
    cachedGlossaryAssistantOptions ?? [],
  );
  const [glossaries, setGlossaries] = useState<GlossaryView[]>(cachedGlossaries ?? []);
  const [config, setConfig] = useState<TranslationConfigView>(initialConfig);
  const [loading, setLoading] = useState(!hasCachedOptions);
  const [busy, setBusy] = useState(false);
  const [savingConfig, setSavingConfig] = useState(false);
  const [creationJobs, setCreationJobs] = useState<StartCreationJob[]>(
    appSessionCache.startCreationJobs.read(),
  );
  const shouldLoadInitialOptions = useRef(!hasCachedOptions);
  const dropZoneRef = useRef<HTMLButtonElement | null>(null);
  const lastNativeDropRef = useRef<{ key: string; at: number } | null>(null);
  const { pushToast } = useToast();

  const activeCreation = useMemo(
    () => creationJobs.some((job) => creationStatusActive(job.status)),
    [creationJobs],
  );
  const completedCreationJobs = useMemo(
    () => creationJobs.filter((job) => job.status === "success" && job.taskId),
    [creationJobs],
  );
  const creationFilePaths = useMemo(
    () => new Set(creationJobs.map((job) => job.filePath)),
    [creationJobs],
  );
  const idleFilePaths = useMemo(
    () => filePaths.filter((path) => !creationFilePaths.has(path)),
    [creationFilePaths, filePaths],
  );
  const allStartFilePaths = useMemo(
    () => [...idleFilePaths, ...creationJobs.map((job) => job.filePath)],
    [creationJobs, idleFilePaths],
  );
  const fileRowCount = idleFilePaths.length + creationJobs.length;
  const creationReady = useMemo(
    () => (
      creationJobs.length > 0
      && idleFilePaths.length === 0
      && creationJobs.every((job) => job.status === "success" && job.taskId)
    ),
    [creationJobs, idleFilePaths],
  );

  const addFilePaths = useCallback((paths: string[]): void => {
    const supported = paths.filter(supportedFile);
    supported.forEach((path) => ignoredCreationPaths.delete(path));
    if (paths.length > supported.length) {
      pushToast("已忽略不受支持的文件", "warning");
    }
    if (supported.length > 0) {
      void startPreprocessingForPaths(supported);
    }
  }, [
    config,
    filePaths,
    loading,
    pushToast,
    sourceLanguage,
    targetLanguage,
  ]);
  const addFilePathsRef = useRef(addFilePaths);

  useEffect(() => {
    addFilePathsRef.current = addFilePaths;
  }, [addFilePaths]);

  useEffect(() => {
    ensureCreationProgressListener();
    return appSessionCache.startCreationJobs.subscribe(() => {
      setCreationJobs([...appSessionCache.startCreationJobs.read()]);
    });
  }, []);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    const unlisteners: Array<() => void> = [];
    let cancelled = false;
    const handleNativePayload = (payload: NativeDragDropPayload): void => {
      if (payload.type === "leave") {
        setDragActive(false);
        return;
      }
      if (payload.type === "enter" || payload.type === "over") {
        setDragActive(nativeDropInsideElement(payload.position, dropZoneRef.current));
        return;
      }
      setDragActive(false);
      const key = payload.paths.join("\0");
      const now = Date.now();
      const previous = lastNativeDropRef.current;
      if (previous && previous.key === key && now - previous.at < 500) return;
      lastNativeDropRef.current = { key, at: now };
      addFilePathsRef.current(payload.paths);
    };
    const registerUnlistener = (cleanup: () => void): void => {
      if (cancelled) {
        cleanup();
        return;
      }
      unlisteners.push(cleanup);
    };
    void getCurrentWindow()
      .onDragDropEvent((event) => handleNativePayload(event.payload))
      .then(registerUnlistener)
      .catch((error: unknown) => pushToast(getErrorMessage(error), "error"));
    void getCurrentWebview()
      .onDragDropEvent((event) => handleNativePayload(event.payload))
      .then(registerUnlistener)
      .catch((error: unknown) => pushToast(getErrorMessage(error), "error"));
    return () => {
      cancelled = true;
      unlisteners.forEach((cleanup) => cleanup());
    };
  }, [pushToast]);

  useEffect(() => {
    if (loading) return;
    appSessionCache.startDraft.set({
      filePaths: idleFilePaths,
      sourceLanguage,
      detectedSourceLanguage,
      targetLanguage,
      providerId: config.providerId,
      modelId: config.modelId,
      assistantId: config.assistantId,
      config,
    });
  }, [
    config,
    detectedSourceLanguage,
    idleFilePaths,
    loading,
    sourceLanguage,
    targetLanguage,
  ]);

  useEffect(() => {
    if (allStartFilePaths.length === 0 || sourceLanguage !== AUTO_LANGUAGE_CODE || !isTauriRuntime()) {
      setDetectedSourceLanguage(null);
      return;
    }
    let cancelled = false;
    void invoke<string | null>("detect_source_language", { filePaths: allStartFilePaths })
      .then((language) => {
        if (!cancelled) setDetectedSourceLanguage(language);
      })
      .catch(() => {
        if (!cancelled) setDetectedSourceLanguage(null);
      });
    return () => {
      cancelled = true;
    };
  }, [allStartFilePaths, sourceLanguage]);

  useEffect(() => {
    if (!shouldLoadInitialOptions.current) return;
    shouldLoadInitialOptions.current = false;
    void refreshOptions();
  }, []);

  async function refreshOptions(): Promise<void> {
    setLoading(true);
    try {
      if (!isTauriRuntime()) {
        setTranslationProviders([]);
        setGlossaryProviders([]);
        setTranslationAssistants([]);
        setGlossaryAssistants([]);
        setGlossaries([]);
        setConfig(DEFAULT_CONFIG);
        setSourceLanguage(DEFAULT_CONFIG.sourceLanguage);
        setTargetLanguage(DEFAULT_CONFIG.targetLanguage);
        appSessionCache.providers("translation").set([]);
        appSessionCache.providers("glossary").set([]);
        appSessionCache.assistants("translation").set([]);
        appSessionCache.assistants("glossary").set([]);
        appSessionCache.translationConfig.set(DEFAULT_CONFIG);
        appSessionCache.glossaryIndex.set({
          glossaries: [],
          filterSeed: [],
          selectedGlossaryId: null,
          search: "",
          tagFilter: START_GLOSSARY_ALL_VALUE,
          sourceFilter: START_GLOSSARY_ALL_VALUE,
          targetFilter: START_GLOSSARY_ALL_VALUE,
          listSort: { field: "name", mode: "created-desc" },
          listPage: 0,
          listPageSize: 20,
          listWidths: START_GLOSSARY_WIDTHS,
        });
        return;
      }
      const [
        translationProviderResult,
        glossaryProviderResult,
        translationAssistantResult,
        glossaryAssistantResult,
        configResult,
        glossaryIndex,
      ] = await Promise.all([
        appSessionCache
          .providers("translation")
          .loadOnce(() => invoke<ProviderView[]>("list_providers", { purpose: "translation" })),
        appSessionCache
          .providers("glossary")
          .loadOnce(() => invoke<ProviderView[]>("list_providers", { purpose: "glossary" })),
        appSessionCache
          .assistants("translation")
          .loadOnce(() => invoke<AssistantView[]>("list_assistants", { purpose: "translation" })),
        appSessionCache
          .assistants("glossary")
          .loadOnce(() => invoke<AssistantView[]>("list_assistants", { purpose: "glossary" })),
        appSessionCache.translationConfig.loadOnce(getTranslationConfig),
        appSessionCache.glossaryIndex.loadOnce(async () => {
          const glossaries = await listGlossaries(null);
          return {
            glossaries,
            filterSeed: glossaries,
            selectedGlossaryId: null,
            search: "",
            tagFilter: START_GLOSSARY_ALL_VALUE,
            sourceFilter: START_GLOSSARY_ALL_VALUE,
            targetFilter: START_GLOSSARY_ALL_VALUE,
            listSort: { field: "name" as const, mode: "created-desc" as const },
            listPage: 0,
            listPageSize: 20,
            listWidths: START_GLOSSARY_WIDTHS,
          };
        }),
      ]);
      const glossaryResult = glossaryIndex.filterSeed;
      const enabledTranslationProviders = translationProviderResult.filter(
        (provider) => provider.enabled,
      );
      const enabledGlossaryProviders = glossaryProviderResult.filter(
        (provider) => provider.enabled,
      );
      setTranslationProviders(enabledTranslationProviders);
      setGlossaryProviders(enabledGlossaryProviders);
      setTranslationAssistants(translationAssistantResult);
      setGlossaryAssistants(glossaryAssistantResult);
      setGlossaries(glossaryResult);
      const normalizedConfig = initializeEmptyRuntimeConfig(
        normalizeStartConfig(cachedDraft?.config ?? configResult, glossaryResult),
        enabledTranslationProviders,
        enabledGlossaryProviders,
      );
      setConfig(normalizedConfig);
      setSourceLanguage(normalizedConfig.sourceLanguage);
      setTargetLanguage(normalizedConfig.targetLanguage);
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    } finally {
      setLoading(false);
    }
  }

  async function pickFiles(): Promise<void> {
    try {
      const result = await invoke<string[]>("pick_translation_files");
      addFilePaths(result);
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
    }
  }

  function handleBrowserDrag(event: DragEvent<HTMLButtonElement>): void {
    event.preventDefault();
    event.stopPropagation();
    event.dataTransfer.dropEffect = "copy";
    setDragActive(true);
  }

  function handleBrowserDrop(event: DragEvent<HTMLButtonElement>): void {
    event.preventDefault();
    event.stopPropagation();
    setDragActive(false);
    const paths = Array.from(event.dataTransfer.files)
      .map((file) => (file as File & { path?: string }).path ?? "")
      .filter(Boolean);
    if (paths.length > 0) addFilePaths(paths);
    else pushToast("无法读取拖拽文件路径，请点击选择文件", "warning");
  }

  function updateNumber(key: StartSettingsNumberKey, value: string): void {
    const parsed = Number(value);
    setConfig((current) => ({
      ...current,
      [key]: Number.isFinite(parsed) ? parsed : 0,
    }));
  }

  async function saveConfig(showSuccess = true): Promise<boolean> {
    setSavingConfig(true);
    try {
      const saved = await updateTranslationConfig({
        ...config,
        sourceLanguage,
        customSourceLanguage: "",
        targetLanguage,
        customTargetLanguage: "",
      });
      const normalizedSaved = normalizeStartConfig(saved, glossaries);
      setConfig(normalizedSaved);
      setSourceLanguage(saved.sourceLanguage);
      setTargetLanguage(saved.targetLanguage);
      appSessionCache.translationConfig.set(normalizedSaved);
      if (showSuccess) pushToast("全部设置已保存", "success");
      return true;
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
      return false;
    } finally {
      setSavingConfig(false);
    }
  }

  async function startPreprocessingForPaths(paths: string[]): Promise<void> {
    const existingPaths = new Set([
      ...filePaths,
      ...appSessionCache.startCreationJobs.read().map((job) => job.filePath),
    ]);
    const pathsToCreate = Array.from(new Set(paths)).filter((path) => !existingPaths.has(path));
    if (pathsToCreate.length === 0) return;

    if (loading) {
      pushToast("配置仍在加载，请稍后再添加文件", "warning");
      return;
    }
    const executionModeError = executionModeValidationError(config);
    if (executionModeError) {
      pushToast(executionModeError, "warning");
      return;
    }
    if (config.enableTranslation && (!config.providerId || !config.modelId)) {
      pushToast("请先选择已启用的翻译提供商和模型，再添加文件", "warning");
      return;
    }
    if (
      config.useGlossary
      && config.glossaryMode === "auto"
      && (
        !config.glossaryGenerationConfig.providerId
        || !config.glossaryGenerationConfig.modelId
      )
    ) {
      pushToast("请先选择已启用的术语表提供商和模型，再添加文件", "warning");
      return;
    }
    const resolvedSourceLanguage = normalizeLanguageCode(sourceLanguage);
    const resolvedTargetLanguage = normalizeLanguageCode(targetLanguage);
    if (!resolvedSourceLanguage || !resolvedTargetLanguage) {
      pushToast("请选择有效的原始语言和目标语言，再添加文件", "warning");
      return;
    }

    if (!(await saveConfig(false))) return;
    setFilePaths((current) => current.filter((path) => !pathsToCreate.includes(path)));

    for (const path of pathsToCreate) {
      const localId = temporaryCreationId();
      appSessionCache.startCreationJobs.upsert(createCreationJob(path, localId, config));
      try {
        const result = await startTranslationTaskCreation({
          filePath: path,
          sourceLanguage: resolvedSourceLanguage,
          targetLanguage: resolvedTargetLanguage,
          tags: [],
          providerId: config.providerId,
          modelId: config.modelId,
          assistantId: config.assistantId === "__none__" ? null : config.assistantId,
          enableTranslation: config.enableTranslation,
          useGlossary: config.useGlossary,
          glossaryMode: config.glossaryMode,
          glossaryId: config.glossaryMode === "auto" ? null : config.glossaryId,
          glossaryGenerationConfig: config.glossaryGenerationConfig,
        });
        if (ignoredCreationPaths.has(path)) {
          ignoredCreationIds.add(result.clientTaskId);
          void cancelTranslationTaskCreation(result.clientTaskId);
          continue;
        }
        appSessionCache.startCreationJobs.update(localId, (job) => ({
          ...job,
          clientTaskId: result.clientTaskId,
        }));
      } catch (error) {
        appSessionCache.startCreationJobs.update(localId, (job) => ({
          ...job,
          status: "failed",
          error: `${fileName(path)}：${getErrorMessage(error)}`,
          stages: {
            ...job.stages,
            ast: progressStep("failed", 0, 0, "创建任务失败"),
          },
        }));
      }
    }
  }

  async function createTasks(): Promise<void> {
    const executionModeError = executionModeValidationError(config);
    if (executionModeError) {
      pushToast(executionModeError, "warning");
      return;
    }
    if (fileRowCount === 0) {
      pushToast("请先添加需要处理的文件", "warning");
      return;
    }
    if (!creationReady) {
      pushToast("仍有任务未完成预处理，请等待完成或移除失败条目", "warning");
      return;
    }

    setBusy(true);
    try {
      const publishedIds: string[] = [];
      const failed: string[] = [];
      for (const job of completedCreationJobs) {
        try {
          await publishTranslationTaskCreation(job.clientTaskId);
          publishedIds.push(job.clientTaskId);
        } catch (error) {
          failed.push(`${fileName(job.filePath)}：${getErrorMessage(error)}`);
        }
      }
      if (publishedIds.length > 0) {
        const publishedIdSet = new Set(publishedIds);
        appSessionCache.startCreationJobs.set(
          appSessionCache.startCreationJobs
            .read()
            .filter((job) => !publishedIdSet.has(job.clientTaskId)),
        );
        appSessionCache.invalidateProofreading();
        pushToast(`已创建 ${publishedIds.length} 个任务`, "success");
        onTaskCreated();
      }
      if (failed.length > 0) {
        pushToast(`${failed.length} 个任务发布失败：${failed[0]}`, "error");
      }
    } finally {
      setBusy(false);
    }
  }

  function removeIdleFile(path: string): void {
    setFilePaths((current) => current.filter((item) => item !== path));
  }

  async function removeCreationJob(job: StartCreationJob): Promise<void> {
    if (creationStatusActive(job.status)) {
      ignoredCreationIds.add(job.clientTaskId);
      ignoredCreationPaths.add(job.filePath);
      appSessionCache.startCreationJobs.remove(job.clientTaskId);
      if (!job.clientTaskId.startsWith("local_")) {
        try {
          await cancelTranslationTaskCreation(job.clientTaskId);
        } catch (error) {
          pushToast(getErrorMessage(error), "error");
        }
      }
      return;
    }

    if (job.taskId) {
      try {
        await cancelTranslationTaskCreation(job.clientTaskId);
        appSessionCache.invalidateProofreading();
        appSessionCache.startCreationJobs.remove(job.clientTaskId);
      } catch (error) {
        pushToast(getErrorMessage(error), "error");
      }
      return;
    }

    appSessionCache.startCreationJobs.remove(job.clientTaskId);
  }

  const browserDropProps = {
    onDragEnter: handleBrowserDrag,
    onDragOver: handleBrowserDrag,
    onDragLeave: () => setDragActive(false),
    onDrop: handleBrowserDrop,
  };

  return (
    <main
      data-layout="start-settings-v2"
      className="relative flex min-w-0 flex-1 flex-col overflow-hidden p-3"
    >
      <header className="mb-3 shrink-0">
        <div className="flex items-center gap-2">
          <LayoutDashboard className="size-5 text-primary" />
          <h1 className="text-xl font-medium tracking-tight">开始</h1>
        </div>
        <p className="mt-0.5 text-xs text-muted-foreground">
          添加文件和修改翻译配置
        </p>
      </header>

      <ScrollArea className="-mr-1.5 min-h-0 flex-1" viewportClassName="pr-1.5">
        <div className="grid w-full gap-3 pb-28">
          <Card size="sm" className="rounded-[6px] py-3">
            <CardHeader className="px-3">
              <div className="flex items-center gap-2">
                <FileText className="size-4 text-primary" />
                <CardTitle>添加翻译文件</CardTitle>
                {fileRowCount > 0 && (
                  <Badge variant="secondary" className="ml-auto rounded-[6px]">
                    {fileRowCount} 个任务
                  </Badge>
                )}
              </div>
            </CardHeader>
            <CardContent className="grid gap-2 px-3">
              <motion.button
                type="button"
                ref={dropZoneRef}
                onClick={() => void pickFiles()}
                {...browserDropProps}
                animate={dragActive ? { scale: 1.005 } : { scale: 1 }}
                transition={{ duration: 0.15, ease: [0.03, 0.59, 0.19, 1] }}
                className={cn(
                  "flex min-h-36 w-full flex-col items-center justify-center gap-2 rounded-[6px] border-2 border-dashed bg-muted/20 px-4 py-5 text-center outline-none transition-colors duration-150 hover:border-primary/55 hover:bg-accent/35 focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30",
                  dragActive && "border-primary bg-accent/55",
                )}
              >
                <span className="flex size-11 items-center justify-center rounded-[6px] border bg-background shadow-sm">
                  <UploadCloud className="size-5 text-primary" />
                </span>
                <span className="text-sm font-medium">
                  拖拽文件到这里，或点击选择文件
                </span>
                <span className="text-xs text-muted-foreground">
                  支持批量添加 PDF、Markdown、EPUB、HTML、TXT、DOCX、XLSX、JSON、字幕与歌词文件
                </span>
              </motion.button>

              {fileRowCount > 0 && (
                <ScrollArea
                  className="max-h-32 rounded-[6px] border bg-muted/20"
                  viewportClassName="h-auto max-h-32"
                >
                  <div className="grid gap-1 p-1.5">
                    {idleFilePaths.map((path) => (
                      <IdleFileRow key={path} path={path} onRemove={removeIdleFile} />
                    ))}
                    {creationJobs.map((job) => (
                      <CreationFileRow
                        key={job.clientTaskId}
                        job={job}
                        onRemove={(target) => void removeCreationJob(target)}
                      />
                    ))}
                  </div>
                </ScrollArea>
              )}
            </CardContent>
          </Card>

          {loading ? (
            <StartSettingsSkeleton />
          ) : (
            <StartSettingsPanel
              sourceLanguage={sourceLanguage}
              detectedSourceLanguage={detectedSourceLanguage}
              targetLanguage={targetLanguage}
              translationProviders={translationProviders}
              glossaryProviders={glossaryProviders}
              translationAssistants={translationAssistants}
              glossaryAssistants={glossaryAssistants}
              glossaries={glossaries}
              config={config}
              loading={loading}
              onSourceLanguageChange={setSourceLanguage}
              onTargetLanguageChange={setTargetLanguage}
              onConfigChange={setConfig}
              onNumberChange={updateNumber}
            />
          )}
        </div>
      </ScrollArea>

      <div className="absolute right-5 bottom-5 z-20 flex items-center gap-2">
        <Button
          type="button"
          variant="outline"
          disabled={savingConfig || loading}
          onClick={() => void saveConfig()}
          className="h-9 bg-card px-4 shadow-[0_6px_18px_rgba(0,0,0,0.14)] dark:bg-card dark:shadow-[0_6px_20px_rgba(0,0,0,0.34)]"
        >
          <Save className="size-4" />
          保存全部设置
        </Button>
        <Button
          type="button"
          disabled={
            busy
            || activeCreation
            || loading
            || savingConfig
          }
          onClick={() => void createTasks()}
          className="h-9 px-4 shadow-[0_6px_18px_rgba(0,0,0,0.16)] dark:shadow-[0_6px_20px_rgba(0,0,0,0.38)]"
        >
          <PlayCircle className="size-4" />
          创建任务
        </Button>
      </div>
    </main>
  );
}
