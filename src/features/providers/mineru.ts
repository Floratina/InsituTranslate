import type { MinerUProviderConfig, ProviderConfig, ProviderView } from "./types";

export const MINERU_PROVIDER_ID = "builtin_document-parsing_mineru";
export const MINERU_STANDARD_BASE_URL = "https://mineru.net/api/v4";
export const MINERU_FLASH_BASE_URL = "https://mineru.net/api/v1/agent";

export const DEFAULT_MINERU_CONFIG: MinerUProviderConfig = {
  mode: "standard",
  flashBaseUrl: MINERU_FLASH_BASE_URL,
};

export function getMinerUConfig(config: ProviderConfig | null | undefined): MinerUProviderConfig {
  const mineru = config?.mineru;
  return {
    mode: mineru?.mode === "flash" ? "flash" : "standard",
    flashBaseUrl: mineru?.flashBaseUrl?.trim() || MINERU_FLASH_BASE_URL,
  };
}

export function withMinerUConfig(
  config: ProviderConfig,
  next: Partial<MinerUProviderConfig>,
): ProviderConfig {
  return {
    ...config,
    mineru: {
      ...getMinerUConfig(config),
      ...next,
    },
  };
}

export function isMinerUProvider(provider: ProviderView | null): boolean {
  return Boolean(
    provider &&
      (provider.id === MINERU_PROVIDER_ID || provider.config?.mineru !== undefined),
  );
}
