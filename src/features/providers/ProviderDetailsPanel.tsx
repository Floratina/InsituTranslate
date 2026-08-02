import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Braces, KeyRound, Network, Wrench } from "lucide-react";

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
import { SECONDARY_PAGE_FADE_UP_STYLE } from "@/lib/motion";

import {
  getMinerUConfig,
  isMinerUProvider,
  withMinerUConfig,
} from "./mineru";
import { ProviderAvatar } from "./ProviderAvatar";
import { ProtocolConfigFields } from "./ProtocolConfigFields";
import { validateProviderConfig } from "./providerConfigSchema";
import { ProviderModelList } from "./ProviderModelList";
import { VertexAiConfigPanel } from "./VertexAiConfigPanel";
import type {
  MinerUMode,
  ModelView,
  ProtocolDescriptor,
  ProtocolEndpointPreview,
  ProviderDraft,
  ProviderView,
} from "./types";
import type { UpdateVertexAiConfigInput } from "./vertexAi";

interface ProviderDetailsPanelProps {
  provider: ProviderView | null;
  draft: ProviderDraft | null;
  protocolDisplayName: string;
  protocolDescriptor: ProtocolDescriptor | null;
  testingModelId: string;
  onDraftChange: (draft: ProviderDraft) => void;
  onEnabledChange: (provider: ProviderView, enabled: boolean) => void;
  onOpenCredential: () => void;
  onOpenHeaders: () => void;
  onOpenRemoteModels: () => void;
  onAddModel: () => void;
  onTestModel: (model: ModelView) => void;
  onOpenModelSettings: (model: ModelView) => void;
  onOpenServiceAccountJson: () => void;
  onOpenPrivateKey: () => void;
  onOpenProtocolRepair: () => void;
  onUpdateVertexAiConfig: (input: UpdateVertexAiConfigInput) => Promise<void>;
  onError: (message: string) => void;
}

