import { Braces, KeyRound, Network } from "lucide-react";
import { motion } from "motion/react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import { Switch } from "@/components/ui/switch";

import {
  getMinerUConfig,
  isMinerUProvider,
  withMinerUConfig,
} from "./mineru";
import { ProviderAvatar } from "./ProviderAvatar";
import { ProviderModelList } from "./ProviderModelList";
import type { MinerUMode, ModelView, ProviderDraft, ProviderProtocol, ProviderView } from "./types";

interface ProviderDetailsPanelProps {
  provider: ProviderView | null;
  draft: ProviderDraft | null;
  testingModelId: string;
  onDraftChange: (draft: ProviderDraft) => void;
  onEnabledChange: (provider: ProviderView, enabled: boolean) => void;
  onOpenCredential: () => void;
  onOpenHeaders: () => void;
  onOpenRemoteModels: () => void;
  onAddModel: () => void;
  onTestModel: (model: ModelView) => void;
  onOpenModelSettings: (model: ModelView) => void;
}

const panelTransition = { duration: 0.28, ease: [0.03, 0.59, 0.19, 1] as const };
const BASE_URL_HELP_TEXT = "在末尾添加“#”会以当前输入为完整路径";
const GEMINI_KEY_TYPE_HELP_TEXT =
  "Google 正在从标准 API 密钥切换到授权 (auth) 密钥，2026 年 9 月起将拒绝任何标准密钥的请求，InsituTranslate 已经适配。详情参考 Gemini API 文档。";

function protocolLabel(protocol: ProviderProtocol): string {
  const labels: Record<ProviderProtocol, string> = {
    "openai-chat": "OpenAI Chat Completions",
    "openai-responses": "OpenAI Responses",
    anthropic: "Anthropic Messages",
    gemini: "Gemini API",
    ollama: "Ollama Chat",
  };
  return labels[protocol];
}

function splitBaseUrlMarker(baseUrl: string): { base: string; markerRaw: boolean } {
  const markerIndex = baseUrl.indexOf("#");
  if (markerIndex === -1) {
    return { base: baseUrl, markerRaw: false };
  }
  return { base: baseUrl.slice(0, markerIndex), markerRaw: true };
}

function appendPreviewEndpoint(baseUrl: string, suffix: string): string {
  return `${baseUrl}${suffix}`;
}

function isVersionedBase(baseUrl: string): boolean {
  try {
    const path = new URL(baseUrl).pathname.replace(/\/+$/, "");
    return /\/v\d+$/i.test(path);
  } catch {
    return /\/v\d+\/?$/i.test(baseUrl);
  }
}

function baseUrlPreview(baseUrl: string, protocol: ProviderProtocol, useRawBaseUrl: boolean): string {
  const { base, markerRaw } = splitBaseUrlMarker(baseUrl);
  const raw = useRawBaseUrl || markerRaw;
  if (!base.trim()) return "请先填写 Base URL";
  if (protocol === "ollama") {
    const suffix = raw ? "/chat" : base.endsWith("/api") ? "/chat" : "/api/chat";
    return `预览: ${appendPreviewEndpoint(base, suffix)}`;
  }
  if (protocol === "anthropic") {
    return `预览: ${appendPreviewEndpoint(base, raw ? "/messages" : "/v1/messages")}`;
  }
  if (protocol === "gemini") {
    return `预览: ${appendPreviewEndpoint(base, raw ? "/models/{model}:generateContent" : "/v1beta/models/{model}:generateContent")}`;
  }
  const versioned = isVersionedBase(base);
  return protocol === "openai-responses"
    ? `预览: ${appendPreviewEndpoint(base, raw || versioned ? "/responses" : "/v1/responses")}`
    : `预览: ${appendPreviewEndpoint(base, raw || versioned ? "/chat/completions" : "/v1/chat/completions")}`;
}

