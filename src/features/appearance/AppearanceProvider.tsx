import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";

import {
  getAppearancePreferences,
  isTauriRuntime,
  updateAppearancePreferences,
} from "@/features/appearance/api";
import {
  DEFAULT_APPEARANCE,
  DEFAULT_CUSTOM_THEME_COLOR,
  APPEARANCE_STORAGE_KEY,
  SYSTEM_FONT_STACK,
  SYSTEM_FONT_VALUE,
} from "@/features/appearance/constants";
import {
  applyCustomThemeVariables,
  clearCustomThemeVariables,
  normalizeHexColor,
} from "@/features/appearance/theme-colors";
import type {
  AppearancePreferences,
  ColorMode,
  ThemeId,
} from "@/features/appearance/types";

interface AppearanceContextValue {
  preferences: AppearancePreferences;
  resolvedMode: Exclude<ColorMode, "system">;
  setColorMode: (colorMode: ColorMode) => void;
  setThemeId: (themeId: ThemeId) => void;
  setCustomThemeColor: (customThemeColor: string) => void;
  setFontFamily: (fontFamily: string) => void;
}

const AppearanceContext = createContext<AppearanceContextValue | null>(null);

function fontStack(fontFamily: string): string {
  if (fontFamily === SYSTEM_FONT_VALUE) return SYSTEM_FONT_STACK;
  const escaped = fontFamily.replaceAll("\\", "\\\\").replaceAll('"', '\\"');
  return `"${escaped}", ${SYSTEM_FONT_STACK}`;
}

function systemIsDark(): boolean {
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

function normalizePreferences(value: AppearancePreferences): AppearancePreferences {
  const customThemeColor = normalizeHexColor(value.customThemeColor);
  return {
    colorMode:
      value.colorMode === "light" ||
      value.colorMode === "dark" ||
      value.colorMode === "system"
        ? value.colorMode
        : DEFAULT_APPEARANCE.colorMode,
    themeId:
      value.themeId === "sky" ||
      value.themeId === "iris" ||
      value.themeId === "pine" ||
      value.themeId === "lagoon" ||
      value.themeId === "sand" ||
      value.themeId === "custom"
        ? value.themeId
        : DEFAULT_APPEARANCE.themeId,
    customThemeColor: customThemeColor ?? DEFAULT_CUSTOM_THEME_COLOR,
    fontFamily: value.fontFamily.trim() || DEFAULT_APPEARANCE.fontFamily,
  };
}

function readLegacyPreferences(): AppearancePreferences | null {
  try {
    const stored = window.localStorage.getItem(APPEARANCE_STORAGE_KEY);
    if (!stored) return null;
    const value = JSON.parse(stored) as Partial<AppearancePreferences>;
    return normalizePreferences({
      colorMode: value.colorMode ?? DEFAULT_APPEARANCE.colorMode,
      themeId: value.themeId ?? DEFAULT_APPEARANCE.themeId,
      customThemeColor: value.customThemeColor ?? DEFAULT_APPEARANCE.customThemeColor,
      fontFamily: value.fontFamily ?? DEFAULT_APPEARANCE.fontFamily,
    });
  } catch {
    return null;
  }
}

export function AppearanceProvider({ children }: { children: ReactNode }) {
  const [preferences, setPreferences] = useState<AppearancePreferences>(DEFAULT_APPEARANCE);
  const [loaded, setLoaded] = useState<boolean>(!isTauriRuntime());
  const [systemDark, setSystemDark] = useState<boolean>(systemIsDark);
  const resolvedMode =
    preferences.colorMode === "system"
      ? systemDark
        ? "dark"
        : "light"
      : preferences.colorMode;

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    void getAppearancePreferences()
      .then((stored) => {
        if (disposed) return;
        const migrated = stored.stored ? null : readLegacyPreferences();
        const nextPreferences = migrated ?? normalizePreferences(stored.preferences);
        setPreferences(nextPreferences);
        if (migrated) void updateAppearancePreferences(migrated);
        setLoaded(true);
      })
      .catch((error: unknown) => {
        console.error("Unable to load appearance preferences.", error);
      });
    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    const query = window.matchMedia("(prefers-color-scheme: dark)");
    const update = (event: MediaQueryListEvent): void => setSystemDark(event.matches);
    setSystemDark(query.matches);
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, []);

  useEffect(() => {
    const root = document.documentElement;
    root.classList.toggle("dark", resolvedMode === "dark");
    root.dataset.theme = preferences.themeId;
    root.style.setProperty("--app-font-family", fontStack(preferences.fontFamily));
    if (preferences.themeId === "custom") {
      applyCustomThemeVariables(
        root,
        preferences.customThemeColor,
        resolvedMode === "dark",
      );
    } else {
      clearCustomThemeVariables(root);
    }
  }, [preferences, resolvedMode]);

  useEffect(() => {
    if (!loaded || !isTauriRuntime()) return;
    void updateAppearancePreferences(preferences);
  }, [loaded, preferences]);

  const setColorMode = useCallback((colorMode: ColorMode): void => {
    setPreferences((current) => ({ ...current, colorMode }));
  }, []);

  const setThemeId = useCallback((themeId: ThemeId): void => {
    setPreferences((current) => ({ ...current, themeId }));
  }, []);

  const setCustomThemeColor = useCallback((customThemeColor: string): void => {
    const normalized = normalizeHexColor(customThemeColor);
    if (!normalized) return;
    setPreferences((current) => ({ ...current, customThemeColor: normalized }));
  }, []);

  const setFontFamily = useCallback((fontFamily: string): void => {
    setPreferences((current) => ({ ...current, fontFamily }));
  }, []);

  const value = useMemo<AppearanceContextValue>(
    () => ({
      preferences,
      resolvedMode,
      setColorMode,
      setCustomThemeColor,
      setThemeId,
      setFontFamily,
    }),
    [preferences, resolvedMode, setColorMode, setCustomThemeColor, setFontFamily, setThemeId],
  );

  return <AppearanceContext.Provider value={value}>{children}</AppearanceContext.Provider>;
}

export function useAppearance(): AppearanceContextValue {
  const context = useContext(AppearanceContext);
  if (!context) throw new Error("useAppearance must be used within AppearanceProvider");
  return context;
}
