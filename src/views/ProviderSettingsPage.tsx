import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Reorder } from "motion/react";
import {
  Brain,
  FileCheck2,
  Globe2,
  Languages,
  Minus,
  Network,
  Plus,
  ScanText,
  Search,
  Trash2,
  Wrench,
  X,
  BookOpen,
  type LucideIcon,
} from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogField,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  SurfaceList,
  SurfaceListItem,
} from "@/components/ui/surface-list";
import { Textarea } from "@/components/ui/textarea";
import { useToast } from "@/components/ui/toast-stack";
import { appSessionCache } from "@/lib/session-cache";
import { cn } from "@/lib/utils";
import { AvatarPickerPopover } from "@/features/providers/AvatarPickerPopover";
import { EMPTY_MODEL_FORM, EMPTY_PROVIDER_FORM } from "@/features/providers/constants";
import { getMinerUConfig, isMinerUProvider } from "@/features/providers/mineru";
import { ProviderDetailsPanel } from "@/features/providers/ProviderDetailsPanel";
import { ProviderListItem } from "@/features/providers/ProviderListItem";
import { useProviderEnabledToggle } from "@/features/providers/useProviderEnabledToggle";
import {
  getVertexAiConfig,
  type ImportVertexAiServiceAccountInput,
  type UpdateVertexAiConfigInput,
} from "@/features/providers/vertexAi";
import type {
  ConnectivityResult,
  ModelView,
  NewModelForm,
  ProviderDraft,
  ProviderForm,
  ProviderProtocol,
  ProviderPurpose,
  ProviderView,
  RemoteModel,
} from "@/features/providers/types";

interface PurposeOption {
  value: ProviderPurpose;
  label: string;
  icon: IconName;
}

const ICONS = {
  add: Plus,
  capabilities: Wrench,
  close: X,
  delete: Trash2,
  documentParsing: ScanText,
  glossary: BookOpen,
  proofreading: FileCheck2,
  reasoning: Brain,
  remove: Minus,
  search: Search,
  translation: Languages,
  web: Globe2,
} satisfies Record<string, LucideIcon>;

type IconName = keyof typeof ICONS;

const PURPOSES: PurposeOption[] = [
  { value: "translation", label: "翻译", icon: "translation" },
  { value: "glossary", label: "术语表", icon: "glossary" },
  { value: "proofreading", label: "校对", icon: "proofreading" },
  { value: "document-parsing", label: "文档解析", icon: "documentParsing" },
];

function Icon({
  name,
  className,
}: {
  name: IconName;
  className?: string;
}) {
  const IconComponent = ICONS[name];
  return <IconComponent className={cn("size-4 shrink-0", className)} strokeWidth={1.8} />;
}

function getErrorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

function protocolLabel(protocol: ProviderProtocol): string {
  const labels: Record<ProviderProtocol, string> = {
    "openai-chat": "OpenAI Chat Completions",
    "openai-responses": "OpenAI Responses",
    anthropic: "Anthropic Messages",
    gemini: "Gemini API",
    "vertex-ai": "Agent Platform (Vertex AI)",
    ollama: "Ollama Chat",
  };
  return labels[protocol];
}

function purposeLabel(purpose: ProviderPurpose): string {
  return PURPOSES.find((item) => item.value === purpose)?.label ?? purpose;
}

function isTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

function hasRawBaseMarker(baseUrl: string): boolean {
  return baseUrl.includes("#");
}

function baseUrlBeforeMarker(baseUrl: string): string {
  return baseUrl.split("#", 1)[0] ?? "";
}

function displayBaseUrl(provider: ProviderView): string {
  return provider.baseUrl;
}

function draftForSave(draft: ProviderDraft): ProviderDraft {
  return {
    ...draft,
    useRawBaseUrl: draft.useRawBaseUrl || hasRawBaseMarker(draft.baseUrl),
  };
}

function hasValidDraftUrls(draft: ProviderDraft): boolean {
  try {
    new URL(baseUrlBeforeMarker(draft.baseUrl).trim());
    const mineru = draft.config.mineru;
    if (mineru) {
      new URL(baseUrlBeforeMarker(getMinerUConfig(draft.config).flashBaseUrl).trim());
    }
    return true;
  } catch {
    return false;
  }
}

function CapabilityBadge({
  icon,
  label,
  active,
  onClick,
}: {
  icon: IconName;
  label: string;
  active: boolean;
  onClick?: () => void;
}) {
  if (!onClick) {
    return (
      <span
        className={cn(
          "inline-flex h-6 items-center gap-1 rounded-[6px] border px-2 text-2xs text-muted-foreground",
          active &&
            "border-enabled-accent/30 bg-enabled-accent/15 text-enabled-accent",
        )}
      >
        <Icon name={icon} className="size-3" />
        {label}
      </span>
    );
  }
  return (
    <Button
      type="button"
      size="xs"
      variant={active ? "accent" : "outline"}
      aria-pressed={active}
      className="text-2xs"
      onClick={onClick}
    >
      <Icon name={icon} className="text-sm" />
      {label}
    </Button>
  );
}

function ProviderListSkeleton() {
  return (
    <div className="grid gap-1">
      {Array.from({ length: 5 }).map((_, index) => (
        <div key={index} className="flex h-12 items-center gap-2 rounded-[6px] p-2">
          <Skeleton className="size-8 shrink-0" />
          <div className="grid min-w-0 flex-1 gap-1.5">
            <Skeleton className="h-4 w-28" />
            <Skeleton className="h-3 w-16" />
          </div>
          <Skeleton className="h-5 w-12" />
        </div>
      ))}
    </div>
  );
}

