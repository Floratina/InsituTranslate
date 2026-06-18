import {
  Bot,
  ChevronRight,
  Code2,
  Pencil,
  Save,
  SlidersHorizontal,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useMemo, useRef, useState } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { HelpTooltip } from "@/components/ui/help-tooltip";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import { PURPOSES as PROVIDER_PURPOSES } from "@/features/providers/constants";

import { AssistantIdentityDialog } from "./AssistantIdentityDialog";
import {
  CUSTOM_PARAMETER_PRESET_GROUPS,
  CUSTOM_PARAMETER_PRESETS,
} from "./constants";
import { deepMerge, isRecord } from "./customParameters";
import { AssistantIcon } from "./AssistantIcon";
import { AssistantPromptDialog } from "./AssistantPromptDialog";
import type {
  AssistantSettingsDraft,
  AssistantToolMode,
  AssistantView,
  CustomParameterPreset,
} from "./types";

interface AssistantDetailsPanelProps {
  assistant: AssistantView | null;
  settings: AssistantSettingsDraft | null;
  promptDraft: string;
  customParametersDraft: string;
  customParametersDirty: boolean;
  savingPrompt: boolean;
  savingCustomParameters: boolean;
  onSettingsChange: (settings: AssistantSettingsDraft) => void;
  onCustomParametersChange: (value: string, warnImmediately?: boolean) => void;
  onSavePrompt: (value?: string) => Promise<boolean>;
  onSaveCustomParameters: () => void;
  onError: (message: string) => void;
}

const sectionTransition = {
  duration: 0.24,
  ease: [0.03, 0.59, 0.19, 1] as const,
};

function formatTruncatedDecimal(value: number, precision: number): string {
  const factor = 10 ** precision;
  const truncated = Math.trunc(value * factor) / factor;
  return truncated.toFixed(precision);
}

function purposeLabel(purpose: AssistantView["purpose"]): string {
  return PROVIDER_PURPOSES.find((item) => item.value === purpose)?.label ?? purpose;
}

function compactPromptPreview(value: string): string {
  return value.trim().replace(/\s+/g, " ");
}

function SamplingControl({
  label,
  help,
  enabled,
  value,
  displayPrecision,
  min,
  max,
  step,
  onEnabledChange,
  onValueChange,
}: {
  label: string;
  help: string;
  enabled: boolean;
  value: number;
  displayPrecision: number;
  min: number;
  max: number;
  step: number;
  onEnabledChange: (enabled: boolean) => void;
  onValueChange: (value: number) => void;
}) {
  return (
    <div className="grid gap-2 rounded-[6px] border p-3">
      <div className="flex items-center justify-between gap-3">
        <div className="flex items-center gap-1">
          <Label>{label}</Label>
          <HelpTooltip>{help}</HelpTooltip>
        </div>
        <div className="flex items-center gap-2">
          <span className="min-w-8 text-right text-2xs text-muted-foreground">
            {formatTruncatedDecimal(value, displayPrecision)}
          </span>
          <Switch checked={enabled} onCheckedChange={onEnabledChange} />
        </div>
      </div>
      <Slider
        disabled={!enabled}
        min={min}
        max={max}
        step={step}
        value={[value]}
        onValueChange={(values) => onValueChange(values[0] ?? value)}
      />
    </div>
  );
}

