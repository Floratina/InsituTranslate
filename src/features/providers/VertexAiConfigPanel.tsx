import { useEffect, useMemo, useState } from "react";
import { Check, ChevronDown, Info, Search, Braces, KeyRound } from "lucide-react";

import { ActionCallout } from "@/components/ui/action-callout";
import { Button } from "@/components/ui/button";
import { HelpTooltip } from "@/components/ui/help-tooltip";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { ScrollArea } from "@/components/ui/scroll-area";
import { selectItemClassName, selectTriggerClassName } from "@/components/ui/select";
import { cn } from "@/lib/utils";

import type { ProviderDraft, ProviderView } from "./types";
import {
  getVertexAiConfig,
  type UpdateVertexAiConfigInput,
  VERTEX_AI_DEFAULT_LOCATION,
  VERTEX_AI_LOCATIONS,
  vertexAiPreviewBaseUrl,
} from "./vertexAi";

interface VertexAiConfigPanelProps {
  provider: ProviderView;
  draft: ProviderDraft;
  onDraftChange: (draft: ProviderDraft) => void;
  onOpenHeaders: () => void;
  onOpenServiceAccountJson: () => void;
  onOpenPrivateKey: () => void;
  onUpdateConfig: (input: UpdateVertexAiConfigInput) => Promise<void>;
  onError: (message: string) => void;
}

interface LocalVertexConfig {
  projectId: string;
  location: string;
  clientEmail: string;
}

const BASE_URL_HELP_TEXT = "在末尾添加“#”会以当前输入为完整路径";

function BaseUrlLabel() {
  return (
    <div className="flex items-center gap-1.5">
      <Label className="text-sm">Base URL</Label>
      <HelpTooltip contentClassName="max-w-80">{BASE_URL_HELP_TEXT}</HelpTooltip>
    </div>
  );
}

function UrlPreview({ value }: { value: string }) {
  const [prefix, url] = value.startsWith("预览: ")
    ? ["预览: ", value.slice("预览: ".length)]
    : ["", value];
  const parts = url.split(/([/?&])/g);

  return (
    <div className="px-1 text-2xs leading-4 text-muted-foreground/70">
      {prefix}
      <span className="break-words">
        {parts.map((part, index) => (
          <span key={`${part}-${index}`}>
            {part}
            {part === "/" || part === "?" || part === "&" ? <wbr /> : null}
          </span>
        ))}
      </span>
    </div>
  );
}

