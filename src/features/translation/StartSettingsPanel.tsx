import {
  BookOpen,
  FileCheck2,
  Languages,
  SlidersHorizontal,
  type LucideIcon,
} from "lucide-react";
import {
  useMemo,
  useState,
  type Dispatch,
  type ReactNode,
  type SetStateAction,
} from "react";

import { Input } from "@/components/ui/input";
import { HelpTooltip } from "@/components/ui/help-tooltip";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { SelectableOptionButton } from "@/components/ui/selectable-option-button";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import type { AssistantView } from "@/features/assistants/types";
import { displayLanguagePair } from "@/features/glossary/languages";
import type { GlossaryView } from "@/features/glossary/types";
import { LanguageCombobox } from "@/features/languages/LanguageCombobox";
import { displayLanguage } from "@/features/languages/languageOptions";
import type { ProviderView } from "@/features/providers/types";
import {
  ModelRuntimeSettings,
  type ModelRuntimeSettingsValue,
} from "@/features/translation/ModelRuntimeSettings";
import type {
  ContextHandlingMode,
  PdfParsingMode,
  RateLimitStrategy,
  TranslationConfigView,
} from "@/features/translation/types";
import { cn } from "@/lib/utils";

export type StartSettingsNumberKey =
  | "chunkTokenLimit"
  | "maxConcurrency"
  | "maxRetries"
  | "maxFailurePercentage"
  | "maxRequestsPerMinute"
  | "maxTokensPerMinute";

type SettingsTab = "translation" | "glossary" | "task" | "proofreading";
type ProofreadingOptionId = "rule" | "confidence" | "assistant";

interface StartSettingsPanelProps {
  sourceLanguage: string;
  detectedSourceLanguage: string | null;
  targetLanguage: string;
  translationProviders: ProviderView[];
  glossaryProviders: ProviderView[];
  translationAssistants: AssistantView[];
  glossaryAssistants: AssistantView[];
  glossaries: GlossaryView[];
  config: TranslationConfigView;
  loading: boolean;
  onSourceLanguageChange: (value: string) => void;
  onTargetLanguageChange: (value: string) => void;
  onConfigChange: Dispatch<SetStateAction<TranslationConfigView>>;
  onNumberChange: (key: StartSettingsNumberKey, value: string) => void;
}

interface FieldBlockProps {
  label: string;
  children: ReactNode;
  className?: string;
  help?: ReactNode;
}

interface NavItem {
  value: SettingsTab;
  label: string;
  icon: LucideIcon;
}

interface ProofreadingOption {
  id: ProofreadingOptionId;
  label: string;
  description: string;
}

const NAV_ITEMS: NavItem[] = [
  { value: "translation", label: "翻译配置", icon: Languages },
  { value: "glossary", label: "术语表配置", icon: BookOpen },
  { value: "task", label: "任务配置", icon: SlidersHorizontal },
  { value: "proofreading", label: "校对配置", icon: FileCheck2 },
];

const ENABLE_TRANSLATION_HELP =
  "若关闭此开关，可以启用“自动建立术语表”开关，以进行仅建立术语表的任务。";

const RATE_LIMIT_OPTIONS: Array<{
  value: RateLimitStrategy;
  label: string;
  description: string;
}> = [
  {
    value: "dynamic",
    label: "动态限流策略",
    description: "根据响应头与请求结果自动调整速率",
  },
  {
    value: "manual",
    label: "手动限流",
    description: "使用固定的每分钟请求数与 Token 数",
  },
];

const CONTEXT_HANDLING_OPTIONS: Array<{
  value: ContextHandlingMode;
  label: string;
  description: string;
}> = [
  {
    value: "off",
    label: "关闭",
    description: "不告诉模型任何上下文信息",
  },
  {
    value: "sliding-window-target",
    label: "串行滑动窗口 (仅译文)",
    description: "始终告诉模型上一块的译文信息，并发数只能为 1，速度慢",
  },
  {
    value: "sliding-window-source",
    label: "并行滑动窗口 (仅原文)",
    description: "始终告诉模型上一块的原文信息，支持多并发",
  },
  {
    value: "global-background",
    label: "全局背景信息",
    description: "提取文档开头的文本作为背景信息，全局参考",
  },
];

