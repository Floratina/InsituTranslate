import { Monitor, Moon, Palette, Settings, Sun, Type } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { SelectableOptionButton } from "@/components/ui/selectable-option-button";
import { FontPicker } from "@/features/appearance/FontPicker";
import { useAppearance } from "@/features/appearance/AppearanceProvider";
import {
  SYSTEM_FONT_STACK,
  SYSTEM_FONT_VALUE,
  THEME_PRESETS,
} from "@/features/appearance/constants";
import type { ColorMode } from "@/features/appearance/types";

const colorModes: readonly {
  value: ColorMode;
  label: string;
  icon: typeof Sun;
}[] = [
  { value: "light", label: "浅色模式", icon: Sun },
  { value: "dark", label: "深色模式", icon: Moon },
  { value: "system", label: "跟随系统", icon: Monitor },
];

export default function AppearanceSettingsPage() {
  const {
    preferences,
    setColorMode,
    setFontFamily,
    setThemeId,
  } = useAppearance();
  const previewFont =
    preferences.fontFamily === SYSTEM_FONT_VALUE
      ? SYSTEM_FONT_STACK
      : `"${preferences.fontFamily.replaceAll('"', '\\"')}", ${SYSTEM_FONT_STACK}`;

  return (
    <main className="flex min-w-0 flex-1 flex-col overflow-hidden p-3">
      <header className="mb-3 shrink-0">
        <div className="flex items-center gap-2">
          <Settings className="size-5 text-primary" />
          <h1 className="text-xl font-medium tracking-tight">设置</h1>
        </div>
        <p className="mt-0.5 text-xs text-muted-foreground">
          调整界面颜色与全局字体
        </p>
      </header>

      <div className="min-h-0 flex-1 overflow-x-hidden overflow-y-auto overscroll-contain pr-1">
        <div className="grid w-full max-w-4xl gap-3">
          <Card size="sm" className="gap-3 rounded-[6px] py-3">
            <CardHeader className="px-3">
              <div className="flex items-center gap-2">
                <Palette className="size-4 text-primary" />
                <CardTitle>颜色模式</CardTitle>
              </div>
            </CardHeader>
            <CardContent className="grid grid-cols-[repeat(auto-fit,minmax(10rem,1fr))] gap-2 px-3">
              {colorModes.map((mode) => {
                const Icon = mode.icon;
                const selected = preferences.colorMode === mode.value;
                return (
                  <Button
                    key={mode.value}
                    type="button"
                    variant={selected ? "default" : "outline"}
                    className="h-9 min-w-0 rounded-[6px]"
                    aria-pressed={selected}
                    onClick={() => setColorMode(mode.value)}
                  >
                    <Icon className="size-4" />
                    {mode.label}
                  </Button>
                );
              })}
            </CardContent>
          </Card>

          <Card size="sm" className="gap-3 rounded-[6px] py-3">
            <CardHeader className="px-3">
              <CardTitle>主题色板</CardTitle>
            </CardHeader>
            <CardContent className="grid grid-cols-[repeat(auto-fit,minmax(15rem,1fr))] gap-2 px-3">
              {THEME_PRESETS.map((theme) => {
                const selected = preferences.themeId === theme.id;
                return (
                  <SelectableOptionButton
                    key={theme.id}
                    type="button"
                    label={theme.name}
                    description={theme.description}
                    selected={selected}
                    className="min-h-16 p-3"
                    leading={(
                      <span className="grid shrink-0 grid-cols-2 overflow-hidden rounded-[6px] border">
                        {theme.swatches.map((swatch) => (
                          <span
                            key={swatch}
                            className="size-5"
                            style={{ backgroundColor: swatch }}
                          />
                        ))}
                      </span>
                    )}
                    onClick={() => setThemeId(theme.id)}
                  />
                );
              })}
            </CardContent>
          </Card>

          <Card size="sm" className="gap-3 rounded-[6px] py-3">
            <CardHeader className="px-3">
              <div className="flex items-center gap-2">
                <Type className="size-4 text-primary" />
                <CardTitle>界面字体</CardTitle>
              </div>
            </CardHeader>
            <CardContent className="grid gap-3 px-3">
              <FontPicker value={preferences.fontFamily} onValueChange={setFontFamily} />
              <div
                className="break-words rounded-[6px] border bg-muted/35 px-3 py-2 text-sm"
                style={{ fontFamily: previewFont }}
              >
                海浪的声音平静了我的心灵。The sound of waves calms my soul.
              </div>
            </CardContent>
          </Card>
        </div>
      </div>
    </main>
  );
}
