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
  APPEARANCE_STORAGE_KEY,
  DEFAULT_APPEARANCE,
  SYSTEM_FONT_STACK,
  SYSTEM_FONT_VALUE,
} from "@/features/appearance/constants";
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
  setFontFamily: (fontFamily: string) => void;
}

const AppearanceContext = createContext<AppearanceContextValue | null>(null);

function readPreferences(): AppearancePreferences {
  try {
    const stored = window.localStorage.getItem(APPEARANCE_STORAGE_KEY);
    if (!stored) return DEFAULT_APPEARANCE;
    const value = JSON.parse(stored) as Partial<AppearancePreferences>;
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
        value.themeId === "sand"
          ? value.themeId
          : DEFAULT_APPEARANCE.themeId,
      fontFamily:
        typeof value.fontFamily === "string" && value.fontFamily.trim()
          ? value.fontFamily
          : DEFAULT_APPEARANCE.fontFamily,
    };
  } catch {
    return DEFAULT_APPEARANCE;
  }
}

function fontStack(fontFamily: string): string {
  if (fontFamily === SYSTEM_FONT_VALUE) return SYSTEM_FONT_STACK;
  const escaped = fontFamily.replaceAll("\\", "\\\\").replaceAll('"', '\\"');
  return `"${escaped}", ${SYSTEM_FONT_STACK}`;
}

function systemIsDark(): boolean {
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

export function AppearanceProvider({ children }: { children: ReactNode }) {
  const [preferences, setPreferences] = useState<AppearancePreferences>(readPreferences);
  const [systemDark, setSystemDark] = useState<boolean>(systemIsDark);
  const resolvedMode =
    preferences.colorMode === "system"
      ? systemDark
        ? "dark"
        : "light"
      : preferences.colorMode;

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
    window.localStorage.setItem(APPEARANCE_STORAGE_KEY, JSON.stringify(preferences));
  }, [preferences, resolvedMode]);

  const setColorMode = useCallback((colorMode: ColorMode): void => {
    setPreferences((current) => ({ ...current, colorMode }));
  }, []);

  const setThemeId = useCallback((themeId: ThemeId): void => {
    setPreferences((current) => ({ ...current, themeId }));
  }, []);

  const setFontFamily = useCallback((fontFamily: string): void => {
    setPreferences((current) => ({ ...current, fontFamily }));
  }, []);

  const value = useMemo<AppearanceContextValue>(
    () => ({
      preferences,
      resolvedMode,
      setColorMode,
      setThemeId,
      setFontFamily,
    }),
    [preferences, resolvedMode, setColorMode, setFontFamily, setThemeId],
  );

  return <AppearanceContext.Provider value={value}>{children}</AppearanceContext.Provider>;
}

export function useAppearance(): AppearanceContextValue {
  const context = useContext(AppearanceContext);
  if (!context) throw new Error("useAppearance must be used within AppearanceProvider");
  return context;
}
