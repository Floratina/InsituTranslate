import { invoke } from "@tauri-apps/api/core";

import type { AppearancePreferences } from "@/features/appearance/types";

export interface AppearancePreferencesState {
  preferences: AppearancePreferences;
  stored: boolean;
}

export interface FontCacheRefresh {
  changed: boolean;
  fonts: string[] | null;
}

export function isTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

export function getAppearancePreferences(): Promise<AppearancePreferencesState> {
  return invoke<AppearancePreferencesState>("get_appearance_preferences");
}

export function updateAppearancePreferences(
  input: AppearancePreferences,
): Promise<AppearancePreferences> {
  return invoke<AppearancePreferences>("update_appearance_preferences", { input });
}

export function openBackendConsole(): Promise<void> {
  return invoke<void>("open_backend_console");
}

export function getCachedSystemFonts(): Promise<string[]> {
  return invoke<string[]>("get_cached_system_fonts");
}

export function refreshSystemFontsCache(): Promise<FontCacheRefresh> {
  return invoke<FontCacheRefresh>("refresh_system_fonts_cache");
}
