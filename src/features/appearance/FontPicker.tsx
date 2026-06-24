import { useCallback, useDeferredValue, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Check, ChevronDown, Search } from "lucide-react";

import { Input } from "@/components/ui/input";
import { LoadingState } from "@/components/ui/loading-state";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  selectItemClassName,
  selectTriggerClassName,
} from "@/components/ui/select";
import { useToast } from "@/components/ui/toast-stack";
import {
  SYSTEM_FONT_STACK,
  SYSTEM_FONT_VALUE,
} from "@/features/appearance/constants";
import { cn } from "@/lib/utils";

const PREVIEW_FONTS = [
  "Arial",
  "Calibri",
  "Microsoft YaHei",
  "Segoe UI",
  "SimSun",
  "Times New Roman",
];
const COMMON_UI_FONT_CANDIDATES = [
  "Segoe UI Variable Text",
  "Segoe UI Variable Display",
  "Segoe UI Variable",
  "Segoe UI",
  "Microsoft YaHei UI",
  "Microsoft YaHei",
  "DengXian",
  "Aptos",
  "Calibri",
  "Arial",
  "Verdana",
  "Tahoma",
  "SimSun",
  "NSimSun",
  "PingFang SC",
  "Hiragino Sans GB",
  "Noto Sans CJK SC",
  "Noto Sans SC",
  "Source Han Sans SC",
  "Source Han Sans CN",
  "HarmonyOS Sans SC",
  "MiSans",
  "Yu Gothic UI",
  "Meiryo",
  "Malgun Gothic",
  "Apple SD Gothic Neo",
] as const;
const FONT_CACHE_KEY = "insitu-system-fonts-v1";

interface FontCache {
  version: 1;
  fonts: string[];
}

interface FontState {
  fonts: string[];
  cached: boolean;
}

let fontStore: FontState | null = null;
let fontRefreshPromise: Promise<string[]> | null = null;

interface FontPickerProps {
  value: string;
  onValueChange: (value: string) => void;
}

