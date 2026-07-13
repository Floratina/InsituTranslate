import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { Bot, Plus } from "lucide-react";
import { Reorder } from "motion/react";

import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useToast } from "@/components/ui/toast-stack";
import { appSessionCache } from "@/lib/session-cache";
import { AssistantDetailsPanel } from "@/features/assistants/AssistantDetailsPanel";
import { AssistantListItem } from "@/features/assistants/AssistantListItem";
import {
  hasJsonOutputConflict,
  isRecord,
  JSON_OUTPUT_CONFLICT_WARNING,
} from "@/features/assistants/customParameters";
import type {
  AssistantSettingsDraft,
  AssistantView,
} from "@/features/assistants/types";
import { PURPOSES } from "@/features/providers/constants";
import type { ProviderPurpose } from "@/features/providers/types";

export type AssistantNavigationGuard = (action: () => void) => void;

interface AssistantSettingsPageProps {
  onRegisterNavigationGuard: (guard: AssistantNavigationGuard | null) => void;
}

function isTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

function getErrorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

function purposeLabel(purpose: ProviderPurpose): string {
  return PURPOSES.find((item) => item.value === purpose)?.label ?? purpose;
}

function settingsFromAssistant(assistant: AssistantView): AssistantSettingsDraft {
  return {
    id: assistant.id,
    name: assistant.name,
    iconKind: assistant.iconKind,
    iconValue: assistant.iconValue,
    temperatureEnabled: assistant.temperatureEnabled,
    temperature: assistant.temperature,
    topPEnabled: assistant.topPEnabled,
    topP: assistant.topP,
  };
}

function formattedCustomParameters(assistant: AssistantView): string {
  return JSON.stringify(assistant.customParameters ?? {}, null, 2);
}

function draftHasJsonOutputConflict(value: string): boolean {
  try {
    const parsed: unknown = JSON.parse(value);
    return isRecord(parsed) && hasJsonOutputConflict(parsed);
  } catch {
    return false;
  }
}

function AssistantListSkeleton() {
  return (
    <div className="grid gap-1">
      {Array.from({ length: 5 }).map((_, index) => (
        <div key={index} className="flex h-12 items-center gap-2 rounded-[6px] p-2">
          <Skeleton className="size-8 shrink-0" />
          <Skeleton className="h-4 w-28 min-w-0 flex-1" />
          <Skeleton className="size-4 shrink-0" />
        </div>
      ))}
    </div>
  );
}

function AssistantDetailsSkeleton() {
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center justify-between gap-3 border-b p-3">
        <div className="flex items-center gap-3">
          <Skeleton className="size-9" />
          <div className="flex items-center gap-2">
            <Skeleton className="h-4 w-32" />
            <Skeleton className="h-5 w-12" />
            <Skeleton className="size-7" />
          </div>
        </div>
      </div>
      <div className="grid gap-3 p-3">
        <div className="grid gap-1.5 rounded-[6px] border p-3">
          <Skeleton className="h-4 w-24" />
          <Skeleton className="h-8 w-full" />
        </div>
        <div className="grid grid-cols-2 gap-3 max-[920px]:grid-cols-1">
          <Skeleton className="h-24 w-full" />
          <Skeleton className="h-24 w-full" />
        </div>
        <Skeleton className="h-36 w-full" />
      </div>
    </div>
  );
}