const PDF_PARSING_OPTIONS: Array<{
  value: PdfParsingMode;
  label: string;
  description: string;
}> = [
  {
    value: "local-first",
    label: "优先本地解析",
    description: "优先使用 pdf_oxide 本地解析，失败则使用 MinerU",
  },
  {
    value: "mineru-first",
    label: "优先 MinerU",
    description: "优先通过 MinerU 解析，失败则使用 pdf_oxide",
  },
  {
    value: "local-only",
    label: "仅本地解析",
    description: "仅使用 pdf_oxide 本地解析，失败则中断任务",
  },
  {
    value: "mineru-only",
    label: "仅 MinerU",
    description: "仅使用 MinerU，失败则中断任务",
  },
];

const PROOFREADING_OPTIONS: ProofreadingOption[] = [
  {
    id: "rule",
    label: "规则校对",
    description: "根据正则表达式等寻找错误",
  },
  {
    id: "confidence",
    label: "综合置信度检测",
    description: "向模型请求 Logprobs，可能不被一些提供商支持",
  },
  {
    id: "assistant",
    label: "校对助手",
    description: "使用另一个模型来校对",
  },
];

const TWO_COLUMN_GRID_CLASS = "grid grid-cols-1 gap-3 min-[920px]:grid-cols-2";
const THREE_COLUMN_GRID_CLASS = "grid grid-cols-1 gap-3 min-[1120px]:grid-cols-3";
const FAILURE_THRESHOLD_HELP = "失败分块的比例高于此阈值时，则任务失败。设为 0% 表示超出最大重试次数后出错即失败。";

function autoLabel(detectedSourceLanguage: string | null): string {
  return detectedSourceLanguage
    ? `自动检测 (${displayLanguage(detectedSourceLanguage)})`
    : "自动检测";
}

function FieldBlock({ label, children, className, help }: FieldBlockProps) {
  return (
    <div className={cn("grid min-w-0 content-start gap-1.5", className)}>
      <div className="flex items-center gap-1 text-sm font-medium text-foreground">
        <span>{label}</span>
        {help && <HelpTooltip>{help}</HelpTooltip>}
      </div>
      {children}
    </div>
  );
}

function SettingsNav({
  activeTab,
  onTabChange,
}: {
  activeTab: SettingsTab;
  onTabChange: (tab: SettingsTab) => void;
}) {
  return (
    <nav className="grid content-start gap-1 rounded-[6px] bg-muted/20 p-2 max-[900px]:grid-cols-4 max-[620px]:grid-cols-2">
      {NAV_ITEMS.map((item) => {
        const Icon = item.icon;
        const selected = activeTab === item.value;
        return (
          <button
            key={item.value}
            type="button"
            aria-pressed={selected}
            className={cn(
              "flex h-9 min-w-0 items-center gap-2 rounded-[6px] px-2 text-left text-sm font-medium outline-none transition-[background-color,color,box-shadow] duration-150 focus-visible:ring-3 focus-visible:ring-ring/40",
              selected
                ? "bg-enabled-accent/16 text-enabled-accent hover:bg-enabled-accent/24 hover:text-enabled-accent active:bg-enabled-accent/24 active:shadow-[inset_0_0_0_999px_rgb(0_0_0_/_0.12)] dark:bg-enabled-accent/22 dark:hover:bg-enabled-accent/30 dark:active:bg-enabled-accent/30 dark:active:shadow-[inset_0_0_0_999px_rgb(0_0_0_/_0.18)]"
                : "text-muted-foreground hover:bg-[var(--button-ghost-hover-bg)] hover:text-foreground active:bg-[var(--button-ghost-pressed-bg)]",
            )}
            onClick={() => onTabChange(item.value)}
          >
            <Icon className="size-4 shrink-0" strokeWidth={1.8} />
            <span className="truncate">{item.label}</span>
          </button>
        );
      })}
    </nav>
  );
}

