import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type Dispatch,
  type DragEvent,
  type SetStateAction,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  Check,
  ChevronDown,
  FileText,
  Gauge,
  BookOpen,
  LayoutDashboard,
  PlayCircle,
  Save,
  SlidersHorizontal,
  UploadCloud,
  X,
  type LucideIcon,
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { useToast } from "@/components/ui/toast-stack";
import type { AssistantView } from "@/features/assistants/types";
import { listGlossaries } from "@/features/glossary/api";
import { displayLanguagePair } from "@/features/glossary/languages";
import {
  AUTO_LANGUAGE_CODE,
  displayLanguage,
  normalizeLanguageCode,
} from "@/features/languages/languageOptions";
import type { GlossaryView } from "@/features/glossary/types";
import type { ProviderView } from "@/features/providers/types";
import {
  createTranslationTask,
  getTranslationConfig,
  updateTranslationConfig,
} from "@/features/translation/api";
import { LanguageSettingsCard } from "@/features/translation/LanguageSettingsCard";
import { ModelAssistantSettingsCard } from "@/features/translation/ModelAssistantSettingsCard";
import type {
  RateLimitStrategy,
  TranslationConfigView,
} from "@/features/translation/types";
import { cn } from "@/lib/utils";
import { appSessionCache } from "@/lib/session-cache";

interface StartPageProps {
  onTaskCreated: () => void;
}

interface RateLimitOption {
  value: RateLimitStrategy;
  label: string;
  description: string;
  icon: LucideIcon;
}

type NumericConfigKey =
  | "chunkTokenLimit"
  | "maxConcurrency"
  | "maxRetries"
  | "maxRequestsPerMinute"
  | "maxTokensPerMinute";

const DEFAULT_CONFIG: TranslationConfigView = {
  sourceLanguage: AUTO_LANGUAGE_CODE,
  customSourceLanguage: "",
  targetLanguage: "zh-CN",
  customTargetLanguage: "",
  providerId: "",
  modelId: "",
  assistantId: "__none__",
  chunkTokenLimit: 4000,
  maxConcurrency: 5,
  maxRetries: 5,
  rateLimitStrategy: "dynamic",
  maxRequestsPerMinute: 60,
  maxTokensPerMinute: 60_000,
  useGlossary: false,
  glossaryMode: "auto",
  glossaryId: null,
};

const RATE_LIMIT_OPTIONS: RateLimitOption[] = [
  {
    value: "dynamic",
    label: "动态限流策略",
    description: "根据响应头与请求结果自动调整速率",
    icon: Gauge,
  },
  {
    value: "manual",
    label: "手动设置",
    description: "使用固定的每分钟请求数与 Token 数",
    icon: SlidersHorizontal,
  },
];

const START_GLOSSARY_ALL_VALUE = "__all__";
const START_GLOSSARY_WIDTHS = [320, 84, 220, 260];
const SUPPORTED_EXTENSIONS = new Set(["txt", "md"]);

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
    glossaryMode: "auto",
    glossaryId: null,
  };
}