export default function AssistantSettingsPage({
  onRegisterNavigationGuard,
}: AssistantSettingsPageProps) {
  const cachedAssistants = appSessionCache.assistants("translation").read();
  const cachedSelectedAssistantId =
    appSessionCache.assistantSelectedIds.get("translation") ?? "";
  const [purpose, setPurpose] = useState<ProviderPurpose>("translation");
  const [assistants, setAssistants] = useState<AssistantView[] | null>(
    cachedAssistants ?? null,
  );
  const [selectedAssistantId, setSelectedAssistantId] = useState(
    cachedAssistants?.some((assistant) => assistant.id === cachedSelectedAssistantId)
      ? cachedSelectedAssistantId
      : (cachedAssistants?.[0]?.id ?? ""),
  );
  const [settingsDraft, setSettingsDraft] = useState<AssistantSettingsDraft | null>(null);
  const [promptDraft, setPromptDraft] = useState("");
  const [customParametersDraft, setCustomParametersDraft] = useState("{}");
  const [loading, setLoading] = useState(!cachedAssistants);
  const [busy, setBusy] = useState(false);
  const [savingPrompt, setSavingPrompt] = useState(false);
  const [savingCustomParameters, setSavingCustomParameters] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<AssistantView | null>(null);
  const [pendingAction, setPendingAction] = useState<(() => void) | null>(null);
  const settingsBaselineRef = useRef("");
  const promptBaselineRef = useRef("");
  const customParametersBaselineRef = useRef("{}");
  const assistantOrderRef = useRef<string[]>([]);
  const autoSaveTimerRef = useRef<number | null>(null);
  const conflictWarningTimerRef = useRef<number | null>(null);
  const customParametersConflictRef = useRef(false);
  const assistantRequestIdRef = useRef(0);
  const purposeRef = useRef<ProviderPurpose>(purpose);
  const visibleAssistants = assistants ?? [];
  const { pushToast } = useToast();
  const setAssistantsAndCache = useCallback(
    (action: AssistantView[] | ((current: AssistantView[]) => AssistantView[])): void => {
      setAssistants((current) => {
        const resource = appSessionCache.assistants(purpose);
        const source = purposeRef.current === purpose ? (current ?? []) : (resource.read() ?? []);
        const next = typeof action === "function" ? action(source) : action;
        resource.set(next);
        if (purposeRef.current !== purpose) return current;
        return next;
      });
    },
    [purpose],
  );

  const selectedAssistant = useMemo(
    () => visibleAssistants.find((assistant) => assistant.id === selectedAssistantId) ?? null,
    [selectedAssistantId, visibleAssistants],
  );
  const promptDirty = promptDraft !== promptBaselineRef.current;
  const customParametersDirty =
    customParametersDraft !== customParametersBaselineRef.current;
  const manualDirty = promptDirty || customParametersDirty;
  const settingsDirty =
    settingsDraft !== null &&
    JSON.stringify(settingsDraft) !== settingsBaselineRef.current;

  const showError = useCallback(
    (message: string): void => pushToast(message, "error"),
    [pushToast],
  );
  const showConflictWarning = useCallback(
    (): void => pushToast(JSON_OUTPUT_CONFLICT_WARNING, "warning"),
    [pushToast],
  );
  const updateCustomParametersDraft = useCallback(
    (value: string, warnImmediately = false): void => {
      setCustomParametersDraft(value);
      if (!warnImmediately) return;
      const hasConflict = draftHasJsonOutputConflict(value);
      if (hasConflict) showConflictWarning();
      customParametersConflictRef.current = hasConflict;
    },
    [showConflictWarning],
  );

  useEffect(() => {
    if (selectedAssistantId) {
      appSessionCache.assistantSelectedIds.set(purpose, selectedAssistantId);
    } else {
      appSessionCache.assistantSelectedIds.delete(purpose);
    }
  }, [purpose, selectedAssistantId]);

  useEffect(() => {
    void refreshAssistants();
  }, [purpose]);

  useEffect(() => {
    if (!selectedAssistantId) {
      setSettingsDraft(null);
      setPromptDraft("");
      setCustomParametersDraft("{}");
      settingsBaselineRef.current = "";
      promptBaselineRef.current = "";
      customParametersBaselineRef.current = "{}";
      customParametersConflictRef.current = false;
      return;
    }
    const assistant = visibleAssistants.find((item) => item.id === selectedAssistantId);
    if (!assistant) return;
    const nextSettings = settingsFromAssistant(assistant);
    const nextCustomParameters = formattedCustomParameters(assistant);
    setSettingsDraft(nextSettings);
    setPromptDraft(assistant.systemPrompt);
    setCustomParametersDraft(nextCustomParameters);
    settingsBaselineRef.current = JSON.stringify(nextSettings);
    promptBaselineRef.current = assistant.systemPrompt;
    customParametersBaselineRef.current = nextCustomParameters;
    customParametersConflictRef.current =
      draftHasJsonOutputConflict(nextCustomParameters);
  }, [selectedAssistantId]);

  useEffect(() => {
    if (conflictWarningTimerRef.current !== null) {
      window.clearTimeout(conflictWarningTimerRef.current);
    }
    conflictWarningTimerRef.current = window.setTimeout(() => {
      const hasConflict = draftHasJsonOutputConflict(customParametersDraft);
      if (hasConflict && !customParametersConflictRef.current) {
        showConflictWarning();
      }
      customParametersConflictRef.current = hasConflict;
    }, 500);
    return () => {
      if (conflictWarningTimerRef.current !== null) {
        window.clearTimeout(conflictWarningTimerRef.current);
      }
    };
  }, [customParametersDraft, showConflictWarning]);

  useEffect(() => {
    if (!settingsDraft || !settingsDirty || !settingsDraft.name.trim()) return;
    if (autoSaveTimerRef.current !== null) {
      window.clearTimeout(autoSaveTimerRef.current);
    }
    autoSaveTimerRef.current = window.setTimeout(() => {
      void saveSettings(settingsDraft);
    }, 500);
    return () => {
      if (autoSaveTimerRef.current !== null) {
        window.clearTimeout(autoSaveTimerRef.current);
      }
    };
  }, [settingsDraft, settingsDirty]);

  useEffect(() => {
    function beforeUnload(event: BeforeUnloadEvent): void {
      if (!manualDirty) return;
      event.preventDefault();
    }
    window.addEventListener("beforeunload", beforeUnload);
    return () => window.removeEventListener("beforeunload", beforeUnload);
  }, [manualDirty]);

  const runAfterSettingsSave = useCallback(
    (action: () => void): void => {
      if (settingsDraft && settingsDirty && settingsDraft.name.trim()) {
        void saveSettings(settingsDraft).then((saved) => {
          if (saved) action();
        });
        return;
      }
      action();
    },
    [settingsDraft, settingsDirty],
  );

  const requestTransition = useCallback<AssistantNavigationGuard>(
    (action) => {
      if (manualDirty) {
        setPendingAction(() => () => runAfterSettingsSave(action));
        return;
      }
      runAfterSettingsSave(action);
    },
    [manualDirty, runAfterSettingsSave],
  );

  useEffect(() => {
    onRegisterNavigationGuard(requestTransition);
    return () => onRegisterNavigationGuard(null);
  }, [onRegisterNavigationGuard, requestTransition]);

  async function refreshAssistants(preferredId?: string, force = false): Promise<void> {
    const resource = appSessionCache.assistants(purpose);
    if (purposeRef.current !== purpose) {
      if (force && isTauriRuntime()) {
        try {
          await resource.refresh(() => invoke<AssistantView[]>("list_assistants", { purpose }));
        } catch (cause) {
          showError(getErrorMessage(cause));
        }
      }
      return;
    }
    const requestId = assistantRequestIdRef.current + 1;
    assistantRequestIdRef.current = requestId;
    const cached = force ? undefined : resource.read();
    if (cached) {
      if (assistantRequestIdRef.current !== requestId) return;
      assistantOrderRef.current = cached.map((assistant) => assistant.id);
      setAssistants(cached);
      setSelectedAssistantId((current) => {
        const next =
          preferredId ?? current ?? appSessionCache.assistantSelectedIds.get(purpose);
        return cached.some((assistant) => assistant.id === next)
          ? next
          : (cached[0]?.id ?? "");
      });
      setLoading(false);
      return;
    }

    setLoading(true);
    try {
      if (!isTauriRuntime()) {
        resource.set([]);
        setAssistants([]);
        setSelectedAssistantId("");
        return;
      }
      const result = await (force
        ? resource.refresh(() => invoke<AssistantView[]>("list_assistants", { purpose }))
        : resource.loadOnce(() => invoke<AssistantView[]>("list_assistants", { purpose })));
      if (assistantRequestIdRef.current !== requestId) return;
      assistantOrderRef.current = result.map((assistant) => assistant.id);
      setAssistants(result);
      setSelectedAssistantId((current) => {
        const next =
          preferredId ?? current ?? appSessionCache.assistantSelectedIds.get(purpose);
        return result.some((assistant) => assistant.id === next)
          ? next
          : (result[0]?.id ?? "");
      });
    } catch (cause) {
      if (assistantRequestIdRef.current === requestId) showError(getErrorMessage(cause));
    } finally {
      if (assistantRequestIdRef.current === requestId) setLoading(false);
    }
  }

  function changePurpose(nextPurpose: ProviderPurpose): void {
    if (nextPurpose === purpose) return;
    assistantRequestIdRef.current += 1;
    const cached = appSessionCache.assistants(nextPurpose).read();
    const cachedSelectedId = appSessionCache.assistantSelectedIds.get(nextPurpose) ?? "";
    purposeRef.current = nextPurpose;
    setPurpose(nextPurpose);
    setAssistants(cached ?? null);
    setSelectedAssistantId(
      cached?.some((assistant) => assistant.id === cachedSelectedId)
        ? cachedSelectedId
        : (cached?.[0]?.id ?? ""),
    );
    setLoading(cached === undefined);
  }

  async function saveSettings(draft: AssistantSettingsDraft): Promise<boolean> {
    try {
      const updated = await invoke<AssistantView>("update_assistant_settings", {
        input: draft,
      });
      const normalizedSettings = settingsFromAssistant(updated);
      settingsBaselineRef.current = JSON.stringify(normalizedSettings);
      setSettingsDraft((current) =>
        current?.id === updated.id && JSON.stringify(current) === JSON.stringify(draft)
          ? normalizedSettings
          : current,
      );
      setAssistantsAndCache((items) =>
        items.map((item) => (item.id === updated.id ? updated : item)),
      );
      return true;
    } catch (cause) {
      showError(getErrorMessage(cause));
      return false;
    }
  }

  async function savePrompt(nextPrompt = promptDraft): Promise<boolean> {
    if (!selectedAssistant) return false;
    setSavingPrompt(true);
    try {
      const updated = await invoke<AssistantView>("update_assistant_prompt", {
        input: { id: selectedAssistant.id, systemPrompt: nextPrompt },
      });
      promptBaselineRef.current = updated.systemPrompt;
      setPromptDraft(updated.systemPrompt);
      setAssistantsAndCache((items) =>
        items.map((item) => (item.id === updated.id ? updated : item)),
      );
      pushToast("系统提示词已保存");
      return true;
    } catch (cause) {
      showError(getErrorMessage(cause));
      return false;
    } finally {
      setSavingPrompt(false);
    }
  }

  async function saveCustomParameters(): Promise<boolean> {
    if (!selectedAssistant) return false;
    let parsed: unknown;
    try {
      parsed = JSON.parse(customParametersDraft);
    } catch {
      showError("自定义参数不是有效 JSON");
      return false;
    }
    if (!isRecord(parsed)) {
      showError("自定义参数必须是 JSON 对象");
      return false;
    }
    if (hasJsonOutputConflict(parsed)) showConflictWarning();
    setSavingCustomParameters(true);
    try {
      const updated = await invoke<AssistantView>(
        "update_assistant_custom_parameters",
        { input: { id: selectedAssistant.id, customParameters: parsed } },
      );
      const formatted = formattedCustomParameters(updated);
      customParametersBaselineRef.current = formatted;
      setCustomParametersDraft(formatted);
      setAssistantsAndCache((items) =>
        items.map((item) => (item.id === updated.id ? updated : item)),
      );
      pushToast("自定义参数已保存");
      return true;
    } catch (cause) {
      showError(getErrorMessage(cause));
      return false;
    } finally {
      setSavingCustomParameters(false);
    }
  }

  async function saveDirtyAndContinue(): Promise<void> {
    if (promptDirty && !(await savePrompt())) return;
    if (customParametersDirty && !(await saveCustomParameters())) return;
    const action = pendingAction;
    setPendingAction(null);
    action?.();
  }

  async function createAssistant(): Promise<void> {
    setBusy(true);
    try {
      const created = await invoke<AssistantView>("create_assistant", {
        input: { purpose },
      });
      await refreshAssistants(created.id, true);
      pushToast("助手已添加");
    } catch (cause) {
      showError(getErrorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  async function copyAssistant(
    assistant: AssistantView,
    targetPurpose: ProviderPurpose,
  ): Promise<void> {
    try {
      const copied = await invoke<AssistantView>("copy_assistant", {
        input: { assistantId: assistant.id, purpose: targetPurpose },
      });
      if (targetPurpose === purpose) {
        await refreshAssistants(copied.id, true);
      } else {
        appSessionCache.assistants(targetPurpose).invalidate();
      }
      pushToast(`已复制到${purposeLabel(targetPurpose)}`);
    } catch (cause) {
      showError(getErrorMessage(cause));
    }
  }

  async function deleteAssistant(assistant: AssistantView): Promise<void> {
    setBusy(true);
    try {
      await invoke("delete_assistant", { id: assistant.id });
      setDeleteTarget(null);
      await refreshAssistants(undefined, true);
      pushToast("助手已删除");
    } catch (cause) {
      showError(getErrorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  function reorderAssistants(nextIds: string[]): void {
    assistantOrderRef.current = nextIds;
    setAssistantsAndCache((current) =>
      nextIds
        .map((id) => current.find((assistant) => assistant.id === id))
        .filter((assistant): assistant is AssistantView => assistant !== undefined),
    );
  }

  async function persistAssistantOrder(): Promise<void> {
    try {
      await invoke("reorder_assistants", {
        input: { purpose, assistantIds: assistantOrderRef.current },
      });
    } catch (cause) {
      showError(getErrorMessage(cause));
      await refreshAssistants(selectedAssistantId, true);
    }
  }

  function selectAssistant(id: string): void {
    if (id === selectedAssistantId) {
      return;
    }
    requestTransition(() => {
      setSelectedAssistantId(id);
    });
  }

  return (
    <>
      <main className="flex min-w-0 flex-1 flex-col overflow-hidden p-3">
        <header className="mb-3 shrink-0">
          <div className="flex items-center gap-2">
            <Bot className="size-5 text-primary" />
            <h1 className="text-xl font-medium tracking-tight">助手</h1>
          </div>
          <p className="mt-0.5 text-xs text-muted-foreground">
            管理不同用途的系统提示词与模型调用参数
          </p>
        </header>

        <div className="grid min-h-0 min-w-0 flex-1 grid-cols-[minmax(13.5rem,16.25rem)_minmax(0,1fr)] gap-3 max-[760px]:grid-cols-1">
          <Card className="min-h-0 min-w-0 gap-0 rounded-[12px] py-0">
            <div className="border-b p-2">
              <Select
                value={purpose}
                onValueChange={(value) =>
                  requestTransition(() => changePurpose(value as ProviderPurpose))
                }
              >
                <SelectTrigger className="bg-card">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {PURPOSES.map((item) => {
                    const PurposeIcon = item.icon;
                    return (
                      <SelectItem key={item.value} value={item.value}>
                        <span className="flex items-center gap-2">
                          <PurposeIcon className="size-3.5" />
                          {item.label}
                        </span>
                      </SelectItem>
                    );
                  })}
                </SelectContent>
              </Select>
              <div className="mt-2 text-xs text-muted-foreground">
                共 {visibleAssistants.length} 个助手
              </div>
            </div>
            <ScrollArea className="min-h-0 flex-1">
              <Reorder.Group
                axis="y"
                values={visibleAssistants.map((assistant) => assistant.id)}
                onReorder={reorderAssistants}
                className="grid gap-1 p-2"
              >
                {assistants === null && loading ? (
                  <AssistantListSkeleton />
                ) : visibleAssistants.length === 0 ? (
                  <div className="rounded-[6px] border border-dashed p-3 text-center text-xs text-muted-foreground">
                    当前用途还没有助手
                  </div>
                ) : (
                  visibleAssistants.map((assistant) => (
                    <AssistantListItem
                      key={assistant.id}
                      assistant={assistant}
                      selected={assistant.id === selectedAssistantId}
                      onSelect={() => selectAssistant(assistant.id)}
                      onDelete={() => setDeleteTarget(assistant)}
                      onCopy={(targetPurpose) => {
                        const action = () => void copyAssistant(assistant, targetPurpose);
                        if (targetPurpose === purpose) requestTransition(action);
                        else action();
                      }}
                      onDragComplete={() => void persistAssistantOrder()}
                    />
                  ))
                )}
              </Reorder.Group>
            </ScrollArea>
            <div className="border-t p-2">
              <Button
                className="w-full rounded-[6px]"
                disabled={busy}
                onClick={() => requestTransition(() => void createAssistant())}
              >
                <Plus className="size-4" />
                添加助手
              </Button>
            </div>
          </Card>

          <Card className="min-h-0 min-w-0 gap-0 rounded-[12px] py-0">
            {assistants === null && loading ? (
              <AssistantDetailsSkeleton />
            ) : (
              <AssistantDetailsPanel
                assistant={selectedAssistant}
                settings={settingsDraft}
                promptDraft={promptDraft}
                customParametersDraft={customParametersDraft}
                customParametersDirty={customParametersDirty}
                savingPrompt={savingPrompt}
                savingCustomParameters={savingCustomParameters}
                onSettingsChange={setSettingsDraft}
                onCustomParametersChange={updateCustomParametersDraft}
                onSavePrompt={savePrompt}
                onSaveCustomParameters={() => void saveCustomParameters()}
                onError={showError}
              />
            )}
          </Card>
        </div>
      </main>

      <AlertDialog
        open={deleteTarget !== null}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
      >
        <AlertDialogContent open={deleteTarget !== null}>
          <AlertDialogTitle className="font-semibold">
            删除 {deleteTarget?.name}？
          </AlertDialogTitle>
          <AlertDialogDescription>
            此操作会永久删除该助手及其提示词和参数配置。
          </AlertDialogDescription>
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setDeleteTarget(null)}>
              取消
            </Button>
            <Button
              variant="destructive"
              disabled={busy}
              onClick={() => deleteTarget && void deleteAssistant(deleteTarget)}
            >
              确认删除
            </Button>
          </div>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog
        open={pendingAction !== null}
        onOpenChange={(open) => !open && setPendingAction(null)}
      >
        <AlertDialogContent open={pendingAction !== null}>
          <AlertDialogTitle className="font-semibold">保存未提交的修改？</AlertDialogTitle>
          <AlertDialogDescription>
            自定义参数尚未保存。保存后继续，或放弃这些修改。
          </AlertDialogDescription>
          <div className="flex justify-end gap-2">
            <Button variant="outline" onClick={() => setPendingAction(null)}>
              取消
            </Button>
            <Button
              variant="destructive"
              onClick={() => {
                const action = pendingAction;
                setPendingAction(null);
                action?.();
              }}
            >
              放弃修改
            </Button>
            <Button
              disabled={savingPrompt || savingCustomParameters}
              onClick={() => void saveDirtyAndContinue()}
            >
              保存并继续
            </Button>
          </div>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
