import { useCallback, useRef, type Dispatch, type SetStateAction } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { ProviderView } from "./types";

interface UseProviderEnabledToggleOptions {
  setProviders: Dispatch<SetStateAction<ProviderView[]>>;
  onError: (message: string) => void;
  onUpdated?: (provider: ProviderView) => void;
}

function errorMessage(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

export function useProviderEnabledToggle({
  setProviders,
  onError,
  onUpdated,
}: UseProviderEnabledToggleOptions) {
  const saveTimer = useRef<number | null>(null);
  const requestSequence = useRef(0);
  const confirmedEnabled = useRef<Map<string, boolean>>(new Map());

  const syncProviders = useCallback((providers: ProviderView[]): void => {
    confirmedEnabled.current = new Map(
      providers.map((provider) => [provider.id, provider.enabled]),
    );
  }, []);

  const setEnabledOptimistically = useCallback(
    (provider: ProviderView, enabled: boolean): void => {
      const confirmed = confirmedEnabled.current.get(provider.id) ?? provider.enabled;
      setProviders((items) =>
        items.map((item) => (item.id === provider.id ? { ...item, enabled } : item)),
      );
      if (saveTimer.current !== null) window.clearTimeout(saveTimer.current);
      const sequence = ++requestSequence.current;
      saveTimer.current = window.setTimeout(async () => {
        try {
          const updated = await invoke<ProviderView>("set_provider_enabled", {
            input: { id: provider.id, enabled },
          });
          if (sequence !== requestSequence.current) return;
          confirmedEnabled.current.set(updated.id, updated.enabled);
          setProviders((items) =>
            items.map((item) => (item.id === updated.id ? updated : item)),
          );
          onUpdated?.(updated);
        } catch (cause) {
          if (sequence !== requestSequence.current) return;
          setProviders((items) =>
            items.map((item) =>
              item.id === provider.id ? { ...item, enabled: confirmed } : item,
            ),
          );
          onError(errorMessage(cause));
        }
      }, 300);
    },
    [onError, onUpdated, setProviders],
  );

  return { setEnabledOptimistically, syncProviders };
}
