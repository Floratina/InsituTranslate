import { Check, Monitor, Moon, Palette, Settings, Sun, Type } from "lucide-react";
import { motion } from "motion/react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { FontPicker } from "@/features/appearance/FontPicker";
import { useAppearance } from "@/features/appearance/AppearanceProvider";
import {
  SYSTEM_FONT_STACK,
  SYSTEM_FONT_VALUE,
  THEME_PRESETS,
} from "@/features/appearance/constants";
import type { ColorMode } from "@/features/appearance/types";
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
                  <motion.button
                    key={theme.id}
                    type="button"
                    whileHover={{ y: -1 }}
                    whileTap={{ scale: 0.99 }}
                    transition={{ duration: 0.16, ease: [0.03, 0.59, 0.19, 1] }}
                    aria-pressed={selected}
                    className={cn(
                      "relative flex min-w-0 items-center gap-3 rounded-[6px] border bg-background p-3 text-left outline-none hover:bg-muted/60 focus-visible:ring-3 focus-visible:ring-ring/40",
                      selected && "border-primary ring-1 ring-primary/35",
                    )}
                    onClick={() => setThemeId(theme.id)}
                  >
                    <span className="grid shrink-0 grid-cols-2 overflow-hidden rounded-[6px] border">
                      {theme.swatches.map((swatch) => (
                        <span
                          key={swatch}
                          className="size-5"
                          style={{ backgroundColor: swatch }}
                        />
                      ))}
                    </span>
                    <span className="min-w-0">
                      <span className="block text-sm font-medium">{theme.name}</span>
                      <span className="block truncate text-xs text-muted-foreground">
                        {theme.description}
                      </span>
                    </span>
                    {selected && (
                      <span className="ml-auto flex size-5 items-center justify-center rounded-full bg-primary text-primary-foreground">
                        <Check className="size-3" />
                      </span>
                    )}
                  </motion.button>
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