function ProviderDetailsSkeleton() {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center justify-between gap-3 border-b p-3">
        <div className="flex items-center gap-3">
          <Skeleton className="size-9" />
          <div className="grid gap-2">
            <Skeleton className="h-4 w-36" />
            <Skeleton className="h-4 w-32" />
          </div>
        </div>
        <Skeleton className="h-5 w-9 rounded-full" />
      </div>
      <div className="grid gap-3 p-3">
        <div className="grid gap-2 rounded-[6px] border p-3">
          <Skeleton className="h-4 w-20" />
          <Skeleton className="h-9 w-full" />
          <Skeleton className="h-4 w-2/3" />
          <div className="grid grid-cols-[minmax(0,1fr)_auto_auto] gap-2 max-[820px]:grid-cols-1">
            <Skeleton className="h-9 w-full" />
            <Skeleton className="h-9 w-28" />
            <Skeleton className="h-9 w-32" />
          </div>
        </div>
        <div className="grid gap-2 rounded-[6px] border p-3">
          <Skeleton className="h-5 w-32" />
          {Array.from({ length: 4 }).map((_, index) => (
            <Skeleton key={index} className="h-9 w-full" />
          ))}
        </div>
      </div>
    </div>
  );
}

function ProviderSettingsPage() {
  const cachedProviders = appSessionCache.providers("translation").read();
  const cachedSelectedProviderId =
    appSessionCache.providerSelectedIds.get("translation") ?? "";
  const [purpose, setPurpose] = useState<ProviderPurpose>("translation");
  const [providers, setProviders] = useState<ProviderView[]>(cachedProviders ?? []);
  const [selectedProviderId, setSelectedProviderId] = useState<string>(
    cachedProviders?.some((provider) => provider.id === cachedSelectedProviderId)
      ? cachedSelectedProviderId
      : (cachedProviders?.[0]?.id ?? ""),
  );
  const [providerDraft, setProviderDraft] = useState<ProviderDraft | null>(null);
  const [loading, setLoading] = useState<boolean>(!cachedProviders);
  const [busy, setBusy] = useState<boolean>(false);
  const { pushToast } = useToast();
  const setError = useCallback(
    (message: string): void => {
      if (message) pushToast(message, "error");
    },
    [pushToast],
  );

  const [addProviderOpen, setAddProviderOpen] = useState<boolean>(false);
  const [editingProviderId, setEditingProviderId] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<ProviderView | null>(null);
  const [providerForm, setProviderForm] =
    useState<ProviderForm>(EMPTY_PROVIDER_FORM);
  const [credentialOpen, setCredentialOpen] = useState<boolean>(false);
  const [credentialValue, setCredentialValue] = useState<string>("");
  const [headersOpen, setHeadersOpen] = useState<boolean>(false);
  const [headersJson, setHeadersJson] = useState<string>("");
  const [serviceAccountOpen, setServiceAccountOpen] = useState<boolean>(false);
  const [serviceAccountJson, setServiceAccountJson] = useState<string>("");
  const [privateKeyOpen, setPrivateKeyOpen] = useState<boolean>(false);
  const [privateKeyValue, setPrivateKeyValue] = useState<string>("");
  const [privateKeyLoading, setPrivateKeyLoading] = useState<boolean>(false);
  const [remoteModelsOpen, setRemoteModelsOpen] = useState<boolean>(false);
  const [remoteModels, setRemoteModels] = useState<RemoteModel[]>([]);
  const [remoteModelsLoading, setRemoteModelsLoading] =
    useState<boolean>(false);
  const [remoteModelSearch, setRemoteModelSearch] = useState<string>("");
  const [addModelOpen, setAddModelOpen] = useState<boolean>(false);
  const [modelForm, setModelForm] =
    useState<NewModelForm>(EMPTY_MODEL_FORM);
  const [settingsModel, setSettingsModel] = useState<ModelView | null>(null);
  const [testingModelId, setTestingModelId] = useState<string>("");
  const providerDraftBaseline = useRef<string>("");
  const autoSaveTimer = useRef<number | null>(null);
  const providerOrderRef = useRef<string[]>([]);
  const setProvidersAndCache = useCallback(
    (action: ProviderView[] | ((current: ProviderView[]) => ProviderView[])): void => {
      setProviders((current) => {
        const next = typeof action === "function" ? action(current) : action;
        appSessionCache.providers(purpose).set(next);
        return next;
      });
    },
    [purpose],
  );
  const { setEnabledOptimistically, syncProviders } = useProviderEnabledToggle({
    setProviders: setProvidersAndCache,
    onError: setError,
  });

  const selectedProvider = useMemo(
    () => providers.find((provider) => provider.id === selectedProviderId) ?? null,
    [providers, selectedProviderId],
  );
  const selectedProviderIsMinerU = useMemo(
    () => isMinerUProvider(selectedProvider),
    [selectedProvider],
  );

  const filteredRemoteModels = useMemo<RemoteModel[]>(() => {
    const query = remoteModelSearch.trim().toLocaleLowerCase();
    if (!query) return remoteModels;
    return remoteModels.filter((model) =>
      `${model.alias} ${model.requestName}`.toLocaleLowerCase().includes(query),
    );
  }, [remoteModelSearch, remoteModels]);

  useEffect(() => {
    if (selectedProviderId) {
      appSessionCache.providerSelectedIds.set(purpose, selectedProviderId);
    } else {
      appSessionCache.providerSelectedIds.delete(purpose);
    }
  }, [purpose, selectedProviderId]);

  useEffect(() => {
    void refreshProviders();
  }, [purpose]);

  useEffect(() => {
    if (!selectedProvider) {
      providerDraftBaseline.current = "";
      setProviderDraft(null);
      return;
    }
    const nextDraft: ProviderDraft = {
      id: selectedProvider.id,
      baseUrl: displayBaseUrl(selectedProvider),
      useRawBaseUrl: selectedProvider.useRawBaseUrl,
      config: selectedProvider.config ?? {},
    };
    setProviderDraft((currentDraft) => {
      const currentDraftJson = currentDraft ? JSON.stringify(currentDraft) : "";
      if (
        currentDraft?.id === selectedProvider.id &&
        providerDraftBaseline.current === currentDraftJson
      ) {
        return currentDraft;
      }
      providerDraftBaseline.current = JSON.stringify(nextDraft);
      return nextDraft;
    });
  }, [selectedProvider]);

  useEffect(() => {
    if (
      !providerDraft ||
      !isTauriRuntime() ||
      providerDraftBaseline.current === JSON.stringify(providerDraft) ||
      !providerDraft.baseUrl.trim() ||
      !hasValidDraftUrls(providerDraft)
    ) {
      return;
    }
    if (autoSaveTimer.current !== null) {
      window.clearTimeout(autoSaveTimer.current);
    }
    autoSaveTimer.current = window.setTimeout(() => {
      void saveProvider(providerDraft);
    }, 500);
    return () => {
      if (autoSaveTimer.current !== null) {
        window.clearTimeout(autoSaveTimer.current);
      }
    };
  }, [providerDraft]);

  async function refreshProviders(preferredId?: string, force = false): Promise<void> {
    const resource = appSessionCache.providers(purpose);
    const cached = force ? undefined : resource.read();
    if (cached) {
      providerOrderRef.current = cached.map((provider) => provider.id);
      syncProviders(cached);
      setProviders(cached);
      setSelectedProviderId((current) => {
        const preferred =
          preferredId ?? current ?? appSessionCache.providerSelectedIds.get(purpose);
        return cached.some((item) => item.id === preferred)
          ? preferred
          : (cached[0]?.id ?? "");
      });
      setLoading(false);
      setError("");
      return;
    }

    setLoading(true);
    try {
      if (!isTauriRuntime()) {
        resource.set([]);
        setProviders([]);
        setSelectedProviderId("");
        setError("");
        return;
      }
      const result = await (force
        ? resource.refresh(() => invoke<ProviderView[]>("list_providers", { purpose }))
        : resource.loadOnce(() => invoke<ProviderView[]>("list_providers", { purpose })));
      providerOrderRef.current = result.map((provider) => provider.id);
      syncProviders(result);
      setProviders(result);
      setSelectedProviderId((current) => {
        const preferred =
          preferredId ?? current ?? appSessionCache.providerSelectedIds.get(purpose);
        return result.some((item) => item.id === preferred)
          ? preferred
          : (result[0]?.id ?? "");
      });
      setError("");
    } catch (cause) {
      setError(getErrorMessage(cause));
    } finally {
      setLoading(false);
    }
  }

  function flash(message: string): void {
    pushToast(message);
  }

  async function createProvider(): Promise<void> {
    setBusy(true);
    try {
      const created = await invoke<ProviderView>("create_provider", {
        input: {
          ...providerForm,
          purpose,
        },
      });
      setAddProviderOpen(false);
      setProviderForm(EMPTY_PROVIDER_FORM);
      await refreshProviders(created.id, true);
      flash("提供商已添加");
    } catch (cause) {
      setError(getErrorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function saveProvider(nextDraft?: ProviderDraft): Promise<void> {
    const draft = nextDraft ?? providerDraft;
    if (!draft) return;
    const input = draftForSave(draft);
    try {
      const updated = await invoke<ProviderView>("update_provider_config", {
        input,
      });
      providerDraftBaseline.current = JSON.stringify(input);
      setProvidersAndCache((items) =>
        items.map((item) => (item.id === updated.id ? updated : item)),
      );
      setError("");
    } catch (cause) {
      setError(getErrorMessage(cause));
    }
  }

  async function updateVertexAiConfig(input: UpdateVertexAiConfigInput): Promise<void> {
    try {
      const updated = await invoke<ProviderView>("update_vertex_ai_config", {
        input,
      });
      setProvidersAndCache((items) =>
        items.map((item) => (item.id === updated.id ? updated : item)),
      );
      setError("");
    } catch (cause) {
      throw new Error(getErrorMessage(cause));
    }
  }

  async function parseServiceAccountJson(clear = false): Promise<void> {
    if (!selectedProvider) return;
    setBusy(true);
    try {
      const vertexConfig = getVertexAiConfig(selectedProvider.config ?? {});
      const updated = clear
        ? await invoke<ProviderView>("update_vertex_ai_config", {
            input: {
              providerId: selectedProvider.id,
              projectId: vertexConfig.projectId,
              location: vertexConfig.location,
              clientEmail: vertexConfig.clientEmail,
              privateKey: "",
            } satisfies UpdateVertexAiConfigInput,
          })
        : await invoke<ProviderView>("import_vertex_ai_service_account", {
            input: {
              providerId: selectedProvider.id,
              serviceAccountJson: serviceAccountJson.trim(),
              location: vertexConfig.location,
            } satisfies ImportVertexAiServiceAccountInput,
          });
      setProvidersAndCache((items) =>
        items.map((item) => (item.id === updated.id ? updated : item)),
      );
      setServiceAccountJson("");
      setServiceAccountOpen(false);
      pushToast(clear ? "服务账号私钥已清空" : "服务账号 JSON 已解析并安全保存", "success");
    } catch (cause) {
      pushToast(getErrorMessage(cause), "error");
    } finally {
      setBusy(false);
    }
  }

  async function openPrivateKeyEditor(): Promise<void> {
    if (!selectedProvider) return;
    setPrivateKeyOpen(true);
    setPrivateKeyValue("");
    setPrivateKeyLoading(true);
    try {
      const value = await invoke<string | null>("get_vertex_ai_private_key", {
        providerId: selectedProvider.id,
      });
      setPrivateKeyValue(value ?? "");
    } catch (cause) {
      pushToast(getErrorMessage(cause), "error");
    } finally {
      setPrivateKeyLoading(false);
    }
  }

  async function saveVertexPrivateKey(clear = false): Promise<void> {
    if (!selectedProvider) return;
    setBusy(true);
    try {
      const vertexConfig = getVertexAiConfig(selectedProvider.config ?? {});
      const updated = await invoke<ProviderView>("update_vertex_ai_config", {
        input: {
          providerId: selectedProvider.id,
          projectId: vertexConfig.projectId,
          location: vertexConfig.location,
          clientEmail: vertexConfig.clientEmail,
          privateKey: clear ? "" : privateKeyValue,
        } satisfies UpdateVertexAiConfigInput,
      });
      setProvidersAndCache((items) =>
        items.map((item) => (item.id === updated.id ? updated : item)),
      );
      setPrivateKeyValue("");
      setPrivateKeyOpen(false);
      pushToast(clear ? "私钥已清空" : "私钥已安全保存", "success");
    } catch (cause) {
      pushToast(getErrorMessage(cause), "error");
    } finally {
      setBusy(false);
    }
  }

  async function deleteProvider(provider: ProviderView): Promise<void> {
    setBusy(true);
    try {
      await invoke("delete_provider", { id: provider.id });
      setDeleteTarget(null);
      await refreshProviders(undefined, true);
      flash("提供商已删除");
    } catch (cause) {
      setError(getErrorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  function openAddProvider(): void {
    setEditingProviderId(null);
    setProviderForm(EMPTY_PROVIDER_FORM);
    setAddProviderOpen(true);
  }

  function openEditProvider(provider: ProviderView): void {
    setEditingProviderId(provider.id);
    setProviderForm({
      name: provider.name,
      protocol: provider.protocol,
      avatar: provider.avatar,
    });
    setAddProviderOpen(true);
  }

  async function saveProviderMetadata(): Promise<void> {
    if (!editingProviderId) {
      await createProvider();
      return;
    }
    setBusy(true);
    try {
      const updated = await invoke<ProviderView>("update_provider_metadata", {
        input: { id: editingProviderId, ...providerForm },
      });
      setProvidersAndCache((items) =>
        items.map((item) => (item.id === updated.id ? updated : item)),
      );
      setAddProviderOpen(false);
      setEditingProviderId(null);
      flash("提供商已更新");
    } catch (cause) {
      setError(getErrorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function copyProvider(provider: ProviderView, targetPurpose: ProviderPurpose): Promise<void> {
    try {
      const copied = await invoke<ProviderView>("copy_provider", {
        input: { providerId: provider.id, purpose: targetPurpose },
      });
      if (targetPurpose === purpose) {
        await refreshProviders(copied.id, true);
      } else {
        appSessionCache.providers(targetPurpose).invalidate();
      }
      flash(`已复制到${purposeLabel(targetPurpose)}`);
    } catch (cause) {
      setError(getErrorMessage(cause));
    }
  }

  function reorderProviders(nextIds: string[]): void {
    providerOrderRef.current = nextIds;
    setProvidersAndCache((current) =>
      nextIds
        .map((id) => current.find((provider) => provider.id === id))
        .filter((provider): provider is ProviderView => provider !== undefined),
    );
  }

  async function persistProviderOrder(): Promise<void> {
    try {
      await invoke<ProviderView[]>("reorder_providers", {
        input: { purpose, providerIds: providerOrderRef.current },
      });
    } catch (cause) {
      setError(getErrorMessage(cause));
      await refreshProviders(selectedProviderId, true);
    }
  }

  async function replaceCredential(clear = false): Promise<void> {
    if (!selectedProvider) return;
    setBusy(true);
    try {
      await invoke("replace_provider_credential", {
        providerId: selectedProvider.id,
        credential: clear ? null : credentialValue,
      });
      setCredentialValue("");
      setCredentialOpen(false);
      await refreshProviders(selectedProvider.id, true);
      flash(clear ? "凭据已清除" : "凭据已安全保存");
    } catch (cause) {
      setError(getErrorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function replaceHeaders(clear = false): Promise<void> {
    if (!selectedProvider) return;
    setBusy(true);
    try {
      await invoke("replace_provider_headers", {
        providerId: selectedProvider.id,
        headersJson: clear ? null : headersJson,
      });
      setHeadersJson("");
      setHeadersOpen(false);
      await refreshProviders(selectedProvider.id, true);
      flash(clear ? "自定义请求头已清除" : "自定义请求头已安全保存");
    } catch (cause) {
      setError(getErrorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function openRemoteModels(): Promise<void> {
    if (!selectedProvider) return;
    setRemoteModelSearch("");
    setRemoteModelsOpen(true);
    setRemoteModelsLoading(true);
    try {
      const result = await invoke<RemoteModel[]>("fetch_provider_models", {
        providerId: selectedProvider.id,
      });
      setRemoteModels(result);
    } catch (cause) {
      setError(getErrorMessage(cause));
    } finally {
      setRemoteModelsLoading(false);
    }
  }

  async function toggleRemoteModel(remote: RemoteModel): Promise<void> {
    if (!selectedProvider) return;
    setBusy(true);
    try {
      if (remote.added) {
        const local = selectedProvider.models.find(
          (model) => model.requestName === remote.requestName,
        );
        if (local) {
          await invoke("delete_model", { id: local.id });
        }
      } else {
        await invoke("add_model", {
          input: {
            providerId: selectedProvider.id,
            requestName: remote.requestName,
            alias: remote.alias,
            source: "remote",
          },
        });
      }
      setRemoteModels((items) =>
        items.map((item) =>
          item.requestName === remote.requestName
            ? { ...item, added: !item.added }
            : item,
        ),
      );
      await refreshProviders(selectedProvider.id, true);
    } catch (cause) {
      setError(getErrorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function addCustomModel(): Promise<void> {
    if (!selectedProvider) return;
    setBusy(true);
    try {
      await invoke("add_model", {
        input: {
          providerId: selectedProvider.id,
          requestName: modelForm.requestName,
          alias: modelForm.alias,
          source: "manual",
        },
      });
      setAddModelOpen(false);
      setModelForm(EMPTY_MODEL_FORM);
      await refreshProviders(selectedProvider.id, true);
      flash("模型已添加");
    } catch (cause) {
      setError(getErrorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function updateSelectedModel(): Promise<void> {
    if (!settingsModel || !selectedProvider) return;
    setBusy(true);
    try {
      await invoke("update_model", {
        input: {
          id: settingsModel.id,
          alias: settingsModel.alias,
          capabilityReasoning: settingsModel.capabilityReasoning,
          capabilityWeb: settingsModel.capabilityWeb,
          capabilityTools: settingsModel.capabilityTools,
        },
      });
      setSettingsModel(null);
      await refreshProviders(selectedProvider.id, true);
      flash("模型设置已保存");
    } catch (cause) {
      setError(getErrorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function deleteSelectedModel(): Promise<void> {
    if (!settingsModel || !selectedProvider) return;
    setBusy(true);
    try {
      await invoke("delete_model", { id: settingsModel.id });
      setSettingsModel(null);
      await refreshProviders(selectedProvider.id, true);
      flash("模型已删除");
    } catch (cause) {
      setError(getErrorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function testModel(model: ModelView): Promise<void> {
    if (!selectedProvider) return;
    setTestingModelId(model.id);
    try {
      const result = await invoke<ConnectivityResult>(
        "test_model_connectivity",
        { modelId: model.id },
      );
      await refreshProviders(selectedProvider.id, true);
      flash(
        result.success
          ? `连接正常，${result.latencyMs}ms`
          : `连接失败：${result.error ?? "未知错误"}`,
      );
    } catch (cause) {
      setError(getErrorMessage(cause));
    } finally {
      setTestingModelId("");
    }
  }

  return (
    <>
      <main className="flex min-w-0 flex-1 flex-col overflow-hidden p-3">
        <header className="mb-3 flex shrink-0 items-end">
          <div>
            <div className="flex items-center gap-2">
              <Network className="size-5 text-primary" />
              <h1 className="text-xl font-medium tracking-tight">提供商</h1>
            </div>
            <p className="mt-0.5 text-xs text-muted-foreground">
              管理不同用途的模型提供商与模型能力
            </p>
          </div>
        </header>

        <div className="grid min-h-0 flex-1 grid-cols-[minmax(13.5rem,16.25rem)_minmax(0,1fr)] gap-3 max-[760px]:grid-cols-1">
          <Card className="min-h-0 gap-0 rounded-[6px] py-0">
            <div className="border-b p-2">
              <Select
                value={purpose}
                onValueChange={(value) => setPurpose(value as ProviderPurpose)}
              >
                <SelectTrigger className="bg-card">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {PURPOSES.map((item) => (
                    <SelectItem key={item.value} value={item.value}>
                      <span className="flex items-center gap-2">
                        <Icon name={item.icon} className="size-3.5" />
                        {item.label}
                      </span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <div className="mt-2 text-xs text-muted-foreground">
                共 {providers.length} 个提供商
              </div>
            </div>
            <ScrollArea className="min-h-0 flex-1">
              <Reorder.Group
                axis="y"
                values={providers.map((provider) => provider.id)}
                onReorder={reorderProviders}
                className="grid gap-1 p-2"
              >
                {loading ? (
                  <ProviderListSkeleton />
                ) : loading ? (
                  <div className="p-3 text-center text-xs text-muted-foreground">
                    正在读取配置…
                  </div>
                ) : providers.length === 0 ? (
                  <div className="rounded-[6px] border border-dashed p-3 text-center text-xs text-muted-foreground">
                    当前用途还没有提供商
                  </div>
                ) : (
                  providers.map((provider) => (
                    <ProviderListItem
                      key={provider.id}
                      provider={provider}
                      selected={selectedProviderId === provider.id}
                      onSelect={() => setSelectedProviderId(provider.id)}
                      onEdit={() => openEditProvider(provider)}
                      onDelete={() => setDeleteTarget(provider)}
                      onCopy={(targetPurpose) => void copyProvider(provider, targetPurpose)}
                      onDragComplete={() => void persistProviderOrder()}
                    />
                  ))
                )}
              </Reorder.Group>
            </ScrollArea>
            <div className="border-t p-2">
              <Button
                className="w-full rounded-[6px]"
                onClick={openAddProvider}
              >
                <Icon name="add" className="text-sm" />
                添加提供商
              </Button>
            </div>
          </Card>

          <Card className="min-h-0 gap-0 rounded-[6px] py-0">
            {loading ? (
              <ProviderDetailsSkeleton />
            ) : (
              <ProviderDetailsPanel
                provider={selectedProvider}
                draft={providerDraft}
                testingModelId={testingModelId}
                onDraftChange={setProviderDraft}
                onEnabledChange={setEnabledOptimistically}
                onOpenCredential={() => setCredentialOpen(true)}
                onOpenHeaders={() => setHeadersOpen(true)}
                onOpenRemoteModels={() => void openRemoteModels()}
                onAddModel={() => setAddModelOpen(true)}
                onTestModel={(model) => void testModel(model)}
                onOpenModelSettings={(model) => setSettingsModel({ ...model })}
                onOpenServiceAccountJson={() => setServiceAccountOpen(true)}
                onOpenPrivateKey={() => void openPrivateKeyEditor()}
                onUpdateVertexAiConfig={updateVertexAiConfig}
                onError={setError}
              />
            )}
          </Card>
        </div>
      </main>

      <Dialog open={addProviderOpen} onOpenChange={setAddProviderOpen}>
        <DialogContent open={addProviderOpen} className="max-w-xl">
          <DialogHeader>
            <DialogTitle>{editingProviderId ? "编辑提供商" : "添加提供商"}</DialogTitle>
            <DialogDescription>
              {editingProviderId
                ? "修改提供商名称、头像与协议。"
                : "添加自定义提供商。新增后可在右侧配置 Base URL 与 API Key。"}
            </DialogDescription>
          </DialogHeader>
          <div className="flex justify-center py-1">
            <AvatarPickerPopover
              name={providerForm.name}
              avatar={providerForm.avatar}
              onAvatarChange={(avatar) =>
                setProviderForm({ ...providerForm, avatar })
              }
              onError={setError}
            />
          </div>
          <div className="grid grid-cols-[repeat(auto-fit,minmax(12rem,1fr))] gap-2">
            <DialogField>
              <Label>名称</Label>
              <Input
                value={providerForm.name}
                onChange={(event) =>
                  setProviderForm({ ...providerForm, name: event.target.value })
                }
              />
            </DialogField>
            <DialogField>
              <Label>协议</Label>
              <Select
                value={providerForm.protocol}
                onValueChange={(value) =>
                  setProviderForm({
                    ...providerForm,
                    protocol: value as ProviderProtocol,
                  })
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {(
                    [
                      "openai-chat",
                      "openai-responses",
                      "anthropic",
                      "gemini",
                      "vertex-ai",
                      "ollama",
                    ] as ProviderProtocol[]
                  ).map((protocol) => (
                    <SelectItem key={protocol} value={protocol}>
                      {protocolLabel(protocol)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </DialogField>
          </div>
          <DialogFooter>
            <Button
              disabled={busy || !providerForm.name.trim()}
              onClick={() => void saveProviderMetadata()}
            >
              {editingProviderId ? "保存修改" : "添加提供商"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <AlertDialog open={deleteTarget !== null} onOpenChange={(open) => !open && setDeleteTarget(null)}>
        <AlertDialogContent open={deleteTarget !== null}>
          <AlertDialogTitle className="font-semibold">
            删除 {deleteTarget?.name}？
          </AlertDialogTitle>
          <AlertDialogDescription className="text-xs text-muted-foreground">
            此操作会同时删除该提供商的全部模型和系统凭据。
          </AlertDialogDescription>
          <div className="flex justify-end gap-2">
            <AlertDialogCancel asChild>
              <Button variant="outline">取消</Button>
            </AlertDialogCancel>
            <AlertDialogAction asChild>
              <Button
                variant="destructive"
                onClick={() => deleteTarget && void deleteProvider(deleteTarget)}
              >
                确认删除
              </Button>
            </AlertDialogAction>
          </div>
        </AlertDialogContent>
      </AlertDialog>

      <Dialog
        open={credentialOpen}
        onOpenChange={(open) => {
          setCredentialOpen(open);
          if (!open) setCredentialValue("");
        }}
      >
        <DialogContent open={credentialOpen}>
          <DialogHeader>
            <DialogTitle>设置 API Key</DialogTitle>
          </DialogHeader>
          <Input
            type="password"
            autoComplete="off"
            placeholder="输入新的 API Key"
            value={credentialValue}
            onChange={(event) => setCredentialValue(event.target.value)}
          />
          <DialogFooter className="justify-between">
            <Button
              variant="destructive"
              onClick={() => void replaceCredential(true)}
            >
              清除 API Key
            </Button>
            <Button
              disabled={!credentialValue || busy}
              onClick={() => void replaceCredential()}
            >
              保存 API Key
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={headersOpen}
        onOpenChange={(open) => {
          setHeadersOpen(open);
          if (!open) setHeadersJson("");
        }}
      >
        <DialogContent open={headersOpen}>
          <DialogHeader>
            <DialogTitle>自定义 JSON 请求头</DialogTitle>
            <DialogDescription>
              该配置应用于此提供商的全部模型。
            </DialogDescription>
          </DialogHeader>
          {selectedProvider && selectedProvider.customHeaderKeys.length > 0 && (
            <div className="rounded-[6px] bg-muted p-2 text-xs text-muted-foreground">
              当前已配置：{selectedProvider.customHeaderKeys.join("、")}
            </div>
          )}
          <Textarea
            className="min-h-40 font-mono font-medium text-xs"
            spellCheck={false}
            value={headersJson}
            placeholder={'{\n  "HTTP-Referer": "https://example.com",\n  "X-Title": "InsituTranslate"\n}'}
            onChange={(event) => setHeadersJson(event.target.value)}
          />
          <DialogFooter className="justify-between">
            <Button
              variant="destructive"
              onClick={() => void replaceHeaders(true)}
            >
              清除请求头
            </Button>
            <Button
              disabled={!headersJson.trim() || busy}
              onClick={() => void replaceHeaders()}
            >
              保存并替换
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={serviceAccountOpen}
        onOpenChange={(open) => {
          setServiceAccountOpen(open);
          if (!open) setServiceAccountJson("");
        }}
      >
        <DialogContent open={serviceAccountOpen} className="overflow-x-hidden">
          <DialogHeader>
            <DialogTitle>解析服务账号JSON密钥</DialogTitle>
            <DialogDescription>
              粘贴 Google Cloud Service Account JSON，解析成功后只安全保存项目 ID、客户端邮箱和私钥。
            </DialogDescription>
          </DialogHeader>
          <Textarea
            wrap="soft"
            className="min-h-48 overflow-x-hidden overflow-y-auto break-all font-mono font-medium text-xs whitespace-pre-wrap"
            spellCheck={false}
            value={serviceAccountJson}
            placeholder={'{\n  "type": "service_account",\n  "project_id": "my-project",\n  "private_key": "-----BEGIN PRIVATE KEY-----\\\\n...\\\\n-----END PRIVATE KEY-----\\\\n",\n  "client_email": "name@my-project.iam.gserviceaccount.com"\n}'}
            onChange={(event) => setServiceAccountJson(event.target.value)}
          />
          <DialogFooter className="justify-between">
            <Button
              variant="destructive"
              disabled={busy || !selectedProvider}
              onClick={() => void parseServiceAccountJson(true)}
            >
              清空密钥
            </Button>
            <Button
              disabled={!serviceAccountJson.trim() || busy}
              onClick={() => void parseServiceAccountJson()}
            >
              解析
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={privateKeyOpen}
        onOpenChange={(open) => {
          setPrivateKeyOpen(open);
          if (!open) {
            setPrivateKeyValue("");
          }
        }}
      >
        <DialogContent open={privateKeyOpen} className="overflow-x-hidden">
          <DialogHeader>
            <DialogTitle>修改私钥</DialogTitle>
            <DialogDescription>
              这里显示当前保存的 private_key。保存后会覆盖原私钥。
            </DialogDescription>
          </DialogHeader>
          <Textarea
            rows={4}
            wrap="soft"
            className="h-24 min-h-24 max-h-24 resize-none overflow-x-hidden overflow-y-auto break-all font-mono font-medium text-xs leading-5 whitespace-pre-wrap [field-sizing:fixed]"
            spellCheck={false}
            value={privateKeyValue}
            placeholder={privateKeyLoading ? "正在读取已保存的私钥…" : "粘贴 private_key 字段"}
            disabled={privateKeyLoading}
            onChange={(event) => setPrivateKeyValue(event.target.value)}
          />
          <DialogFooter className="justify-between">
            <Button
              variant="destructive"
              disabled={busy || privateKeyLoading || !selectedProvider}
              onClick={() => void saveVertexPrivateKey(true)}
            >
              清空私钥
            </Button>
            <Button
              disabled={!privateKeyValue.trim() || busy || privateKeyLoading}
              onClick={() => void saveVertexPrivateKey()}
            >
              保存私钥
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={remoteModelsOpen}
        onOpenChange={(open) => {
          setRemoteModelsOpen(open);
          if (!open) setRemoteModelSearch("");
        }}
      >
        <DialogContent open={remoteModelsOpen} className="max-w-xl">
          <DialogHeader>
            <DialogTitle>上游模型列表</DialogTitle>
            <DialogDescription>
              点击加号添加模型；已添加的模型会高亮显示，点击减号可移除。
            </DialogDescription>
          </DialogHeader>
          <div className="relative">
            <Icon
              name="search"
              className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground"
            />
            <Input
              type="search"
              value={remoteModelSearch}
              placeholder="搜索模型名称或请求名"
              className="pl-8"
              disabled={remoteModelsLoading || remoteModels.length === 0}
              onChange={(event) => setRemoteModelSearch(event.target.value)}
            />
          </div>
          <div className="scrollbar-subtle max-h-[55vh] overflow-x-hidden overflow-y-auto overscroll-contain">
            <div>
              <SurfaceList>
                {remoteModelsLoading ? (
                <div className="p-6 text-center text-xs text-muted-foreground">
                  正在从上游获取模型…
                </div>
              ) : remoteModels.length === 0 ? (
                <div className="p-6 text-center text-xs text-muted-foreground">
                  上游没有返回可用模型
                </div>
              ) : filteredRemoteModels.length === 0 ? (
                <div className="p-6 text-center text-xs text-muted-foreground">
                  没有匹配的模型
                </div>
              ) : (
                filteredRemoteModels.map((model) => (
                  <SurfaceListItem
                    key={model.requestName}
                    className={cn(
                      "justify-between",
                      model.added && "bg-enabled-accent/10",
                    )}
                  >
                    <div className="min-w-0">
                      <div className="truncate text-sm font-medium">
                        {model.alias}
                      </div>
                      <div className="truncate text-2xs text-muted-foreground">
                        {model.requestName}
                      </div>
                    </div>
                    <Button
                      size="icon-sm"
                      variant={model.added ? "secondary" : "outline"}
                      disabled={busy}
                      onClick={() => void toggleRemoteModel(model)}
                    >
                      <Icon
                        name={model.added ? "remove" : "add"}
                        className="text-sm"
                      />
                    </Button>
                  </SurfaceListItem>
                ))
                )}
              </SurfaceList>
            </div>
          </div>
        </DialogContent>
      </Dialog>

      <Dialog open={addModelOpen} onOpenChange={setAddModelOpen}>
        <DialogContent open={addModelOpen}>
          <DialogHeader>
            <DialogTitle>添加自定义模型</DialogTitle>
            <DialogDescription>
              请求名创建后不可修改，请填写供应商实际要求的模型 ID。
            </DialogDescription>
          </DialogHeader>
          <DialogField>
            <Label>模型请求名</Label>
            <Input
              className="font-mono font-medium"
              value={modelForm.requestName}
              placeholder="custom-model-name"
              onChange={(event) =>
                setModelForm({
                  ...modelForm,
                  requestName: event.target.value,
                })
              }
            />
          </DialogField>
          <DialogField>
            <Label>模型别名</Label>
            <Input
              value={modelForm.alias}
              placeholder="用于界面展示的名称"
              onChange={(event) =>
                setModelForm({ ...modelForm, alias: event.target.value })
              }
            />
          </DialogField>
          <DialogFooter>
            <Button
              disabled={!modelForm.requestName.trim() || busy}
              onClick={() => void addCustomModel()}
            >
              添加模型
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={settingsModel !== null}
        onOpenChange={(open) => {
          if (!open) setSettingsModel(null);
        }}
      >
        <DialogContent open={settingsModel !== null}>
          {settingsModel && (
            <>
              <DialogHeader>
                <DialogTitle>模型设置</DialogTitle>
                <DialogDescription>
                  {selectedProviderIsMinerU
                    ? "配置模型别名。模型请求名对应 MinerU model_version，创建后不可修改。"
                    : "配置别名和模型能力。模型请求名创建后不可修改。"}
                </DialogDescription>
              </DialogHeader>
              <DialogField>
                <Label>模型请求名</Label>
                <Input
                  className="font-mono font-medium"
                  disabled
                  value={settingsModel.requestName}
                />
              </DialogField>
              <DialogField>
                <Label>模型别名</Label>
                <Input
                  value={settingsModel.alias}
                  onChange={(event) =>
                    setSettingsModel({
                      ...settingsModel,
                      alias: event.target.value,
                    })
                  }
                />
              </DialogField>
              {!selectedProviderIsMinerU && (
                <DialogField>
                  <Label>模型能力</Label>
                  <div className="flex flex-wrap gap-2">
                    <CapabilityBadge
                      icon="reasoning"
                      label="推理"
                      active={settingsModel.capabilityReasoning}
                      onClick={() =>
                        setSettingsModel({
                          ...settingsModel,
                          capabilityReasoning:
                            !settingsModel.capabilityReasoning,
                        })
                      }
                    />
                    <CapabilityBadge
                      icon="web"
                      label="联网"
                      active={settingsModel.capabilityWeb}
                      onClick={() =>
                        setSettingsModel({
                          ...settingsModel,
                          capabilityWeb: !settingsModel.capabilityWeb,
                        })
                      }
                    />
                    <CapabilityBadge
                      icon="capabilities"
                      label="工具调用"
                      active={settingsModel.capabilityTools}
                      onClick={() =>
                        setSettingsModel({
                          ...settingsModel,
                          capabilityTools: !settingsModel.capabilityTools,
                        })
                      }
                    />
                  </div>
                </DialogField>
              )}
              <DialogFooter className="justify-between">
                <Button
                  variant="destructive"
                  disabled={busy}
                  onClick={() => void deleteSelectedModel()}
                >
                  <Icon name="delete" className="text-sm" />
                  删除模型
                </Button>
                <Button
                  disabled={!settingsModel.alias.trim() || busy}
                  onClick={() => void updateSelectedModel()}
                >
                  保存设置
                </Button>
              </DialogFooter>
            </>
          )}
        </DialogContent>
      </Dialog>
    </>
  );
}

export default ProviderSettingsPage;
