export type ColorMode = "light" | "dark" | "system";

export type ThemeId = "sky" | "iris" | "pine" | "sand";

export interface AppearancePreferences {
  colorMode: ColorMode;
  themeId: ThemeId;
  fontFamily: string;
}

export interface ThemePreset {
  id: ThemeId;
  name: string;
  description: string;
  swatches: readonly [string, string, string, string];
}