export default function StartPage({ onTaskCreated }: StartPageProps) {
  const cachedDraft = appSessionCache.startDraft.read();
  const cachedProviderOptions = appSessionCache.providers("translation").read();
  const cachedAssistantOptions = appSessionCache.assistants("translation").read();
  const cachedGlossaryIndex = appSessionCache.glossaryIndex.read();
  const cachedGlossaries = cachedGlossaryIndex?.filterSeed;
  const cachedConfig = cachedDraft?.config ?? appSessionCache.translationConfig.read();
  const initialConfig =
    cachedConfig && cachedGlossaries
      ? normalizeGlossaryConfig(cachedConfig, cachedGlossaries)
      : (cachedDraft?.config ?? DEFAULT_CONFIG);
  const hasCachedOptions = Boolean(
    cachedDraft ||
      (cachedProviderOptions && cachedAssistantOptions && cachedGlossaries && cachedConfig),
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
  const [providers, setProviders] = useState<ProviderView[]>(
    (cachedProviderOptions ?? []).filter((provider) => provider.enabled),
  );
  const [assistants, setAssistants] = useState<AssistantView[]>(
    cachedAssistantOptions ?? [],
  );
  const [glossaries, setGlossaries] = useState<GlossaryView[]>(cachedGlossaries ?? []);
  const [providerId, setProviderId] = useState(
    cachedDraft?.providerId ?? initialConfig.providerId,
  );
  const [modelId, setModelId] = useState(cachedDraft?.modelId ?? initialConfig.modelId);
  const [assistantId, setAssistantId] = useState<string>(
    cachedDraft?.assistantId ?? initialConfig.assistantId,
  );
  const [config, setConfig] = useState<TranslationConfigView>(initialConfig);
  const [loading, setLoading] = useState(!hasCachedOptions);
  const [busy, setBusy] = useState(false);
  const [savingConfig, setSavingConfig] = useState(false);
  const dropZoneRef = useRef<HTMLButtonElement | null>(null);
  const shouldLoadInitialOptions = useRef(!hasCachedOptions);
  const { pushToast } = useToast();

  const selectedProvider = useMemo(
    () => providers.find((provider) => provider.id === providerId) ?? null,
    [providerId, providers],
  );
  const models = selectedProvider?.models ?? [];
  const selectedRateLimitOption = RATE_LIMIT_OPTIONS.find(
    (option) => option.value === config.rateLimitStrategy,
  ) ?? RATE_LIMIT_OPTIONS[0];
  const SelectedRateLimitIcon = selectedRateLimitOption.icon;

  const addFilePaths = useCallback((paths: string[]): void => {
    const supported = paths.filter(supportedFile);
    setFilePaths((current) => Array.from(new Set([...current, ...supported])));
    if (paths.length > supported.length) {
      pushToast("已忽略不受支持的文件，目前仅支持 .txt 和 .md", "warning");
    }
  }, [pushToast]);

  useEffect(() => {
    if (loading) return;
    appSessionCache.startDraft.set({
      filePaths,
      sourceLanguage,
      detectedSourceLanguage,
      targetLanguage,
      providerId,
      modelId,
      assistantId,
      config,
    });
  }, [
    assistantId,
    config,
    detectedSourceLanguage,
    filePaths,
    loading,
    modelId,
    providerId,
    sourceLanguage,
    targetLanguage,
  ]);

  useEffect(() => {
    if (filePaths.length === 0 || sourceLanguage !== AUTO_LANGUAGE_CODE || !isTauriRuntime()) {
      setDetectedSourceLanguage(null);
      return;
    }
    let cancelled = false;
    void invoke<string | null>("detect_source_language", { filePaths })
      .then((language) => {
        if (!cancelled) setDetectedSourceLanguage(language);
      })
      .catch(() => {
        if (!cancelled) setDetectedSourceLanguage(null);
      });
    return () => {
      cancelled = true;
    };
  }, [filePaths, sourceLanguage]);

  useEffect(() => {
    if (!shouldLoadInitialOptions.current) return;
    shouldLoadInitialOptions.current = false;
    void refreshOptions();
  }, []);

  useEffect(() => {
    if (providers.length === 0) {
      setProviderId("");
      return;
    }
    if (!providers.some((provider) => provider.id === providerId)) {
      setProviderId(providers[0].id);
    }
  }, [providerId, providers]);

  useEffect(() => {
    if (!modelId && models.length > 0) {
      setModelId(models[0].id);
      return;
    }
    if (modelId && !models.some((model) => model.id === modelId)) {
      setModelId(models[0]?.id ?? "");
    }
  }, [modelId, models]);

  useEffect(() => {
    if (assistantId !== "__none__" && !assistants.some((assistant) => assistant.id === assistantId)) {
      setAssistantId("__none__");
    }
  }, [assistantId, assistants]);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let dispose: (() => void) | undefined;
    void getCurrentWebview().onDragDropEvent((event) => {
      const payload = event.payload;
      if (payload.type === "leave") {
        setDragActive(false);
        return;
      }
      const bounds = dropZoneRef.current?.getBoundingClientRect();
      const scale = window.devicePixelRatio || 1;
      const inside = Boolean(
        bounds
          && payload.position.x / scale >= bounds.left
          && payload.position.x / scale <= bounds.right
          && payload.position.y / scale >= bounds.top
          && payload.position.y / scale <= bounds.bottom,
      );
      setDragActive(inside);
      if (payload.type === "drop") {
        setDragActive(false);
        if (inside) addFilePaths(payload.paths);
      }
    }).then((unlisten) => {
      dispose = unlisten;
    });
    return () => dispose?.();
  }, [addFilePaths]);

  async function refreshOptions(): Promise<void> {
    setLoading(true);
    try {
      if (!isTauriRuntime()) {
        setProviders([]);
        setAssistants([]);
        setGlossaries([]);
        setConfig(DEFAULT_CONFIG);
        setSourceLanguage(DEFAULT_CONFIG.sourceLanguage);
        setTargetLanguage(DEFAULT_CONFIG.targetLanguage);
        setProviderId("");
        setModelId("");
        setAssistantId(DEFAULT_CONFIG.assistantId);
        appSessionCache.providers("translation").set([]);
        appSessionCache.assistants("translation").set([]);
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
      const [providerResult, assistantResult, configResult, glossaryIndex] = await Promise.all([
        appSessionCache
          .providers("translation")
          .loadOnce(() => invoke<ProviderView[]>("list_providers", { purpose: "translation" })),
        appSessionCache
          .assistants("translation")
          .loadOnce(() => invoke<AssistantView[]>("list_assistants", { purpose: "translation" })),
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
      setProviders(providerResult.filter((provider) => provider.enabled));
      setAssistants(assistantResult);
      setGlossaries(glossaryResult);
      const normalizedConfig = normalizeGlossaryConfig(configResult, glossaryResult);
      setConfig(normalizedConfig);
      setSourceLanguage(normalizedConfig.sourceLanguage);
      setTargetLanguage(normalizedConfig.targetLanguage);
      setProviderId(normalizedConfig.providerId);
      setModelId(normalizedConfig.modelId);
      setAssistantId(normalizedConfig.assistantId);
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

  function handleBrowserDrop(event: DragEvent<HTMLButtonElement>): void {
    event.preventDefault();
    setDragActive(false);
    const paths = Array.from(event.dataTransfer.files)
      .map((file) => (file as File & { path?: string }).path ?? "")
      .filter(Boolean);
    if (paths.length > 0) addFilePaths(paths);
  }

  function updateNumber(key: NumericConfigKey, value: string): void {
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
        providerId,
        modelId,
        assistantId,
      });
      setConfig(saved);
      setSourceLanguage(saved.sourceLanguage);
      setTargetLanguage(saved.targetLanguage);
      setProviderId(saved.providerId);
      setModelId(saved.modelId);
      setAssistantId(saved.assistantId);
      appSessionCache.translationConfig.set(saved);
      if (showSuccess) pushToast("全部设置已保存", "success");
      return true;
    } catch (error) {
      pushToast(getErrorMessage(error), "error");
      return false;
    } finally {
      setSavingConfig(false);
    }
  }

  async function createTasks(): Promise<void> {
    if (filePaths.length === 0) {
      pushToast("请先添加至少一个 .txt 或 .md 文件", "warning");
      return;
    }
    if (!providerId || !modelId) {
      pushToast("请先选择已启用的翻译提供商和模型", "warning");
      return;
    }
    const resolvedSourceLanguage = normalizeLanguageCode(sourceLanguage);
    const resolvedTargetLanguage = normalizeLanguageCode(targetLanguage);
    if (!resolvedSourceLanguage || !resolvedTargetLanguage) {
      pushToast("请选择有效的原始语言和目标语言", "warning");
      return;
    }

    setBusy(true);
    try {
      if (!(await saveConfig(false))) return;
      let createdCount = 0;
      const failed: string[] = [];
      for (const path of filePaths) {
        try {
          await createTranslationTask({
            filePath: path,
            sourceLanguage: resolvedSourceLanguage,
            targetLanguage: resolvedTargetLanguage,
            tags: [],
            providerId,
            modelId,
            assistantId: assistantId === "__none__" ? null : assistantId,
          });
          createdCount += 1;
        } catch (error) {
          failed.push(`${fileName(path)}：${getErrorMessage(error)}`);
        }
      }
      if (createdCount > 0) {
        appSessionCache.invalidateProofreading();
        pushToast(`已创建 ${createdCount} 个翻译任务`, "success");
        onTaskCreated();
      }
      if (failed.length > 0) {
        pushToast(`${failed.length} 个任务创建失败：${failed[0]}`, "error");
      }
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="relative flex min-w-0 flex-1 flex-col overflow-hidden p-3">
      <header className="mb-3 shrink-0">
        <div className="flex items-center gap-2">
          <LayoutDashboard className="size-5 text-primary" />
          <h1 className="text-xl font-medium tracking-tight">开始</h1>
        </div>
        <p className="mt-0.5 text-xs text-muted-foreground">
          添加文件、选择翻译执行方式并创建任务，源语言将自动识别。
        </p>
      </header>

      <div className="scrollbar-hidden min-h-0 flex-1 overflow-y-auto pb-16">
        <div className="grid max-w-5xl gap-3">
          <Card size="sm" className="rounded-[6px] py-3">
            <CardHeader className="px-3">
              <div className="flex items-center gap-2">
                <FileText className="size-4 text-primary" />
                <CardTitle>添加翻译文件</CardTitle>
                {filePaths.length > 0 && (
                  <Badge variant="secondary" className="ml-auto rounded-[6px]">
                    {filePaths.length} 个任务
                  </Badge>
                )}
              </div>
            </CardHeader>
            <CardContent className="grid gap-2 px-3">
              <motion.button
                ref={dropZoneRef}
                type="button"
                onClick={() => void pickFiles()}
                onDragEnter={(event) => {
                  event.preventDefault();
                  setDragActive(true);
                }}
                onDragOver={(event) => event.preventDefault()}
                onDragLeave={() => setDragActive(false)}
                onDrop={handleBrowserDrop}
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
                  支持批量添加 .txt 与 .md 文件，每个文件会创建一个任务
                </span>
              </motion.button>

              {filePaths.length > 0 && (
                <div className="grid max-h-32 gap-1 overflow-y-auto rounded-[6px] border bg-muted/20 p-1.5">
                  {filePaths.map((path) => (
                    <div
                      key={path}
                      className="flex h-7 items-center gap-2 rounded-[6px] px-2 hover:bg-accent/60"
                      title={path}
                    >
                      <FileText className="size-3.5 shrink-0 text-primary" />
                      <span className="min-w-0 flex-1 truncate text-xs">{fileName(path)}</span>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon-xs"
                        aria-label={`移除 ${fileName(path)}`}
                        onClick={() => setFilePaths((current) => current.filter((item) => item !== path))}
                      >
                        <X className="size-3.5" />
                      </Button>
                    </div>
                  ))}
                </div>
              )}
            </CardContent>
          </Card>

          {loading ? (
            <StartSettingsSkeleton />
          ) : (
            <>
          <div className="grid items-stretch gap-3 lg:grid-cols-2">
              <LanguageSettingsCard
                sourceLanguage={sourceLanguage}
                detectedSourceLanguage={detectedSourceLanguage}
                targetLanguage={targetLanguage}
                onSourceLanguageChange={setSourceLanguage}
                onTargetLanguageChange={setTargetLanguage}
              />

            <ModelAssistantSettingsCard
              providers={providers}
              models={models}
              assistants={assistants}
              providerId={providerId}
              modelId={modelId}
              assistantId={assistantId}
              loading={loading}
              onProviderChange={setProviderId}
              onModelChange={setModelId}
              onAssistantChange={setAssistantId}
            />
          </div>

          <GlossarySettingsCard
            glossaries={glossaries}
            config={config}
            loading={loading}
            onConfigChange={setConfig}
          />

          <Card size="sm" className="rounded-[6px] py-3">
            <CardHeader className="px-3">
              <div className="flex items-center gap-2">
                <SlidersHorizontal className="size-4 text-primary" />
                <CardTitle>翻译配置</CardTitle>
              </div>
            </CardHeader>
            <CardContent className="grid gap-3 px-3">
              <div className="grid gap-3 md:grid-cols-2">
                <section className="grid content-start gap-3 rounded-[6px] border bg-muted/15 p-3">
                  <div>
                    <div className="text-xs font-medium">基础参数</div>
                    <p className="mt-0.5 text-2xs text-muted-foreground">
                      控制分块大小、并发处理与失败重试。
                    </p>
                  </div>
                  <NumberField
                    label="单块 Token 数"
                    value={config.chunkTokenLimit}
                    min={200}
                    max={8000}
                    disabled={loading}
                    onChange={(value) => updateNumber("chunkTokenLimit", value)}
                  />
                  <NumberField
                    label="最大并发数"
                    value={config.maxConcurrency}
                    min={1}
                    max={32}
                    disabled={loading}
                    onChange={(value) => updateNumber("maxConcurrency", value)}
                  />
                  <NumberField
                    label="最大重试次数"
                    value={config.maxRetries}
                    min={0}
                    max={10}
                    disabled={loading}
                    onChange={(value) => updateNumber("maxRetries", value)}
                  />
                </section>

                <section className="grid content-start gap-3 rounded-[6px] border bg-muted/15 p-3">
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild disabled={loading}>
                      <button
                        type="button"
                        className="flex w-full items-center gap-3 rounded-[6px] border bg-background/60 px-3 py-2 text-left outline-none transition-colors duration-150 hover:bg-accent/40 focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30"
                      >
                        <SelectedRateLimitIcon className="size-4 shrink-0 text-primary" />
                        <span className="min-w-0 flex-1">
                          <span className="block truncate text-sm font-medium">
                            {selectedRateLimitOption.label}
                          </span>
                          <span className="mt-0.5 block truncate text-2xs text-muted-foreground">
                            {selectedRateLimitOption.description}
                          </span>
                        </span>
                        <ChevronDown className="size-4 shrink-0 text-muted-foreground" />
                      </button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent
                      align="start"
                      className="w-[var(--radix-dropdown-menu-trigger-width)] py-1"
                    >
                      {RATE_LIMIT_OPTIONS.map((option) => {
                        const Icon = option.icon;
                        const selected = option.value === config.rateLimitStrategy;
                        return (
                          <DropdownMenuItem
                            key={option.value}
                            className={cn(
                              "h-auto min-h-14 items-start gap-3 rounded-none px-3 py-2",
                              selected && "bg-accent",
                            )}
                            onSelect={() => setConfig((current) => ({
                              ...current,
                              rateLimitStrategy: option.value,
                            }))}
                          >
                            <Icon className="mt-0.5 size-4 shrink-0 text-primary" />
                            <span className="min-w-0 flex-1">
                              <span className="block text-sm font-medium">{option.label}</span>
                              <span className="mt-0.5 block text-2xs leading-snug text-muted-foreground">
                                {option.description}
                              </span>
                            </span>
                            {selected && <Check className="mt-0.5 size-4 shrink-0 text-primary" />}
                          </DropdownMenuItem>
                        );
                      })}
                    </DropdownMenuContent>
                  </DropdownMenu>

                  {config.rateLimitStrategy === "manual" && (
                    <div className="grid gap-3">
                      <NumberField
                        label="每分钟最大请求数"
                        value={config.maxRequestsPerMinute}
                        min={1}
                        max={1_000_000}
                        disabled={loading}
                        onChange={(value) => updateNumber("maxRequestsPerMinute", value)}
                      />
                      <NumberField
                        label="每分钟 Token 数"
                        value={config.maxTokensPerMinute}
                        min={1}
                        max={100_000_000}
                        disabled={loading}
                        onChange={(value) => updateNumber("maxTokensPerMinute", value)}
                      />
                    </div>
                  )}
                </section>
              </div>
            </CardContent>
          </Card>
            </>
          )}
        </div>
      </div>

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
          disabled={busy || loading || savingConfig}
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

function StartSettingsSkeleton() {
  return (
    <>
      <div className="grid items-stretch gap-3 lg:grid-cols-2">
        <Card size="sm" className="flex h-full flex-col rounded-[6px] py-3">
          <CardHeader className="px-3">
            <Skeleton className="h-5 w-32" />
          </CardHeader>
          <CardContent className="grid flex-1 content-start gap-3 px-3">
            <SkeletonField />
            <SkeletonField />
          </CardContent>
        </Card>
        <Card size="sm" className="flex h-full flex-col rounded-[6px] py-3">
          <CardHeader className="px-3">
            <Skeleton className="h-5 w-32" />
          </CardHeader>
          <CardContent className="grid flex-1 content-start gap-3 px-3">
            <SkeletonField />
            <SkeletonField />
            <SkeletonField />
          </CardContent>
        </Card>
      </div>

      <Card size="sm" className="rounded-[6px] py-3">
        <CardHeader className="px-3">
          <Skeleton className="h-5 w-28" />
        </CardHeader>
        <CardContent className="px-3">
          <Skeleton className="h-10 w-full" />
        </CardContent>
      </Card>

      <Card size="sm" className="rounded-[6px] py-3">
        <CardHeader className="px-3">
          <Skeleton className="h-5 w-32" />
        </CardHeader>
        <CardContent className="grid gap-3 px-3 md:grid-cols-2">
          <div className="grid gap-3 rounded-[6px] border bg-muted/15 p-3">
            <Skeleton className="h-4 w-28" />
            <SkeletonField />
            <SkeletonField />
            <SkeletonField />
          </div>
          <div className="grid content-start gap-3 rounded-[6px] border bg-muted/15 p-3">
            <Skeleton className="h-10 w-full" />
            <SkeletonField />
            <SkeletonField />
          </div>
        </CardContent>
      </Card>
    </>
  );
}

function SkeletonField() {
  return (
    <div className="grid gap-2">
      <Skeleton className="h-4 w-24" />
      <Skeleton className="h-10 w-full" />
    </div>
  );
}

interface NumberFieldProps {
  label: string;
  value: number;
  min: number;
  max: number;
  disabled: boolean;
  onChange: (value: string) => void;
}

function NumberField({
  label,
  value,
  min,
  max,
  disabled,
  onChange,
}: NumberFieldProps) {
  return (
    <div className="grid gap-2">
      <Label>{label}</Label>
      <Input
        type="number"
        min={min}
        max={max}
        value={value}
        disabled={disabled}
        onChange={(event) => onChange(event.target.value)}
      />
    </div>
  );
}

interface GlossarySettingsCardProps {
  glossaries: GlossaryView[];
  config: TranslationConfigView;
  loading: boolean;
  onConfigChange: Dispatch<SetStateAction<TranslationConfigView>>;
}

function GlossarySettingsCard({
  glossaries,
  config,
  loading,
  onConfigChange,
}: GlossarySettingsCardProps) {
  const hasSelectedGlossary = Boolean(
    config.glossaryId
      && glossaries.some((glossary) => glossary.id === config.glossaryId),
  );
  const selectedValue = config.glossaryMode === "existing" && hasSelectedGlossary
    ? config.glossaryId!
    : "auto";

  function updateSelection(value: string): void {
    if (value === "auto") {
      onConfigChange((current) => ({
        ...current,
        glossaryMode: "auto",
        glossaryId: null,
      }));
      return;
    }
    onConfigChange((current) => ({
      ...current,
      glossaryMode: "existing",
      glossaryId: value,
    }));
  }

  function updateEnabled(enabled: boolean): void {
    onConfigChange((current) => {
      if (
        enabled
        && current.glossaryMode === "existing"
        && !current.glossaryId
        && glossaries.length > 0
      ) {
        return { ...current, useGlossary: enabled, glossaryId: glossaries[0].id };
      }
      return { ...current, useGlossary: enabled };
    });
  }

  return (
    <Card size="sm" className="rounded-[6px] py-3">
      <CardHeader className="px-3">
        <div className="flex items-center gap-2">
          <BookOpen className="size-4 text-primary" />
          <CardTitle>术语表</CardTitle>
          <div className="ml-auto flex items-center gap-2">
            <Label className="text-xs text-muted-foreground">使用术语表</Label>
            <Switch
              size="sm"
              checked={config.useGlossary}
              disabled={loading}
              onCheckedChange={updateEnabled}
            />
          </div>
        </div>
      </CardHeader>
      <CardContent className="grid gap-2 px-3">
        <Select
          value={selectedValue}
          disabled={loading || !config.useGlossary}
          onValueChange={updateSelection}
        >
          <SelectTrigger>
            <SelectValue placeholder="选择术语表" />
          </SelectTrigger>
          <SelectContent viewportClassName="max-h-72">
            <SelectItem value="auto">自动建立术语表</SelectItem>
            <div className="px-3 py-1.5">
              <Separator />
              <div className="pt-1.5 text-2xs text-muted-foreground">已有术语表</div>
            </div>
            {glossaries.length === 0 ? (
              <div className="px-3 py-2 text-xs text-muted-foreground">暂无已有术语表</div>
            ) : (
              glossaries.map((glossary) => (
                <SelectItem key={glossary.id} value={glossary.id}>
                  <span className="flex min-w-0 items-center gap-2">
                    <span className="truncate">{glossary.name}</span>
                    <span className="shrink-0 text-2xs text-muted-foreground">
                      {displayLanguagePair(glossary.sourceLanguage, glossary.targetLanguage)}
                    </span>
                  </span>
                </SelectItem>
              ))
            )}
          </SelectContent>
        </Select>
      </CardContent>
    </Card>
  );
}