const BASE_URL_HELP_TEXT = "在末尾添加“#”会以当前输入为完整路径";

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
  protocolDisplayName,
  protocolDescriptor,
  testingModelId,
  onDraftChange,
  onEnabledChange,
  onOpenCredential,
  onOpenHeaders,
  onOpenRemoteModels,
  onAddModel,
  onTestModel,
  onOpenModelSettings,
  onOpenServiceAccountJson,
  onOpenPrivateKey,
  onOpenProtocolRepair,
  onUpdateVertexAiConfig,
  onError,
}: ProviderDetailsPanelProps) {
  const [endpointPreview, setEndpointPreview] = useState<ProtocolEndpointPreview | null>(null);
  const [endpointPreviewError, setEndpointPreviewError] = useState<string | null>(null);

  useEffect(() => {
    if (
      !provider
      || !draft
      || provider.protocolStatus !== "available"
      || isMinerUProvider(provider)
      || !draft.baseUrl.trim()
    ) {
      setEndpointPreview(null);
      setEndpointPreviewError(null);
      return;
    }
    let active = true;
    void invoke<ProtocolEndpointPreview>("preview_protocol_endpoints", {
      input: {
        protocol: provider.protocol,
        baseUrl: draft.baseUrl,
        useRawBaseUrl: draft.useRawBaseUrl,
        config: draft.config,
      },
    })
      .then((preview) => {
        if (!active) return;
        setEndpointPreview(preview);
        setEndpointPreviewError(null);
      })
      .catch((cause: unknown) => {
        if (!active) return;
        setEndpointPreview(null);
        setEndpointPreviewError(cause instanceof Error ? cause.message : String(cause));
      });
    return () => {
      active = false;
    };
  }, [draft, provider]);

  if (!provider || !draft) {
    return (
      <div className="flex h-full min-h-80 flex-col items-center justify-center gap-2 text-muted-foreground">
        <Network className="size-10" strokeWidth={1.8} />
        <div className="text-sm">选择或添加一个提供商</div>
      </div>
    );
  }

  const isMinerU = isMinerUProvider(provider);
  const protocolAvailable = provider.protocolStatus === "available";
  const isVertexAi = protocolDescriptor?.configKind === "vertex-ai";
  const requiresCredential = protocolDescriptor?.auth.kind !== "none";
  const mineruConfig = getMinerUConfig(draft.config);
  const activeMinerUBaseUrl =
    mineruConfig.mode === "flash" ? mineruConfig.flashBaseUrl : draft.baseUrl;
  const effectiveConfigIssues = protocolDescriptor
    ? validateProviderConfig(protocolDescriptor.configFields, draft.config)
    : provider.configIssues;

  return (
    <div
      key={provider.id}
      style={SECONDARY_PAGE_FADE_UP_STYLE}
      className="app-fade-up-enter flex min-h-0 flex-1 flex-col"
    >
      <div className="flex shrink-0 items-start justify-between gap-3 border-b p-3">
        <div className="flex min-w-0 items-center gap-3">
          <ProviderAvatar name={provider.name} avatar={provider.avatar} size="lg" />
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <h2 className="min-w-0 truncate text-base font-semibold">{provider.name}</h2>
            <Badge variant={protocolAvailable ? "outline" : "destructive"} className="text-xs">
              {isMinerU ? "MinerU Document Parsing" : protocolDisplayName}
            </Badge>
            {protocolDescriptor?.helpText && (
              <HelpTooltip contentClassName="max-w-96">{protocolDescriptor.helpText}</HelpTooltip>
            )}
          </div>
        </div>
        <Switch
          className="self-center"
          checked={provider.enabled}
          disabled={!protocolAvailable || effectiveConfigIssues.length > 0}
          onCheckedChange={(checked) => onEnabledChange(provider, checked)}
        />
      </div>

      <ScrollArea className="min-h-0 flex-1">
        <div className="grid gap-3 p-3">
          {!protocolAvailable && (
            <div className="flex items-center justify-between gap-3 rounded-[6px] border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
              <span>此提供商引用了当前版本无法识别的协议。修复前不会发送任何网络请求。</span>
              <Button variant="outline" size="control-sm" onClick={onOpenProtocolRepair}>
                <Wrench className="size-3.5" />
                修复协议
              </Button>
            </div>
          )}
          <section className="grid min-w-0 gap-2 rounded-[6px] border p-3">
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
                  <Button disabled={!protocolAvailable} className="min-w-0" variant="outline" size="control-sm" onClick={onOpenCredential}>
                    <KeyRound className="size-3.5" />
                    管理 API Key
                  </Button>
                  <Button disabled={!protocolAvailable} className="min-w-0" variant="outline" size="control-sm" onClick={onOpenHeaders}>
                    <Braces className="size-3.5" />
                    自定义请求头
                  </Button>
                </div>
              </>
            ) : isVertexAi ? (
              <VertexAiConfigPanel
                provider={provider}
                draft={draft}
                onDraftChange={onDraftChange}
                onOpenHeaders={onOpenHeaders}
                onOpenServiceAccountJson={onOpenServiceAccountJson}
                onOpenPrivateKey={onOpenPrivateKey}
                onUpdateConfig={onUpdateVertexAiConfig}
                onError={onError}
              />
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
                    {!protocolAvailable
                      ? "未知协议无法生成请求路径预览"
                      : endpointPreviewError
                        ? `无法预览：${endpointPreviewError}`
                        : endpointPreview
                          ? `聊天：${endpointPreview.chat}`
                          : "正在生成请求路径预览…"}
                    {endpointPreview?.models && (
                      <div>模型：{endpointPreview.models}</div>
                    )}
                  </div>
                </div>
                <ProtocolConfigFields
                  fields={protocolDescriptor?.configFields ?? []}
                  config={draft.config}
                  onChange={(config) => onDraftChange({ ...draft, config })}
                />
                <div className="grid grid-cols-[minmax(0,1fr)_auto_auto] items-end gap-2 max-[820px]:grid-cols-1">
                  {requiresCredential ? (
                    <div className="grid gap-1">
                      <div className="flex items-center gap-1.5">
                        <Label className="text-sm">{protocolDescriptor?.auth.label ?? "API Key"}</Label>
                        {protocolDescriptor?.auth.helpText && (
                          <HelpTooltip contentClassName="max-w-96">
                            {protocolDescriptor.auth.helpText}
                          </HelpTooltip>
                        )}
                      </div>
                      <Input disabled value={provider.credentialMask ? "•••••••••••••••••••••••••••••••••" : `尚未配置${protocolDescriptor?.auth.label ?? " API Key"}`} />
                    </div>
                  ) : (
                    <div className="flex min-h-9 items-center text-sm text-muted-foreground">
                      {protocolDescriptor?.auth.label ?? "此协议无需凭证"}
                    </div>
                  )}
                  {requiresCredential && (
                    <Button disabled={!protocolAvailable} className="min-w-0" variant="outline" size="control-sm" onClick={onOpenCredential}>
                      <KeyRound className="size-3.5" />
                      设置 API Key
                    </Button>
                  )}
                  <Button disabled={!protocolAvailable} className="min-w-0" variant="outline" size="control-sm" onClick={onOpenHeaders}>
                    <Braces className="size-3.5" />
                    自定义请求头
                  </Button>
                </div>
              </>
            )}
          </section>

          <ProviderModelList
            models={provider.models}
            testingModelId={testingModelId}
            disabled={!protocolAvailable || effectiveConfigIssues.length > 0}
            supportsModelListing={protocolDescriptor?.supportsModelListing ?? false}
            onOpenRemoteModels={onOpenRemoteModels}
            onAddModel={onAddModel}
            onTestModel={onTestModel}
            onOpenSettings={onOpenModelSettings}
            variant={isMinerU ? "mineru" : "default"}
          />
        </div>
      </ScrollArea>
    </div>
  );
}