function NumberControl({
  value,
  min,
  max,
  disabled,
  onChange,
}: {
  value: number;
  min: number;
  max: number;
  disabled: boolean;
  onChange: (value: string) => void;
}) {
  return (
    <Input
      type="number"
      min={min}
      max={max}
      step={1}
      value={value}
      disabled={disabled}
      onChange={(event) => {
        const parsed = Number(event.target.value);
        if (Number.isInteger(parsed) && parsed >= min && parsed <= max) {
          onChange(event.target.value);
        }
      }}
    />
  );
}

function glossarySelectedValue(config: TranslationConfigView): string {
  return config.glossaryId ?? "__missing__";
}

function selectedRateLimitLabel(strategy: RateLimitStrategy): string {
  return RATE_LIMIT_OPTIONS.find((option) => option.value === strategy)?.label
    ?? RATE_LIMIT_OPTIONS[0].label;
}

function selectedContextHandlingLabel(mode: ContextHandlingMode): string {
  return CONTEXT_HANDLING_OPTIONS.find((option) => option.value === mode)?.label
    ?? CONTEXT_HANDLING_OPTIONS[0].label;
}

function selectedPdfParsingLabel(mode: PdfParsingMode): string {
  return PDF_PARSING_OPTIONS.find((option) => option.value === mode)?.label
    ?? PDF_PARSING_OPTIONS[0].label;
}