export function AssistantDetailsPanel({
  assistant,
  settings,
  promptDraft,
  customParametersDraft,
  customParametersDirty,
  savingPrompt,
  savingCustomParameters,
  onSettingsChange,
  onCustomParametersChange,
  onSavePrompt,
  onSaveCustomParameters,
  onError,
}: AssistantDetailsPanelProps) {
  const [customOpen, setCustomOpen] = useState(false);
  const [identityOpen, setIdentityOpen] = useState(false);
  const [promptOpen, setPromptOpen] = useState(false);
  const customTextareaRef = useRef<HTMLTextAreaElement | null>(null);
  const promptPreview = compactPromptPreview(promptDraft);
  const availablePresets = useMemo(
    () =>
      CUSTOM_PARAMETER_PRESETS.filter(
        (preset) =>
          !preset.purposes ||
          (assistant !== null && preset.purposes.includes(assistant.purpose)),
      ),
    [assistant],
  );
  const presetGroups = useMemo(
    () =>
      CUSTOM_PARAMETER_PRESET_GROUPS.filter((group) =>
        availablePresets.some((preset) => preset.group === group),
      ),
    [availablePresets],
  );

  if (!assistant || !settings) {
    return (
      <div className="flex h-full min-h-80 flex-col items-center justify-center gap-2 text-muted-foreground">
        <Bot className="size-10" strokeWidth={1.8} />
        <div className="text-sm">选择或添加一个助手</div>
      </div>
    );
  }

  function insertPreset(preset: CustomParameterPreset): void {
    try {
      const parsed: unknown = customParametersDraft.trim()
        ? JSON.parse(customParametersDraft)
        : {};
      if (!isRecord(parsed)) {
        onError("自定义参数必须是 JSON 对象");
        return;
      }
      const formatted = `${JSON.stringify(deepMerge(parsed, preset.value), null, 2)}\n`;
      onCustomParametersChange(formatted, true);
      window.requestAnimationFrame(() => {
        const textarea = customTextareaRef.current;
        if (!textarea) return;
        textarea.focus();
        textarea.setSelectionRange(formatted.length, formatted.length);
      });
    } catch {
      onError("当前自定义参数不是有效 JSON，无法插入预设");
    }
  }

  return (
    <motion.div
      key={assistant.id}
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={sectionTransition}
      className="flex min-h-0 flex-1 flex-col"
    >
      <div className="flex shrink-0 items-start justify-between gap-3 border-b p-3">
        <div className="flex min-w-0 items-center gap-3">
          <AssistantIcon
            kind={settings.iconKind}
            value={settings.iconValue}
            className="size-9 text-sm"
            glyphClassName="size-4 text-base"
          />
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <h2 className="min-w-0 truncate text-base font-semibold">
              {settings.name.trim() || assistant.name}
            </h2>
            <Badge variant="outline" className="text-xs">
              {purposeLabel(assistant.purpose)}
            </Badge>
          </div>
        </div>
        <Button
          size="icon-sm"
          variant="outline"
          className="self-center"
          onClick={() => setIdentityOpen(true)}
          aria-label="编辑助手名称和图标"
          title="编辑助手名称和图标"
        >
          <Pencil className="size-3.5" />
        </Button>
      </div>

      <ScrollArea className="min-h-0 flex-1">
        <div className="grid gap-3 p-3 [&_[data-slot=label]]:text-sm">
          <section className="grid gap-1.5 rounded-[6px] border p-3">
            <Label>系统提示词</Label>
            <button
              type="button"
              className={cn(
                "flex h-8 w-full items-center rounded-[6px] border border-input bg-transparent px-2.5 text-left text-sm outline-none transition-colors duration-150 hover:border-ring focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/40 dark:bg-input/30",
                promptPreview ? "text-foreground" : "text-muted-foreground",
              )}
              onClick={() => setPromptOpen(true)}
            >
              <span className="truncate">
                {promptPreview || "点击编辑系统提示词"}
              </span>
            </button>
          </section>

          <div className="grid grid-cols-2 gap-3 max-[920px]:grid-cols-1">
            <SamplingControl
              label="模型温度"
              help="控制输出的随机性。关闭时使用提供商模型的默认设置。"
              enabled={settings.temperatureEnabled}
              value={settings.temperature}
              displayPrecision={1}
              min={0}
              max={2}
              step={0.1}
              onEnabledChange={(temperatureEnabled) =>
                onSettingsChange({ ...settings, temperatureEnabled })
              }
              onValueChange={(temperature) =>
                onSettingsChange({ ...settings, temperature })
              }
            />
            <SamplingControl
              label="Top-P"
              help="控制模型从累计概率范围内采样。关闭时使用提供商模型的默认设置。"
              enabled={settings.topPEnabled}
              value={settings.topP}
              displayPrecision={2}
              min={0}
              max={1}
              step={0.05}
              onEnabledChange={(topPEnabled) =>
                onSettingsChange({ ...settings, topPEnabled })
              }
              onValueChange={(topP) => onSettingsChange({ ...settings, topP })}
            />
          </div>

          <section className="grid gap-2 rounded-[6px] border p-3">
            <div className="grid grid-cols-2 gap-2 max-[820px]:grid-cols-1">
              <div className="flex flex-col gap-1">
                <Label className="h-5">工具调用方式</Label>
                <Select
                  value={settings.toolMode}
                  onValueChange={(value) =>
                    onSettingsChange({
                      ...settings,
                      toolMode: value as AssistantToolMode,
                    })
                  }
                >
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="function">函数</SelectItem>
                    <SelectItem value="prompt">提示词</SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <div className="flex flex-col gap-1">
                <div className="flex h-5 items-center gap-1">
                  <Label>最大工具调用次数</Label>
                  <HelpTooltip>
                    每个翻译分块允许的最大工具调用次数。值越高Token消耗越大。
                  </HelpTooltip>
                </div>
                <Input
                  type="number"
                  min={0}
                  step={1}
                  value={settings.maxToolCalls}
                  onChange={(event) =>
                    onSettingsChange({
                      ...settings,
                      maxToolCalls: Math.max(
                        0,
                        Math.trunc(Number(event.target.value) || 0),
                      ),
                    })
                  }
                />
                <div className="px-1 text-2xs text-muted-foreground">
                  填写 0 表示禁用工具调用
                </div>
              </div>
            </div>
          </section>

          <section className="overflow-hidden rounded-[6px] border">
            <button
              type="button"
              className="flex h-9 w-full items-center justify-between px-3 text-left transition-colors duration-150 hover:bg-accent/60"
              onClick={() => setCustomOpen((open) => !open)}
            >
              <span className="flex items-center gap-2 text-sm font-medium">
                <Code2 className="size-4" strokeWidth={1.8} />
                自定义参数
              </span>
              <ChevronRight
                className={cn(
                  "size-4 text-muted-foreground transition-transform duration-200",
                  customOpen && "rotate-90",
                )}
              />
            </button>
            <AnimatePresence initial={false}>
              {customOpen && (
                <motion.div
                  initial={{ height: 0, opacity: 0 }}
                  animate={{ height: "auto", opacity: 1 }}
                  exit={{ height: 0, opacity: 0 }}
                  transition={sectionTransition}
                  className="overflow-hidden border-t"
                >
                  <div className="grid gap-2 p-3">
                    <div className="flex items-center justify-between gap-2">
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button size="sm" variant="outline">
                            <SlidersHorizontal className="size-3.5" />
                            插入预设
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent className="w-72 p-0">
                          <div className="scrollbar-subtle max-h-80 overflow-x-hidden overflow-y-auto overscroll-contain">
                            <div className="p-1">
                              {presetGroups.map((group, groupIndex) => (
                                <div
                                  key={group}
                                  className={cn(
                                    groupIndex > 0 && "mt-0.5 border-t pt-0.5",
                                  )}
                                >
                                  <div className="rounded-[6px] bg-muted/55 px-2 py-1 text-2xs font-semibold text-muted-foreground">
                                    {group}
                                  </div>
                                  <div className="mt-0.5 grid">
                                    {availablePresets
                                      .filter((preset) => preset.group === group)
                                      .map((preset) => (
                                        <DropdownMenuItem
                                          key={`${preset.group}-${preset.label}`}
                                          className="h-auto items-start rounded-[6px] px-2 py-1.5"
                                          onSelect={() => insertPreset(preset)}
                                        >
                                          <div className="min-w-0">
                                            <div className="text-sm font-medium">
                                              {preset.label}
                                            </div>
                                            <div className="whitespace-normal break-words text-2xs leading-4 text-muted-foreground">
                                              {preset.description}
                                            </div>
                                          </div>
                                        </DropdownMenuItem>
                                      ))}
                                  </div>
                                </div>
                              ))}
                            </div>
                          </div>
                        </DropdownMenuContent>
                      </DropdownMenu>
                      <Button
                        size="icon-sm"
                        disabled={!customParametersDirty || savingCustomParameters}
                        onClick={onSaveCustomParameters}
                        aria-label="保存自定义参数"
                        title="保存自定义参数"
                      >
                        <Save className="size-3.5" />
                      </Button>
                    </div>
                    <Textarea
                      ref={customTextareaRef}
                      spellCheck={false}
                      className="min-h-40 resize-y font-mono text-xs"
                      value={customParametersDraft}
                      onChange={(event) => onCustomParametersChange(event.target.value)}
                    />
                  </div>
                </motion.div>
              )}
            </AnimatePresence>
          </section>
        </div>
      </ScrollArea>

      <AssistantIdentityDialog
        open={identityOpen}
        settings={settings}
        onOpenChange={setIdentityOpen}
        onSave={onSettingsChange}
      />
      <AssistantPromptDialog
        open={promptOpen}
        value={promptDraft}
        saving={savingPrompt}
        onOpenChange={setPromptOpen}
        onSave={onSavePrompt}
      />
    </motion.div>
  );
}