function isTauriRuntime(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

function readFontCache(): FontState {
  if (!isTauriRuntime()) return { fonts: PREVIEW_FONTS, cached: true };
  try {
    const stored = window.localStorage.getItem(FONT_CACHE_KEY);
    if (!stored) return { fonts: [], cached: false };
    const cache = JSON.parse(stored) as Partial<FontCache>;
    if (
      cache.version !== 1 ||
      !Array.isArray(cache.fonts) ||
      !cache.fonts.every((font) => typeof font === "string")
    ) {
      return { fonts: [], cached: false };
    }
    return { fonts: cache.fonts, cached: true };
  } catch {
    return { fonts: [], cached: false };
  }
}

function writeFontCache(fonts: string[]): void {
  if (!isTauriRuntime()) return;
  try {
    const cache: FontCache = { version: 1, fonts };
    window.localStorage.setItem(FONT_CACHE_KEY, JSON.stringify(cache));
  } catch {
    // localStorage can be unavailable or full; the in-memory cache still keeps this session fast.
  }
}

function readFontStore(): FontState {
  fontStore ??= readFontCache();
  return fontStore;
}

function updateFontStore(fonts: string[]): FontState {
  const nextState: FontState = { fonts, cached: true };
  fontStore = nextState;
  writeFontCache(fonts);
  return nextState;
}

function refreshSystemFonts(): Promise<string[]> {
  if (!isTauriRuntime()) return Promise.resolve(PREVIEW_FONTS);
  fontRefreshPromise ??= invoke<string[]>("list_system_fonts").finally(() => {
    fontRefreshPromise = null;
  });
  return fontRefreshPromise;
}

function fontListsMatch(left: string[], right: string[]): boolean {
  return (
    left.length === right.length &&
    left.every((font, index) => font === right[index])
  );
}

function displayName(value: string): string {
  return value === SYSTEM_FONT_VALUE ? "系统默认" : value;
}

function normalizeFontName(value: string): string {
  return value.trim().toLocaleLowerCase();
}

function buildFontLookup(fonts: string[]): Map<string, string> {
  const lookup = new Map<string, string>();
  fonts.forEach((font) => {
    const normalized = normalizeFontName(font);
    if (normalized && !lookup.has(normalized)) {
      lookup.set(normalized, font);
    }
  });
  return lookup;
}

function commonInstalledFonts(fontLookup: Map<string, string>): string[] {
  const result: string[] = [];
  const seen = new Set<string>();
  COMMON_UI_FONT_CANDIDATES.forEach((font) => {
    const installedFont = fontLookup.get(normalizeFontName(font));
    if (!installedFont) return;
    const normalized = normalizeFontName(installedFont);
    if (seen.has(normalized)) return;
    seen.add(normalized);
    result.push(installedFont);
  });
  return result;
}

function fontStyle(value: string): string {
  if (value === SYSTEM_FONT_VALUE) return SYSTEM_FONT_STACK;
  const escaped = value.replaceAll("\\", "\\\\").replaceAll('"', '\\"');
  return `"${escaped}", ${SYSTEM_FONT_STACK}`;
}

export function FontPicker({ value, onValueChange }: FontPickerProps) {
  const { pushToast } = useToast();
  const [fontState, setFontState] = useState<FontState>(readFontStore);
  const [open, setOpen] = useState<boolean>(false);
  const [query, setQuery] = useState<string>("");
  const [refreshing, setRefreshing] = useState<boolean>(false);
  const deferredQuery = useDeferredValue(query);
  const fonts = fontState.fonts;

  useEffect(() => {
    if (!open || fontState.cached) return;
    let disposed = false;
    setRefreshing(isTauriRuntime());
    void refreshSystemFonts()
      .then((result) => {
        if (disposed) return;
        setFontState((current) => {
          if (current.cached && fontListsMatch(current.fonts, result)) {
            return current;
          }
          return updateFontStore(result);
        });
      })
      .catch(() => {
        if (disposed) return;
        setFontState((current) => {
          if (current.fonts.length > 0) return current;
          return { fonts: PREVIEW_FONTS, cached: false };
        });
      })
      .finally(() => {
        if (!disposed) setRefreshing(false);
      });
    return () => {
      disposed = true;
    };
  }, [fontState.cached, open]);

  const availableFonts = useMemo(() => buildFontLookup(fonts), [fonts]);
  const options = useMemo(() => {
    const commonFonts = commonInstalledFonts(availableFonts);
    const commonFontNames = new Set(commonFonts.map(normalizeFontName));
    const selectedFontName = normalizeFontName(value);
    const selectedFont =
      value !== SYSTEM_FONT_VALUE &&
      !commonFontNames.has(selectedFontName) &&
      availableFonts.has(selectedFontName)
        ? [availableFonts.get(selectedFontName) ?? value]
        : [];
    return [SYSTEM_FONT_VALUE, ...selectedFont, ...commonFonts];
  }, [availableFonts, value]);
  const filtered = useMemo(() => {
    const normalized = normalizeFontName(deferredQuery.trim());
    return normalized
      ? options.filter((font) => normalizeFontName(displayName(font)).includes(normalized))
      : options;
  }, [options, deferredQuery]);
  const showLoadingState = fonts.length === 0 && refreshing;
  const selectedFontName = normalizeFontName(value);

  const handleSelectFont = useCallback(
    (font: string) => {
      const missing =
        font !== SYSTEM_FONT_VALUE &&
        fonts.length > 0 &&
        !availableFonts.has(normalizeFontName(font));
      if (missing) {
        pushToast("字体不存在。", "error");
        return;
      }
      onValueChange(font);
      setOpen(false);
    },
    [availableFonts, fonts.length, onValueChange, pushToast],
  );

  return (
    <Popover
      open={open}
      onOpenChange={(nextOpen) => {
        if (nextOpen) setFontState(readFontStore());
        setOpen(nextOpen);
        if (!nextOpen) setQuery("");
      }}
    >
      <PopoverTrigger asChild>
        <button
          type="button"
          className={cn(selectTriggerClassName, "justify-between font-normal")}
        >
          <span className="truncate" style={{ fontFamily: fontStyle(value) }}>
            {displayName(value)}
          </span>
          <ChevronDown className="size-4 shrink-0 text-muted-foreground" strokeWidth={1.8} />
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        sideOffset={4}
        className="w-[360px] overflow-hidden p-0"
      >
        <div className="border-b p-2">
          <div className="relative">
            <Search className="pointer-events-none absolute top-2 left-2.5 size-3.5 text-muted-foreground" />
            <Input
              autoFocus
              value={query}
              placeholder="搜索系统字体"
              className="pl-8"
              onChange={(event) => setQuery(event.target.value)}
            />
          </div>
        </div>
        <ScrollArea className="h-64">
          <div>
            {showLoadingState ? (
              <LoadingState label="正在加载可用界面字体" />
            ) : filtered.length === 0 ? (
              <div className="px-2 py-4 text-center text-xs text-muted-foreground">
                没有匹配的字体
              </div>
            ) : (
              filtered.map((font) => (
                <button
                  key={font}
                  type="button"
                  className={cn(selectItemClassName, "font-normal")}
                  onClick={() => handleSelectFont(font)}
                >
                  <span className="truncate" style={{ fontFamily: fontStyle(font) }}>
                    {displayName(font)}
                  </span>
                  <Check
                    className={cn(
                      "absolute right-3 size-3.5",
                      selectedFontName === normalizeFontName(font) ? "opacity-100" : "opacity-0",
                    )}
                  />
                </button>
              ))
            )}
          </div>
        </ScrollArea>
      </PopoverContent>
    </Popover>
  );
}
