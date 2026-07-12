import { Bot } from "lucide-react";
import { useCallback, useEffect, useMemo } from "react";

import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { AssistantIcon } from "@/features/assistants/AssistantIcon";
import type { AssistantView } from "@/features/assistants/types";
import { ProviderAvatar } from "@/features/providers/ProviderAvatar";
import type { ModelView, ProviderView } from "@/features/providers/types";
import type { ThinkingEffort } from "@/features/translation/types";
import { cn } from "@/lib/utils";

export interface ModelRuntimeSettingsValue {
  providerId: string;
  modelId: string;
  assistantId: string | null;
  thinkingEffort: ThinkingEffort;
  useWebSearch: boolean;
  useCustomParameters: boolean;
}

interface ModelRuntimeSettingsProps {
  value: ModelRuntimeSettingsValue;
  providers: ProviderView[];
  assistants: AssistantView[];
  providerLabel: string;
  modelLabel: string;
  assistantLabel: string;
  disabled?: boolean;
  className?: string;
  onChange: (value: ModelRuntimeSettingsValue) => void;
}

interface FieldBlockProps {
  label: string;
  children: React.ReactNode;
}

const THINKING_EFFORT_OPTIONS: Array<{ value: ThinkingEffort; label: string }> = [
  { value: "none", label: "None" },
  { value: "minimal", label: "Minimal" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
  { value: "xhigh", label: "Xhigh" },
  { value: "max", label: "Max" },
];

function FieldBlock({ label, children }: FieldBlockProps) {
  return (
    <div className="grid min-w-0 content-start gap-1.5">
      <div className="text-sm font-medium text-foreground">{label}</div>
      {children}
    </div>
  );
}

export function supportedThinkingEffortsForModel(model: ModelView | null): ThinkingEffort[] {
  if (!model?.capabilityReasoning) return ["none"];
  if (model.supportedThinkingEfforts.length === 0) {
    return THINKING_EFFORT_OPTIONS.map((option) => option.value);
  }
  return model.supportedThinkingEfforts.includes("none")
    ? model.supportedThinkingEfforts
    : ["none", ...model.supportedThinkingEfforts];
}

function normalizeCapabilities(
  value: ModelRuntimeSettingsValue,
  model: ModelView | null,
): ModelRuntimeSettingsValue {
  if (!model) return value;
  const supportedThinkingEfforts = supportedThinkingEffortsForModel(model);
  return {
    ...value,
    thinkingEffort: supportedThinkingEfforts.includes(value.thinkingEffort)
      ? value.thinkingEffort
      : "none",
    useWebSearch: model.capabilityWeb ? value.useWebSearch : false,
  };
}

function runtimeSettingsEqual(
  left: ModelRuntimeSettingsValue,
  right: ModelRuntimeSettingsValue,
): boolean {
  return left.providerId === right.providerId
    && left.modelId === right.modelId
    && left.assistantId === right.assistantId
    && left.thinkingEffort === right.thinkingEffort
    && left.useWebSearch === right.useWebSearch
    && left.useCustomParameters === right.useCustomParameters;
}

export function useModelRuntimeSettings({
  value,
  providers,
  disabled,
  onChange,
}: Pick<ModelRuntimeSettingsProps, "value" | "providers" | "disabled" | "onChange">) {
  const selectedProvider = useMemo(
    () => providers.find((provider) => provider.id === value.providerId) ?? null,
    [providers, value.providerId],
  );
  const models = selectedProvider?.models ?? [];
  const selectedModel = useMemo(
    () => models.find((model) => model.id === value.modelId) ?? null,
    [models, value.modelId],
  );
  const supportedThinkingEfforts = useMemo(
    () => supportedThinkingEffortsForModel(selectedModel),
    [selectedModel],
  );

  useEffect(() => {
    if (disabled || !selectedModel) return;
    const normalized = normalizeCapabilities(value, selectedModel);
    if (!runtimeSettingsEqual(normalized, value)) onChange(normalized);
  }, [disabled, onChange, selectedModel, value]);

  const changeProvider = useCallback((providerId: string): void => {
    const provider = providers.find((item) => item.id === providerId) ?? null;
    const model = provider?.models[0] ?? null;
    onChange(normalizeCapabilities({
      ...value,
      providerId,
      modelId: model?.id ?? "",
    }, model));
  }, [onChange, providers, value]);

  const changeModel = useCallback((modelId: string): void => {
    const model = models.find((item) => item.id === modelId) ?? null;
    onChange(normalizeCapabilities({ ...value, modelId }, model));
  }, [models, onChange, value]);

  return {
    selectedProvider,
    selectedModel,
    models,
    supportedThinkingEfforts,
    reasoningAvailable: supportedThinkingEfforts.some((effort) => effort !== "none"),
    webSearchAvailable: Boolean(selectedModel?.capabilityWeb),
    changeProvider,
    changeModel,
  };
}

export function ModelRuntimeSettings({
  value,
  providers,
  assistants,
  providerLabel,
  modelLabel,
  assistantLabel,
  disabled = false,
  className,
  onChange,
}: ModelRuntimeSettingsProps) {
  const {
    selectedProvider,
    selectedModel,
    models,
    supportedThinkingEfforts,
    reasoningAvailable,
    webSearchAvailable,
    changeProvider,
    changeModel,
  } = useModelRuntimeSettings({ value, providers, disabled, onChange });
  const assistantValue = value.assistantId ?? "__none__";
  const selectedAssistantExists = value.assistantId === null
    || assistants.some((assistant) => assistant.id === value.assistantId);

  return (
    <div className={cn("grid grid-cols-1 gap-3 min-[1120px]:grid-cols-3", className)}>
      <FieldBlock label={providerLabel}>
        <Select value={value.providerId} onValueChange={changeProvider} disabled={disabled}>
          <SelectTrigger>
            <SelectValue placeholder="选择提供商" />
          </SelectTrigger>
          <SelectContent>
            {value.providerId && !selectedProvider && (
              <SelectItem value={value.providerId} disabled>已失效的提供商</SelectItem>
            )}
            {providers.map((provider) => (
              <SelectItem key={provider.id} value={provider.id}>
                <span className="flex items-center gap-2">
                  <ProviderAvatar name={provider.name} avatar={provider.avatar} size="2xs" />
                  <span>{provider.name}</span>
                </span>
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </FieldBlock>

      <FieldBlock label={modelLabel}>
        <Select
          value={value.modelId}
          onValueChange={changeModel}
          disabled={disabled || !selectedProvider || models.length === 0}
        >
          <SelectTrigger>
            <SelectValue placeholder="选择模型" />
          </SelectTrigger>
          <SelectContent>
            {value.modelId && !selectedModel && (
              <SelectItem value={value.modelId} disabled>已失效的模型</SelectItem>
            )}
            {models.map((model) => (
              <SelectItem key={model.id} value={model.id}>
                {model.alias || model.requestName}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </FieldBlock>

      <FieldBlock label={assistantLabel}>
        <Select
          value={assistantValue}
          onValueChange={(assistantId) => onChange({
            ...value,
            assistantId: assistantId === "__none__" ? null : assistantId,
          })}
          disabled={disabled}
        >
          <SelectTrigger>
            <SelectValue placeholder="选择配置" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="__none__">
              <span className="flex items-center gap-2">
                <Bot className="size-4 text-muted-foreground" />
                <span>不使用配置</span>
              </span>
            </SelectItem>
            {!selectedAssistantExists && value.assistantId && (
              <SelectItem value={value.assistantId} disabled>已失效的配置</SelectItem>
            )}
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
      </FieldBlock>

      <FieldBlock label="思考强度">
        <Select
          value={supportedThinkingEfforts.includes(value.thinkingEffort)
            ? value.thinkingEffort
            : "none"}
          onValueChange={(thinkingEffort) => onChange({
            ...value,
            thinkingEffort: thinkingEffort as ThinkingEffort,
          })}
          disabled={disabled || !selectedModel || !reasoningAvailable}
        >
          <SelectTrigger>
            <SelectValue placeholder="选择思考强度" />
          </SelectTrigger>
          <SelectContent>
            {THINKING_EFFORT_OPTIONS.filter((option) =>
              supportedThinkingEfforts.includes(option.value)
            ).map((option) => (
              <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>
            ))}
          </SelectContent>
        </Select>
      </FieldBlock>

      <FieldBlock label="联网搜索">
        <div className="flex h-8 items-center">
          <Switch
            size="sm"
            checked={value.useWebSearch}
            disabled={disabled || !selectedModel || !webSearchAvailable}
            onCheckedChange={(useWebSearch) => onChange({ ...value, useWebSearch })}
          />
        </div>
      </FieldBlock>

      <FieldBlock label="自定义参数">
        <div className="flex h-8 items-center">
          <Switch
            size="sm"
            checked={value.useCustomParameters}
            disabled={disabled}
            onCheckedChange={(useCustomParameters) => onChange({
              ...value,
              useCustomParameters,
            })}
          />
        </div>
      </FieldBlock>
    </div>
  );
}
