export type ColorMode = "light" | "dark" | "system";

export type BuiltInThemeId = "sky" | "iris" | "pine" | "lagoon" | "sand";
export type ThemeId = BuiltInThemeId | "custom";

export interface AppearancePreferences {
  colorMode: ColorMode;
  themeId: ThemeId;
  customThemeColor: string;
  fontFamily: string;
}

export interface ThemePreset {
  id: BuiltInThemeId;
  name: string;
  description: string;
  swatches: readonly [string, string, string, string];
}
