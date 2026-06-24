import {
  BookOpen,
  Bot,
  FileCheck2,
  Gauge,
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
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { AssistantIcon } from "@/features/assistants/AssistantIcon";
import type { AssistantView } from "@/features/assistants/types";
import { displayLanguagePair } from "@/features/glossary/languages";
import type { GlossaryView } from "@/features/glossary/types";
import { LanguageCombobox } from "@/features/languages/LanguageCombobox";
import { displayLanguage } from "@/features/languages/languageOptions";
import { ProviderAvatar } from "@/features/providers/ProviderAvatar";
import type { ModelView, ProviderView } from "@/features/providers/types";
import type { RateLimitStrategy, TranslationConfigView } from "@/features/translation/types";
import { cn } from "@/lib/utils";

export type StartSettingsNumberKey =
  | "chunkTokenLimit"
  | "maxConcurrency"
  | "maxRetries"
  | "maxRequestsPerMinute"
  | "maxTokensPerMinute";

type SettingsTab = "translation" | "glossary" | "task" | "proofreading";

interface StartSettingsPanelProps {
  sourceLanguage: string;
  detectedSourceLanguage: string | null;
  targetLanguage: string;
  providers: ProviderView[];
  models: ModelView[];
  assistants: AssistantView[];
  glossaries: GlossaryView[];
  providerId: string;
  modelId: string;
  assistantId: string;
  config: TranslationConfigView;
  loading: boolean;
  onSourceLanguageChange: (value: string) => void;
  onTargetLanguageChange: (value: string) => void;
  onProviderChange: (value: string) => void;
  onModelChange: (value: string) => void;
  onAssistantChange: (value: string) => void;
  onConfigChange: Dispatch<SetStateAction<TranslationConfigView>>;
  onNumberChange: (key: StartSettingsNumberKey, value: string) => void;
}

interface SettingRowProps {
  label: string;
  description?: string;
  children: ReactNode;
}

interface NavItem {
  value: SettingsTab;
  label: string;
  icon: LucideIcon;
}

const NAV_ITEMS: NavItem[] = [
  { value: "translation", label: "翻译配置", icon: Languages },
  { value: "glossary", label: "术语表配置", icon: BookOpen },
  { value: "task", label: "任务配置", icon: SlidersHorizontal },
  { value: "proofreading", label: "校对配置", icon: FileCheck2 },
];

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

const CONTROL_CLASS_NAME = "w-72 max-w-[42vw] shrink-0 max-[760px]:w-full max-[760px]:max-w-none";

function autoLabel(detectedSourceLanguage: string | null): string {
  return detectedSourceLanguage
    ? `自动检测 (${displayLanguage(detectedSourceLanguage)})`
    : "自动检测";
}

function SettingRow({ label, description, children }: SettingRowProps) {
  return (
    <div className="flex items-center justify-between gap-6 border-b border-border py-5 max-[760px]:items-start max-[760px]:gap-3 max-[760px]:flex-col">
      <div className="min-w-0">
        <div className="text-sm font-semibold text-foreground">{label}</div>
        {description && (
          <div className="mt-1 text-xs leading-5 text-muted-foreground">
            {description}
          </div>
        )}
      </div>
      <div className={CONTROL_CLASS_NAME}>{children}</div>
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
    <nav className="grid content-start gap-1 border-r p-2 max-[900px]:border-r-0 max-[900px]:border-b max-[900px]:grid-cols-4 max-[620px]:grid-cols-2">
      {NAV_ITEMS.map((item) => {
        const Icon = item.icon;
        const selected = activeTab === item.value;
        return (
          <button
            key={item.value}
            type="button"
            aria-pressed={selected}
            className={cn(
              "flex h-9 min-w-0 items-center gap-2 rounded-[6px] px-2 text-left text-sm font-medium outline-none transition-[background-color,color] duration-150 hover:bg-[var(--button-ghost-hover-bg)] hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/40",
              selected
                ? "bg-enabled-accent/16 text-enabled-accent"
                : "text-muted-foreground",
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
      value={value}
      disabled={disabled}
      onChange={(event) => onChange(event.target.value)}
    />
  );
}

function glossarySelectedValue(config: TranslationConfigView, glossaries: GlossaryView[]): string {
  const hasSelectedGlossary = Boolean(
    config.glossaryId
      && glossaries.some((glossary) => glossary.id === config.glossaryId),
  );
  return config.glossaryMode === "existing" && hasSelectedGlossary
    ? config.glossaryId!
    : "auto";
}

export function StartSettingsSkeleton() {
  return (
    <div className="grid min-h-80 grid-cols-[13rem_minmax(0,1fr)] overflow-hidden rounded-[6px] border bg-card max-[900px]:grid-cols-1">
      <div className="grid content-start gap-1 border-r p-2 max-[900px]:border-r-0 max-[900px]:border-b max-[900px]:grid-cols-4">
        {Array.from({ length: 4 }).map((_, index) => (
          <Skeleton key={index} className="h-9 w-full rounded-[6px]" />
        ))}
      </div>
      <div className="px-4 pb-28">
        {Array.from({ length: 5 }).map((_, index) => (
          <div
            key={index}
            className="flex items-center justify-between gap-6 border-b py-5"
          >
            <div className="grid gap-2">
              <Skeleton className="h-4 w-28" />
              <Skeleton className="h-3 w-48" />
            </div>
            <Skeleton className="h-8 w-72 rounded-[6px]" />
          </div>
        ))}
      </div>
    </div>
  );
}

export function StartSettingsPanel({
  sourceLanguage,
  detectedSourceLanguage,
  targetLanguage,
  providers,
  models,
  assistants,
  glossaries,
  providerId,
  modelId,
  assistantId,
  config,
  loading,
  onSourceLanguageChange,
  onTargetLanguageChange,
  onProviderChange,
  onModelChange,
  onAssistantChange,
  onConfigChange,
  onNumberChange,
}: StartSettingsPanelProps) {
  const [activeTab, setActiveTab] = useState<SettingsTab>("translation");
  const selectedGlossaryValue = useMemo(
    () => glossarySelectedValue(config, glossaries),
    [config, glossaries],
  );

  function updateGlossaryEnabled(enabled: boolean): void {
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

  function updateGlossarySelection(value: string): void {
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

  return (
    <section className="grid min-h-80 grid-cols-[13rem_minmax(0,1fr)] overflow-hidden rounded-[6px] border bg-card max-[900px]:grid-cols-1">
      <SettingsNav activeTab={activeTab} onTabChange={setActiveTab} />
      <div className="min-w-0 px-4 pb-28">
        {activeTab === "translation" && (
          <>
            <SettingRow
              label="原始语言"
              description="源语言将自动识别"
            >
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
            </SettingRow>
            <SettingRow label="目标语言">
              <LanguageCombobox
                value={targetLanguage}
                disabled={loading}
                className="w-full"
                onValueChange={onTargetLanguageChange}
                placeholder="选择目标语言"
                searchPlaceholder="搜索目标语言"
              />
            </SettingRow>
            <SettingRow label="模型提供商">
              <Select value={providerId} onValueChange={onProviderChange} disabled={loading}>
                <SelectTrigger>
                  <SelectValue placeholder="选择提供商" />
                </SelectTrigger>
                <SelectContent>
                  {providers.map((provider) => (
                    <SelectItem key={provider.id} value={provider.id}>
                      <span className="flex items-center gap-2">
                        <ProviderAvatar
                          name={provider.name}
                          avatar={provider.avatar}
                          size="2xs"
                        />
                        <span>{provider.name}</span>
                      </span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </SettingRow>
            <SettingRow label="翻译模型">
              <Select
                value={modelId}
                onValueChange={onModelChange}
                disabled={loading || models.length === 0}
              >
                <SelectTrigger>
                  <SelectValue placeholder="选择模型" />
                </SelectTrigger>
                <SelectContent>
                  {models.map((model) => (
                    <SelectItem key={model.id} value={model.id}>
                      {model.alias || model.requestName}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </SettingRow>
            <SettingRow label="助手配置">
              <Select value={assistantId} onValueChange={onAssistantChange} disabled={loading}>
                <SelectTrigger>
                  <SelectValue placeholder="选择助手" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="__none__">
                    <span className="flex items-center gap-2">
                      <Bot className="size-4 text-muted-foreground" />
                      <span>不使用助手</span>
                    </span>
                  </SelectItem>
                  {assistants.map((assistant) => (
                    <SelectItem key={assistant.id} value={assistant.id}>
                      <span className="flex items-center gap-2">
                        <AssistantIcon
                          kind={assistant.iconKind}
                          value={assistant.iconValue}
                          className="size-4 border-0 bg-transparent text-xs"
                          glyphClassName="size-3.5"
                        />
                        <span>{assistant.name}</span>
                      </span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </SettingRow>
          </>
        )}

        {activeTab === "glossary" && (
          <>
            <SettingRow
              label="使用术语表"
              description="开启后将应用自定义术语"
            >
              <div className="flex justify-end max-[760px]:justify-start">
                <Switch
                  size="sm"
                  checked={config.useGlossary}
                  disabled={loading}
                  onCheckedChange={updateGlossaryEnabled}
                />
              </div>
            </SettingRow>
            <SettingRow label="选择或创建术语表">
              <Select
                value={selectedGlossaryValue}
                disabled={loading || !config.useGlossary}
                onValueChange={updateGlossarySelection}
              >
                <SelectTrigger>
                  <SelectValue placeholder="选择术语表" />
                </SelectTrigger>
                <SelectContent viewportClassName="max-h-72">
                  <SelectItem value="auto">自动建立术语表</SelectItem>
                  {glossaries.map((glossary) => (
                    <SelectItem key={glossary.id} value={glossary.id}>
                      <span className="flex min-w-0 items-center gap-2">
                        <span className="truncate">{glossary.name}</span>
                        <span className="shrink-0 text-2xs text-muted-foreground">
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
            </SettingRow>
          </>
        )}

        {activeTab === "task" && (
          <>
            <SettingRow
              label="单块 Token 数"
              description="控制每次翻译分块的大小"
            >
              <NumberControl
                value={config.chunkTokenLimit}
                min={200}
                max={8000}
                disabled={loading}
                onChange={(value) => onNumberChange("chunkTokenLimit", value)}
              />
            </SettingRow>
            <SettingRow
              label="最大并发数"
              description="控制并发处理任务的数量"
            >
              <NumberControl
                value={config.maxConcurrency}
                min={1}
                max={32}
                disabled={loading}
                onChange={(value) => onNumberChange("maxConcurrency", value)}
              />
            </SettingRow>
            <SettingRow
              label="最大重试次数"
              description="失败后的重试策略"
            >
              <NumberControl
                value={config.maxRetries}
                min={0}
                max={10}
                disabled={loading}
                onChange={(value) => onNumberChange("maxRetries", value)}
              />
            </SettingRow>
            <SettingRow
              label="动态限流策略"
              description="根据响应头与请求结果自动调整速率"
            >
              <Select
                value={config.rateLimitStrategy}
                disabled={loading}
                onValueChange={(value) => onConfigChange((current) => ({
                  ...current,
                  rateLimitStrategy: value as RateLimitStrategy,
                }))}
              >
                <SelectTrigger>
                  <Gauge className="size-3.5 text-primary" />
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {RATE_LIMIT_OPTIONS.map((option) => (
                    <SelectItem key={option.value} value={option.value}>
                      <span className="grid">
                        <span>{option.label}</span>
                        <span className="text-2xs text-muted-foreground">
                          {option.description}
                        </span>
                      </span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </SettingRow>
            {config.rateLimitStrategy === "manual" && (
              <>
                <SettingRow label="每分钟最大请求数">
                  <NumberControl
                    value={config.maxRequestsPerMinute}
                    min={1}
                    max={1_000_000}
                    disabled={loading}
                    onChange={(value) => onNumberChange("maxRequestsPerMinute", value)}
                  />
                </SettingRow>
                <SettingRow label="每分钟 Token 数">
                  <NumberControl
                    value={config.maxTokensPerMinute}
                    min={1}
                    max={100_000_000}
                    disabled={loading}
                    onChange={(value) => onNumberChange("maxTokensPerMinute", value)}
                  />
                </SettingRow>
              </>
            )}
          </>
        )}

        {activeTab === "proofreading" && (
          <SettingRow
            label="综合置信度检测"
            description="视提供商的支持情况而定，不支持时默认忽略"
          >
            <div className="flex justify-end max-[760px]:justify-start">
              <Switch
                size="sm"
                checked={config.confidenceMode === "confidence-index"}
                disabled={loading}
                onCheckedChange={(checked) => onConfigChange((current) => ({
                  ...current,
                  confidenceMode: checked ? "confidence-index" : "off",
                }))}
              />
            </div>
          </SettingRow>
        )}
      </div>
    </section>
  );
}
