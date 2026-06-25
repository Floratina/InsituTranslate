import { Monitor, Moon, Palette, Settings, Sun, Type } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { SelectableOptionButton } from "@/components/ui/selectable-option-button";
import { CustomThemeColorPicker } from "@/features/appearance/CustomThemeColorPicker";
import { FontPicker } from "@/features/appearance/FontPicker";
import { useAppearance } from "@/features/appearance/AppearanceProvider";
import {
  SYSTEM_FONT_STACK,
  SYSTEM_FONT_VALUE,
  THEME_PRESETS,
} from "@/features/appearance/constants";
import { getCustomThemeSwatches } from "@/features/appearance/theme-colors";
import type { ColorMode, ThemeId } from "@/features/appearance/types";
import { cn } from "@/lib/utils";

const colorModes: readonly {
  value: ColorMode;
  label: string;
  icon: typeof Sun;
}[] = [
  { value: "light", label: "浅色模式", icon: Sun },
  { value: "dark", label: "深色模式", icon: Moon },
  { value: "system", label: "跟随系统", icon: Monitor },
];

function ThemeSwatches({ swatches }: { swatches: readonly [string, string, string, string] }) {
  return (
    <span className="grid shrink-0 grid-cols-2 overflow-hidden rounded-[6px] border">
      {swatches.map((swatch) => (
        <span
          key={swatch}
          className="size-5"
          style={{ backgroundColor: swatch }}
        />
      ))}
    </span>
  );
}

function CustomThemeOption({
  selected,
  swatches,
  value,
  onSelect,
  onColorChange,
}: {
  selected: boolean;
  swatches: readonly [string, string, string, string];
  value: string;
  onSelect: (themeId: ThemeId) => void;
  onColorChange: (value: string) => void;
}) {
  const [pickerOpen, setPickerOpen] = useState(false);

  function applyCustomTheme(): void {
    onSelect("custom");
  }

  function applyCustomColor(nextValue: string): void {
    onColorChange(nextValue);
    onSelect("custom");
    setPickerOpen(false);
  }

  return (
    <div
      className={cn(
        "relative flex min-h-16 w-full min-w-0 items-center gap-3 rounded-[6px] border bg-background p-3 text-left outline-none transition-[background-color,border-color,box-shadow] duration-150 hover:bg-muted/60",
        selected && "border-primary bg-background ring-1 ring-primary/35",
      )}
    >
      <button
        type="button"
        aria-pressed={selected}
        className="flex min-w-0 flex-1 items-center gap-3 text-left outline-none focus-visible:ring-3 focus-visible:ring-ring/40"
        onClick={applyCustomTheme}
      >
        <ThemeSwatches swatches={swatches} />
        <span className="min-w-0 flex-1">
          <span className="block text-sm font-medium">自定义</span>
          <span className="mt-0.5 block text-xs leading-snug text-muted-foreground">
            点击修改主题色
          </span>
        </span>
      </button>

      <Popover open={pickerOpen} onOpenChange={setPickerOpen}>
        <PopoverTrigger asChild>
          <Button
            type="button"
            size="sm"
            className="h-7 px-2"
          >
            编辑
          </Button>
        </PopoverTrigger>
        <PopoverContent align="end" className="w-80 p-3">
          <CustomThemeColorPicker value={value} onApply={applyCustomColor} />
        </PopoverContent>
      </Popover>
    </div>
  );
}

export default function AppearanceSettingsPage() {
  const {
    preferences,
    setColorMode,
    setCustomThemeColor,
    setFontFamily,
    setThemeId,
  } = useAppearance();
  const previewFont =
    preferences.fontFamily === SYSTEM_FONT_VALUE
      ? SYSTEM_FONT_STACK
      : `"${preferences.fontFamily.replaceAll('"', '\\"')}", ${SYSTEM_FONT_STACK}`;
  const customThemeSwatches = getCustomThemeSwatches(preferences.customThemeColor);

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
                      <ThemeSwatches swatches={theme.swatches} />
                    )}
                    onClick={() => setThemeId(theme.id)}
                  />
                );
              })}
              <CustomThemeOption
                selected={preferences.themeId === "custom"}
                swatches={customThemeSwatches}
                value={preferences.customThemeColor}
                onSelect={setThemeId}
                onColorChange={setCustomThemeColor}
              />
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
