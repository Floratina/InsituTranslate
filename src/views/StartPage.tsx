import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type DragEvent,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
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
import type { AssistantView } from "@/features/assistants/types";
import { listGlossaries } from "@/features/glossary/api";
import type { GlossaryView } from "@/features/glossary/types";
import {
  AUTO_LANGUAGE_CODE,
  normalizeLanguageCode,
} from "@/features/languages/languageOptions";
import type { ProviderView } from "@/features/providers/types";
import {
  createTranslationTask,
  getTranslationConfig,
  updateTranslationConfig,
} from "@/features/translation/api";
import {
  StartSettingsPanel,
  StartSettingsSkeleton,
  type StartSettingsNumberKey,
} from "@/features/translation/StartSettingsPanel";
import type { TranslationConfigView } from "@/features/translation/types";
import { appSessionCache } from "@/lib/session-cache";
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
  rateLimitStrategy: "dynamic",
  maxRequestsPerMinute: 60,
  maxTokensPerMinute: 60_000,
  useGlossary: false,
  glossaryMode: "auto",
  glossaryId: null,
  confidenceMode: "off",
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

function normalizeStartConfig(
  config: TranslationConfigView,
  glossaries?: GlossaryView[],
): TranslationConfigView {
  const withDefaults: TranslationConfigView = {
    ...config,
    confidenceMode: config.confidenceMode ?? "off",
  };
  return glossaries ? normalizeGlossaryConfig(withDefaults, glossaries) : withDefaults;
}

export default function StartPage({ onTaskCreated }: StartPageProps) {
  const cachedDraft = appSessionCache.startDraft.read();
  const cachedProviderOptions = appSessionCache.providers("translation").read();
  const cachedAssistantOptions = appSessionCache.assistants("translation").read();
  const cachedGlossaryIndex = appSessionCache.glossaryIndex.read();
  const cachedGlossaries = cachedGlossaryIndex?.filterSeed;
  const cachedConfig = cachedDraft?.config ?? appSessionCache.translationConfig.read();
  const initialConfig = cachedConfig
    ? normalizeStartConfig(cachedConfig, cachedGlossaries)
    : normalizeStartConfig(cachedDraft?.config ?? DEFAULT_CONFIG);
  const hasCachedOptions = Boolean(
    cachedDraft
      || (cachedProviderOptions && cachedAssistantOptions && cachedGlossaries && cachedConfig),
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

  const addFilePaths = useCallback((paths: string[]): void => {
    const supported = paths.filter(supportedFile);
    setFilePaths((current) => Array.from(new Set([...current, ...supported])));
    if (paths.length > supported.length) {
      pushToast("已忽略不受支持的文件", "warning");
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
      pushToast("请先添加至少一个支持的文件", "warning");
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
          添加文件、选择翻译执行方式并创建任务，源语言将自动识别。
        </p>
      </header>

      <div className="scrollbar-hidden min-h-0 flex-1 overflow-y-auto pb-28">
        <div className="grid w-full gap-3">
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
                  支持批量添加 PDF、Markdown、EPUB、HTML、TXT、DOCX、XLSX、JSON、字幕与歌词文件
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
            <StartSettingsPanel
              sourceLanguage={sourceLanguage}
              detectedSourceLanguage={detectedSourceLanguage}
              targetLanguage={targetLanguage}
              providers={providers}
              models={models}
              assistants={assistants}
              glossaries={glossaries}
              providerId={providerId}
              modelId={modelId}
              assistantId={assistantId}
              config={config}
              loading={loading}
              onSourceLanguageChange={setSourceLanguage}
              onTargetLanguageChange={setTargetLanguage}
              onProviderChange={setProviderId}
              onModelChange={setModelId}
              onAssistantChange={setAssistantId}
              onConfigChange={setConfig}
              onNumberChange={updateNumber}
            />
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