function VertexLocationCombobox({
  value,
  onValueChange,
}: {
  value: string;
  onValueChange: (value: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const options = useMemo(() => {
    const current = value.trim();
    const base = current && !VERTEX_AI_LOCATIONS.some((item) => item.value === current)
      ? [{ value: current, label: current }, ...VERTEX_AI_LOCATIONS]
      : VERTEX_AI_LOCATIONS;
    const normalized = query.trim().toLocaleLowerCase();
    return normalized
      ? base.filter((item) => item.value.toLocaleLowerCase().includes(normalized))
      : base;
  }, [query, value]);

  function selectValue(nextValue: string): void {
    onValueChange(nextValue);
    setOpen(false);
    setQuery("");
  }

  return (
    <Popover
      open={open}
      onOpenChange={(nextOpen) => {
        setOpen(nextOpen);
        if (!nextOpen) setQuery("");
      }}
    >
      <PopoverTrigger asChild>
        <button
          type="button"
          className={cn(selectTriggerClassName, "justify-between font-normal")}
        >
          <span className="min-w-0 truncate text-left">
            {value.trim() || VERTEX_AI_DEFAULT_LOCATION}
          </span>
          <ChevronDown className="size-4 shrink-0 text-muted-foreground" strokeWidth={1.8} />
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        collisionPadding={12}
        sideOffset={4}
        className="!w-80 max-w-[calc(100vw-1.5rem)] overflow-hidden p-0"
      >
        <div className="border-b p-2">
          <div className="relative">
            <Search className="pointer-events-none absolute top-2 left-2.5 size-3.5 text-muted-foreground" />
            <Input
              autoFocus
              value={query}
              placeholder="搜索地区"
              className="pl-8"
              onChange={(event) => setQuery(event.target.value)}
            />
          </div>
        </div>
        <ScrollArea className="h-64">
          {options.length === 0 ? (
            <div className="px-2 py-4 text-center text-xs text-muted-foreground">
              没有匹配的地区
            </div>
          ) : (
            <div>
              {options.map((option) => (
                <button
                  key={option.value}
                  type="button"
                  className={cn(selectItemClassName, "font-normal")}
                  onClick={() => selectValue(option.value)}
                >
                  <span className="truncate">{option.label}</span>
                  <Check
                    className={cn(
                      "absolute right-3 size-3.5",
                      value === option.value ? "opacity-100" : "opacity-0",
                    )}
                  />
                </button>
              ))}
            </div>
          )}
        </ScrollArea>
      </PopoverContent>
    </Popover>
  );
}

export function VertexAiConfigPanel({
  provider,
  draft,
  onDraftChange,
  onOpenHeaders,
  onOpenServiceAccountJson,
  onOpenPrivateKey,
  onUpdateConfig,
  onError,
}: VertexAiConfigPanelProps) {
  const savedConfig = useMemo(() => getVertexAiConfig(draft.config), [draft.config]);
  const [localConfig, setLocalConfig] = useState<LocalVertexConfig>(savedConfig);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    setLocalConfig(savedConfig);
  }, [provider.id, savedConfig]);

  async function saveConfig(
    nextConfig: LocalVertexConfig = localConfig,
    nextPrivateKey?: string | null,
  ): Promise<void> {
    setSaving(true);
    try {
      await onUpdateConfig({
        providerId: provider.id,
        projectId: nextConfig.projectId,
        location: nextConfig.location || VERTEX_AI_DEFAULT_LOCATION,
        clientEmail: nextConfig.clientEmail,
        ...(nextPrivateKey !== undefined ? { privateKey: nextPrivateKey } : {}),
      });
      onError("");
    } catch (cause) {
      onError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSaving(false);
    }
  }

  function updateLocalConfig(next: Partial<LocalVertexConfig>): LocalVertexConfig {
    const merged = { ...localConfig, ...next };
    setLocalConfig(merged);
    return merged;
  }

  return (
    <>
      <div className="grid gap-1">
        <BaseUrlLabel />
        <Input
          value={draft.baseUrl}
          onChange={(event) =>
            onDraftChange({
              ...draft,
              baseUrl: event.target.value,
              useRawBaseUrl: event.target.value.includes("#"),
            })
          }
        />
        <UrlPreview value={vertexAiPreviewBaseUrl(draft.baseUrl, localConfig)} />
      </div>

      <div className="grid gap-2">
        <ActionCallout
          icon={<Info className="size-3.5" strokeWidth={1.8} />}
          action={
            <Button
              className="min-w-0"
              variant="outline"
              size="control-sm"
              disabled={saving}
              onClick={onOpenServiceAccountJson}
            >
              <KeyRound className="size-3.5" />
              解析服务账号JSON密钥
            </Button>
          }
        >
          使用 Google Cloud Service Account JSON 进行身份验证。
        </ActionCallout>

        <div className="grid grid-cols-[repeat(auto-fit,minmax(13rem,1fr))] gap-2">
          <div className="grid gap-1">
            <Label className="text-sm">项目 ID</Label>
            <Input
              value={localConfig.projectId}
              placeholder="Google Cloud 项目 ID"
              onChange={(event) => updateLocalConfig({ projectId: event.target.value })}
              onBlur={() => void saveConfig()}
            />
          </div>
          <div className="grid gap-1">
            <Label className="text-sm">客户端邮箱</Label>
            <Input
              value={localConfig.clientEmail}
              placeholder="client_email"
              onChange={(event) => updateLocalConfig({ clientEmail: event.target.value })}
              onBlur={() => void saveConfig()}
            />
          </div>
        </div>

        <div className="grid grid-cols-[minmax(0,1fr)_auto_minmax(13rem,16rem)] items-end gap-2 max-[900px]:grid-cols-1">
          <div className="grid gap-1">
            <Label className="text-sm">私钥</Label>
            <Input
              disabled
              value={
                provider.credentialMask
                  ? "•••••••••••••••••••••••••••••••••"
                  : "尚未配置 private_key"
              }
            />
          </div>
          <Button
            className="min-w-0"
            variant="outline"
            size="control-sm"
            disabled={saving}
            onClick={onOpenPrivateKey}
          >
            <KeyRound className="size-3.5" />
            修改私钥
          </Button>
          <div className="grid content-start gap-1">
            <Label className="text-sm">地区</Label>
            <VertexLocationCombobox
              value={localConfig.location || VERTEX_AI_DEFAULT_LOCATION}
              onValueChange={(location) => {
                const next = updateLocalConfig({ location });
                void saveConfig(next);
              }}
            />
          </div>
        </div>
      </div>

      <div className="flex justify-end">
        <Button
          className="min-w-0"
          variant="outline"
          size="control-sm"
          disabled={saving}
          onClick={onOpenHeaders}
        >
          <Braces className="size-3.5" />
          自定义请求头
        </Button>
      </div>
    </>
  );
}
