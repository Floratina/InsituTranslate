import type { AppearancePreferences, ThemePreset } from "@/features/appearance/types";

export const APPEARANCE_STORAGE_KEY = "insitu-appearance-v1";
export const SYSTEM_FONT_VALUE = "system";
export const SYSTEM_FONT_STACK = 'Inter, "Segoe UI", sans-serif';
export const DEFAULT_CUSTOM_THEME_COLOR = "#16b8c4";

export const DEFAULT_APPEARANCE: AppearancePreferences = {
  colorMode: "system",
  themeId: "sky",
  customThemeColor: DEFAULT_CUSTOM_THEME_COLOR,
  fontFamily: SYSTEM_FONT_VALUE,
};

export const THEME_PRESETS: readonly ThemePreset[] = [
  {
    id: "sky",
    name: "澄空",
    description: "清澈冷静的蓝色",
    swatches: ["oklch(0.55 0.2 255)", "oklch(0.72 0.13 235)", "oklch(0.955 0.03 255)", "oklch(0.985 0.002 247.839)"],
  },
  {
    id: "iris",
    name: "鸢尾",
    description: "柔和克制的紫色",
    swatches: ["oklch(0.56 0.18 295)", "oklch(0.72 0.12 315)", "oklch(0.955 0.025 295)", "oklch(0.985 0.004 295)"],
  },
  {
    id: "pine",
    name: "松针",
    description: "安静自然的青绿",
    swatches: ["oklch(0.53 0.12 165)", "oklch(0.70 0.10 180)", "oklch(0.95 0.025 165)", "oklch(0.984 0.004 165)"],
  },
  {
    id: "lagoon",
    name: "澄湾",
    description: "清透柔和的蓝绿色",
    swatches: ["oklch(0.55 0.14 205)", "oklch(0.74 0.11 205)", "oklch(0.952 0.028 205)", "oklch(0.984 0.004 205)"],
  },
  {
    id: "sand",
    name: "暖砂",
    description: "温和舒适的琥珀",
    swatches: ["oklch(0.57 0.13 65)", "oklch(0.75 0.12 82)", "oklch(0.955 0.03 75)", "oklch(0.985 0.006 75)"],
  },
] as const;