export function StartSettingsSkeleton() {
  return (
    <div className="grid gap-3 rounded-[6px] border bg-card p-3">
      <div className={TWO_COLUMN_GRID_CLASS}>
        <div className="grid gap-1.5">
          <Skeleton className="h-4 w-20 rounded-[6px]" />
          <Skeleton className="h-8 w-full rounded-[6px]" />
        </div>
        <div className="grid gap-1.5">
          <Skeleton className="h-4 w-20 rounded-[6px]" />
          <Skeleton className="h-8 w-full rounded-[6px]" />
        </div>
      </div>
      <div className="grid grid-cols-[12rem_minmax(0,1fr)] gap-3 max-[900px]:grid-cols-1">
        <div className="grid content-start gap-1 rounded-[6px] bg-muted/20 p-2 max-[900px]:grid-cols-4">
          {Array.from({ length: 4 }).map((_, index) => (
            <Skeleton key={index} className="h-9 w-full rounded-[6px]" />
          ))}
        </div>
        <div className={THREE_COLUMN_GRID_CLASS}>
          {Array.from({ length: 3 }).map((_, index) => (
            <div key={index} className="grid gap-1.5">
              <Skeleton className="h-4 w-24 rounded-[6px]" />
              <Skeleton className="h-8 w-full rounded-[6px]" />
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

export function StartSettingsPanel({
  sourceLanguage,
  detectedSourceLanguage,
  targetLanguage,
  translationProviders,
  glossaryProviders,
  translationAssistants,
  glossaryAssistants,
  glossaries,
  config,
  loading,
  onSourceLanguageChange,
  onTargetLanguageChange,
  onConfigChange,
  onNumberChange,
}: StartSettingsPanelProps) {
  const [activeTab, setActiveTab] = useState<SettingsTab>("translation");
  const [localProofreadingOptions, setLocalProofreadingOptions] = useState({
    rule: false,
    assistant: false,
  });
  const selectedGlossaryValue = useMemo(
    () => glossarySelectedValue(config),
    [config],
  );

  function updateExecutionMode(
    control: "translation" | "glossary" | "auto-glossary",
    enabled: boolean,
  ): void {
    onConfigChange((current) => {
      if (control === "translation") {
        return { ...current, enableTranslation: enabled };
      }
      if (control === "auto-glossary") {
        return {
          ...current,
          useGlossary: enabled ? true : current.useGlossary,
          glossaryMode: enabled ? "auto" : "existing",
        };
      }
      return {
        ...current,
        useGlossary: enabled,
        glossaryMode: "existing",
      };
    });
  }

  function updateGlossarySelection(value: string): void {
    onConfigChange((current) => ({
      ...current,
      glossaryMode: "existing",
      glossaryId: value,
    }));
  }

  function updateTranslationRuntime(value: ModelRuntimeSettingsValue): void {
    onConfigChange((current) => ({
      ...current,
      ...value,
      assistantId: value.assistantId ?? "__none__",
    }));
  }

  function updateGlossaryRuntime(value: ModelRuntimeSettingsValue): void {
    onConfigChange((current) => ({
      ...current,
      glossaryGenerationConfig: {
        ...current.glossaryGenerationConfig,
        ...value,
      },
    }));
  }

  function updateGlossaryFailurePercentage(value: string): void {
    const maxFailurePercentage = Number(value);
    onConfigChange((current) => ({
      ...current,
      glossaryGenerationConfig: {
        ...current.glossaryGenerationConfig,
        maxFailurePercentage,
      },
    }));
  }

  function toggleProofreadingOption(optionId: ProofreadingOptionId): void {
    if (optionId === "confidence") {
      onConfigChange((current) => ({
        ...current,
        confidenceMode: current.confidenceMode === "confidence-index"
          ? "off"
          : "confidence-index",
      }));
      return;
    }

    setLocalProofreadingOptions((current) => ({
      ...current,
      [optionId]: !current[optionId],
    }));
  }

  function isProofreadingOptionSelected(optionId: ProofreadingOptionId): boolean {
    if (optionId === "confidence") {
      return config.confidenceMode === "confidence-index";
    }
    return localProofreadingOptions[optionId];
  }

  return (
    <section className="grid gap-3 rounded-[6px] border bg-card p-3">
      <div className={TWO_COLUMN_GRID_CLASS}>
        <FieldBlock label="原始语言">
          <LanguageCombobox
            value={sourceLanguage}
            includeAuto
            autoLabel={autoLabel(detectedSourceLanguage)}
            disabled={loading}
            className="w-full"
            onValueChange={onSourceLanguageChange}
            placeholder="选择原始语言"
            searchPlaceholder="搜索原始语言"
          />
        </FieldBlock>
        <FieldBlock label="目标语言">
          <LanguageCombobox
            value={targetLanguage}
            disabled={loading}
            className="w-full"
            onValueChange={onTargetLanguageChange}
            placeholder="选择目标语言"
            searchPlaceholder="搜索目标语言"
          />
        </FieldBlock>
      </div>

      <div className="grid grid-cols-[12rem_minmax(0,1fr)] gap-3 max-[900px]:grid-cols-1">
        <SettingsNav activeTab={activeTab} onTabChange={setActiveTab} />

        <div className="min-w-0">
          {activeTab === "translation" && (
            <div className="grid gap-3">
              <div className={THREE_COLUMN_GRID_CLASS}>
                <FieldBlock label="启用翻译" help={ENABLE_TRANSLATION_HELP}>
                  <div className="flex h-8 items-center">
                    <Switch
                      size="sm"
                      checked={config.enableTranslation}
                      disabled={loading}
                      onCheckedChange={(enabled) => updateExecutionMode("translation", enabled)}
                    />
                  </div>
                </FieldBlock>
              </div>
              <ModelRuntimeSettings
                value={{
                  providerId: config.providerId,
                  modelId: config.modelId,
                  assistantId: config.assistantId === "__none__" ? null : config.assistantId,
                  thinkingEffort: config.thinkingEffort,
                  useWebSearch: config.useWebSearch,
                  useCustomParameters: config.useCustomParameters,
                }}
                providers={translationProviders}
                assistants={translationAssistants}
                providerLabel="模型提供商"
                modelLabel="翻译模型"
                assistantLabel="助手配置"
                disabled={loading || !config.enableTranslation}
                onChange={updateTranslationRuntime}
              />
              <div className={THREE_COLUMN_GRID_CLASS}>
                <FieldBlock label="最大允许失败率 (%)" help={FAILURE_THRESHOLD_HELP}>
                  <NumberControl
                    value={config.maxFailurePercentage}
                    min={0}
                    max={100}
                    disabled={loading || !config.enableTranslation}
                    onChange={(value) => onNumberChange("maxFailurePercentage", value)}
                  />
                </FieldBlock>
              </div>
            </div>
          )}

          {activeTab === "glossary" && (
            <div className="grid gap-3">
              <div className={THREE_COLUMN_GRID_CLASS}>
                <FieldBlock label="启用术语表">
                  <div className="flex h-8 items-center">
                    <Switch
                      size="sm"
                      checked={config.useGlossary}
                      disabled={loading}
                      onCheckedChange={(enabled) => updateExecutionMode("glossary", enabled)}
                    />
                  </div>
                </FieldBlock>

                <FieldBlock label="自动建立术语表">
                  <div className="flex h-8 items-center">
                    <Switch
                      size="sm"
                      checked={config.useGlossary && config.glossaryMode === "auto"}
                      disabled={loading}
                      onCheckedChange={(enabled) => updateExecutionMode("auto-glossary", enabled)}
                    />
                  </div>
                </FieldBlock>

                <FieldBlock label="选择术语表">
                  <Select
                    value={selectedGlossaryValue}
                    disabled={loading || !config.useGlossary || config.glossaryMode === "auto"}
                    onValueChange={updateGlossarySelection}
                  >
                    <SelectTrigger>
                      <SelectValue placeholder="选择术语表" />
                    </SelectTrigger>
                    <SelectContent viewportClassName="max-h-72">
                      {!config.glossaryId && (
                        <SelectItem value="__missing__" disabled>请选择已有术语表</SelectItem>
                      )}
                      {config.glossaryId
                        && !glossaries.some((glossary) => glossary.id === config.glossaryId) && (
                          <SelectItem value={config.glossaryId} disabled>已失效的术语表</SelectItem>
                        )}
                      {glossaries.map((glossary) => (
                        <SelectItem key={glossary.id} value={glossary.id}>
                          <span className="flex min-w-0 items-center gap-2">
                            <span className="truncate">{glossary.name}</span>
                            <span className="shrink-0 text-xs text-muted-foreground">
                              {displayLanguagePair(glossary.sourceLanguage, glossary.targetLanguage)}
                            </span>
                          </span>
                        </SelectItem>
                      ))}
                      {glossaries.length === 0 && (
                        <div className="px-3 py-2 text-xs text-muted-foreground">
                          暂无已有术语表
                        </div>
                      )}
                    </SelectContent>
                  </Select>
                </FieldBlock>
              </div>

              <ModelRuntimeSettings
                value={config.glossaryGenerationConfig}
                providers={glossaryProviders}
                assistants={glossaryAssistants}
                providerLabel="术语表提供商"
                modelLabel="术语表模型"
                assistantLabel="术语表配置"
                disabled={loading || !config.useGlossary || config.glossaryMode !== "auto"}
                onChange={updateGlossaryRuntime}
              />
              <div className={THREE_COLUMN_GRID_CLASS}>
                <FieldBlock label="最大允许失败率 (%)" help={FAILURE_THRESHOLD_HELP}>
                  <NumberControl
                    value={config.glossaryGenerationConfig.maxFailurePercentage}
                    min={0}
                    max={100}
                    disabled={loading || !config.useGlossary || config.glossaryMode !== "auto"}
                    onChange={updateGlossaryFailurePercentage}
                  />
                </FieldBlock>
              </div>
            </div>
          )}

          {activeTab === "task" && (
            <div className={TWO_COLUMN_GRID_CLASS}>
              <FieldBlock label="单块 Token 数">
                <NumberControl
                  value={config.chunkTokenLimit}
                  min={200}
                  max={8000}
                  disabled={loading}
                  onChange={(value) => onNumberChange("chunkTokenLimit", value)}
                />
              </FieldBlock>

              <FieldBlock label="最大并发数">
                <NumberControl
                  value={config.maxConcurrency}
                  min={1}
                  max={32}
                  disabled={loading || config.contextHandlingMode === "sliding-window-target"}
                  onChange={(value) => onNumberChange("maxConcurrency", value)}
                />
              </FieldBlock>

              <FieldBlock label="最大重试次数">
                <NumberControl
                  value={config.maxRetries}
                  min={0}
                  max={10}
                  disabled={loading}
                  onChange={(value) => onNumberChange("maxRetries", value)}
                />
              </FieldBlock>

              <FieldBlock label="上下文处理模式">
                <Select
                  value={config.contextHandlingMode}
                  disabled={loading}
                  onValueChange={(value) => onConfigChange((current) => ({
                    ...current,
                    contextHandlingMode: value as ContextHandlingMode,
                  }))}
                >
                  <SelectTrigger>
                    <SelectValue placeholder="选择上下文处理模式">
                      {selectedContextHandlingLabel(config.contextHandlingMode)}
                    </SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    {CONTEXT_HANDLING_OPTIONS.map((option) => (
                      <SelectItem
                        key={option.value}
                        value={option.value}
                        textValue={option.label}
                        className="h-auto py-2"
                      >
                        <span className="grid gap-0.5">
                          <span>{option.label}</span>
                          <span className="text-xs leading-4 text-muted-foreground">
                            {option.description}
                          </span>
                        </span>
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </FieldBlock>

              <FieldBlock label="PDF解析模式">
                <Select
                  value={config.pdfParsingMode}
                  disabled={loading}
                  onValueChange={(value) => onConfigChange((current) => ({
                    ...current,
                    pdfParsingMode: value as PdfParsingMode,
                  }))}
                >
                  <SelectTrigger>
                    <SelectValue placeholder="选择PDF解析模式">
                      {selectedPdfParsingLabel(config.pdfParsingMode)}
                    </SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    {PDF_PARSING_OPTIONS.map((option) => (
                      <SelectItem
                        key={option.value}
                        value={option.value}
                        textValue={option.label}
                        className="h-auto py-2"
                      >
                        <span className="grid gap-0.5">
                          <span>{option.label}</span>
                          <span className="text-xs leading-4 text-muted-foreground">
                            {option.description}
                          </span>
                        </span>
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </FieldBlock>

              <FieldBlock label="动态限流策略">
                <Select
                  value={config.rateLimitStrategy}
                  disabled={loading}
                  onValueChange={(value) => onConfigChange((current) => ({
                    ...current,
                    rateLimitStrategy: value as RateLimitStrategy,
                  }))}
                >
                  <SelectTrigger>
                    <SelectValue placeholder="选择限流策略">
                      {selectedRateLimitLabel(config.rateLimitStrategy)}
                    </SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    {RATE_LIMIT_OPTIONS.map((option) => (
                      <SelectItem
                        key={option.value}
                        value={option.value}
                        textValue={option.label}
                        className="h-auto py-2"
                      >
                        <span className="grid gap-0.5">
                          <span>{option.label}</span>
                          <span className="text-xs leading-4 text-muted-foreground">
                            {option.description}
                          </span>
                        </span>
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </FieldBlock>

              {config.rateLimitStrategy === "manual" && (
                <>
                  <FieldBlock label="每分钟请求数">
                    <NumberControl
                      value={config.maxRequestsPerMinute}
                      min={1}
                      max={1_000_000}
                      disabled={loading}
                      onChange={(value) => onNumberChange("maxRequestsPerMinute", value)}
                    />
                  </FieldBlock>

                  <FieldBlock label="每分钟 Token 数">
                    <NumberControl
                      value={config.maxTokensPerMinute}
                      min={1}
                      max={100_000_000}
                      disabled={loading}
                      onChange={(value) => onNumberChange("maxTokensPerMinute", value)}
                    />
                  </FieldBlock>
                </>
              )}
            </div>
          )}

          {activeTab === "proofreading" && (
            <div className={THREE_COLUMN_GRID_CLASS}>
              {PROOFREADING_OPTIONS.map((option) => (
                <SelectableOptionButton
                  key={option.id}
                  label={option.label}
                  description={option.description}
                  selected={isProofreadingOptionSelected(option.id)}
                  indicatorVariant="checkbox"
                  disabled={loading}
                  onClick={() => toggleProofreadingOption(option.id)}
                />
              ))}
            </div>
          )}
        </div>
      </div>
    </section>
  );
}