function mineruBasePreview(baseUrl: string, mode: MinerUMode): string[] {
  const { base } = splitBaseUrlMarker(baseUrl);
  if (!base.trim()) return ["请先填写 Base URL"];
  if (mode === "flash") {
    return [`预览: ${appendPreviewEndpoint(base, "/parse/{taskId}")}`];
  }
  return [
    `预览: ${appendPreviewEndpoint(base, "/file-urls/batch")}`,
    `预览: ${appendPreviewEndpoint(base, "/extract-results/batch/{batchId}")}`,
  ];
}

function BaseUrlLabel({ label }: { label: string }) {
  return (
    <div className="flex items-center gap-1.5">
      <Label className="text-sm">{label}</Label>
      <HelpTooltip contentClassName="max-w-80">{BASE_URL_HELP_TEXT}</HelpTooltip>
    </div>
  );
}

function updateMinerUDraft(
  draft: ProviderDraft,
  next: Partial<ReturnType<typeof getMinerUConfig>>,
): ProviderDraft {
  return {
    ...draft,
    config: withMinerUConfig(draft.config, next),
  };
}

export function ProviderDetailsPanel({
  provider,
  draft,
  testingModelId,
  onDraftChange,
  onEnabledChange,
  onOpenCredential,
  onOpenHeaders,
  onOpenRemoteModels,
  onAddModel,
  onTestModel,
  onOpenModelSettings,
}: ProviderDetailsPanelProps) {
  if (!provider || !draft) {
    return (
      <div className="flex h-full min-h-80 flex-col items-center justify-center gap-2 text-muted-foreground">
        <Network className="size-10" strokeWidth={1.8} />
        <div className="text-sm">选择或添加一个提供商</div>
      </div>
    );
  }

  const isMinerU = isMinerUProvider(provider);
  const isGemini = provider.protocol === "gemini" && !isMinerU;
  const mineruConfig = getMinerUConfig(draft.config);
  const activeMinerUBaseUrl =
    mineruConfig.mode === "flash" ? mineruConfig.flashBaseUrl : draft.baseUrl;

  return (
    <motion.div
      key={provider.id}
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={panelTransition}
      className="flex min-h-0 flex-1 flex-col"
    >
      <div className="flex shrink-0 items-start justify-between gap-3 border-b p-3">
        <div className="flex min-w-0 items-center gap-3">
          <ProviderAvatar name={provider.name} avatar={provider.avatar} size="lg" />
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <h2 className="min-w-0 truncate text-base font-semibold">{provider.name}</h2>
            <Badge variant="outline" className="text-xs">
              {isMinerU ? "MinerU Document Parsing" : protocolLabel(provider.protocol)}
            </Badge>
            {isGemini && (
              <HelpTooltip contentClassName="max-w-96">{GEMINI_KEY_TYPE_HELP_TEXT}</HelpTooltip>
            )}
          </div>
        </div>
        <Switch
          className="self-center"
          checked={provider.enabled}
          onCheckedChange={(checked) => onEnabledChange(provider, checked)}
        />
      </div>

      <ScrollArea className="min-h-0 flex-1">
        <div className="grid gap-3 p-3">
          <motion.section
            initial={{ opacity: 0, y: 12 }}
            animate={{ opacity: 1, y: 0 }}
            transition={panelTransition}
            className="grid min-w-0 gap-2 rounded-[6px] border p-3"
          >
            {isMinerU ? (
              <>
                <div className="grid grid-cols-[minmax(9rem,11.25rem)_minmax(0,1fr)] items-start gap-3 max-[820px]:grid-cols-1">
                  <div className="grid gap-1">
                    <Label className="text-sm">解析模式</Label>
                    <Select
                      value={mineruConfig.mode}
                      onValueChange={(value) =>
                        onDraftChange(updateMinerUDraft(draft, { mode: value as MinerUMode }))
                      }
                    >
                      <SelectTrigger className="h-9">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="standard">Standard v4</SelectItem>
                        <SelectItem value="flash">Flash no-auth</SelectItem>
                      </SelectContent>
                    </Select>
                    <div aria-hidden className="min-h-8" />
                  </div>
                  <div className="grid gap-1">
                    <BaseUrlLabel
                      label={mineruConfig.mode === "flash" ? "Flash Base URL" : "Standard Base URL"}
                    />
                    <Input
                      className="h-9"
                      value={activeMinerUBaseUrl}
                      onChange={(event) => {
                        const baseUrl = event.target.value;
                        if (mineruConfig.mode === "flash") {
                          onDraftChange(
                            updateMinerUDraft(draft, { flashBaseUrl: baseUrl }),
                          );
                          return;
                        }
                        onDraftChange({
                          ...draft,
                          baseUrl,
                          useRawBaseUrl: true,
                        });
                      }}
                    />
                    <div className="grid min-h-8 gap-0.5 px-1 text-2xs leading-4 text-muted-foreground/70">
                      {mineruBasePreview(activeMinerUBaseUrl, mineruConfig.mode).map((line) => (
                        <div key={line} className="break-all">
                          {line}
                        </div>
                      ))}
                    </div>
                  </div>
                </div>
                <div className="grid grid-cols-[minmax(0,1fr)_auto_auto] items-end gap-2 max-[820px]:grid-cols-1">
                  <div className="grid gap-1">
                    <Label className="text-sm">API Key</Label>
                    <Input
                      disabled
                      value={
                        mineruConfig.mode === "flash"
                          ? "Flash 模式无需 API Key"
                          : provider.credentialMask
                            ? "•••••••••••••••••••••••••••••••••"
                            : "Standard 模式尚未配置 API Key"
                      }
                    />
                  </div>
                  <Button className="min-w-0" variant="outline" size="control-sm" onClick={onOpenCredential}>
                    <KeyRound className="size-3.5" />
                    管理 API Key
                  </Button>
                  <Button className="min-w-0" variant="outline" size="control-sm" onClick={onOpenHeaders}>
                    <Braces className="size-3.5" />
                    自定义请求头
                  </Button>
                </div>
              </>
            ) : (
              <>
                <div className="grid gap-1">
                  <BaseUrlLabel label="Base URL" />
                  <Input
                    value={draft.baseUrl}
                    onChange={(event) => {
                      const baseUrl = event.target.value;
                      onDraftChange({
                        ...draft,
                        baseUrl,
                        useRawBaseUrl: splitBaseUrlMarker(baseUrl).markerRaw,
                      });
                    }}
                  />
                  <div className="break-all px-1 text-2xs text-muted-foreground/70">
                    {baseUrlPreview(draft.baseUrl, provider.protocol, draft.useRawBaseUrl)}
                  </div>
                </div>
                <div className="grid grid-cols-[minmax(0,1fr)_auto_auto] items-end gap-2 max-[820px]:grid-cols-1">
                  <div className="grid gap-1">
                    <Label className="text-sm">API Key</Label>
                    <Input disabled value={provider.credentialMask ? "•••••••••••••••••••••••••••••••••" : "尚未配置 API Key"} />
                  </div>
                  <Button className="min-w-0" variant="outline" size="control-sm" onClick={onOpenCredential}>
                    <KeyRound className="size-3.5" />
                    设置 API Key
                  </Button>
                  <Button className="min-w-0" variant="outline" size="control-sm" onClick={onOpenHeaders}>
                    <Braces className="size-3.5" />
                    自定义请求头
                  </Button>
                </div>
              </>
            )}
          </motion.section>

          <ProviderModelList
            models={provider.models}
            testingModelId={testingModelId}
            onOpenRemoteModels={onOpenRemoteModels}
            onAddModel={onAddModel}
            onTestModel={onTestModel}
            onOpenSettings={onOpenModelSettings}
            variant={isMinerU ? "mineru" : "default"}
          />
        </div>
      </ScrollArea>
    </motion.div>
  );
}
