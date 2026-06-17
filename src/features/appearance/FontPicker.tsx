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
const FONT_CACHE_KEY = "insitu-system-fonts-v1";
const FONT_RENDER_BATCH_SIZE = 80;

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
  return value.toLocaleLowerCase();
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
  const [listReady, setListReady] = useState<boolean>(false);
  const [visibleCount, setVisibleCount] = useState<number>(FONT_RENDER_BATCH_SIZE);
  const deferredQuery = useDeferredValue(query);
  const fonts = fontState.fonts;

  useEffect(() => {
    if (!open) return;
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
  }, [open]);

  useEffect(() => {
    if (!open) {
      setListReady(false);
      setVisibleCount(FONT_RENDER_BATCH_SIZE);
      return;
    }
    setListReady(false);
    setVisibleCount(FONT_RENDER_BATCH_SIZE);
    let secondFrame = 0;
    const firstFrame = window.requestAnimationFrame(() => {
      secondFrame = window.requestAnimationFrame(() => setListReady(true));
    });
    return () => {
      window.cancelAnimationFrame(firstFrame);
      if (secondFrame) window.cancelAnimationFrame(secondFrame);
    };
  }, [open]);

  const availableFonts = useMemo(
    () => new Set(fonts.map(normalizeFontName)),
    [fonts],
  );
  const options = useMemo(() => {
    const selected = value !== SYSTEM_FONT_VALUE && !fonts.includes(value) ? [value] : [];
    return [SYSTEM_FONT_VALUE, ...selected, ...fonts];
  }, [fonts, value]);
  const filtered = useMemo(() => {
    const normalized = normalizeFontName(deferredQuery.trim());
    return normalized
      ? options.filter((font) => normalizeFontName(displayName(font)).includes(normalized))
      : options;
  }, [options, deferredQuery]);
  const visibleOptions = useMemo(
    () => filtered.slice(0, visibleCount),
    [filtered, visibleCount],
  );
  const loadingLabel = fonts.length === 0 ? "正在加载系统字体" : "正在准备字体列表";
  const showLoadingState = !listReady || (fonts.length === 0 && refreshing);

  useEffect(() => {
    if (!open || !listReady) return;
    setVisibleCount(FONT_RENDER_BATCH_SIZE);
  }, [deferredQuery, fonts, listReady, open]);

  useEffect(() => {
    if (!open || !listReady || visibleCount >= filtered.length) return;
    const timer = window.setTimeout(() => {
      setVisibleCount((current) =>
        Math.min(current + FONT_RENDER_BATCH_SIZE, filtered.length),
      );
    }, 16);
    return () => window.clearTimeout(timer);
  }, [filtered.length, listReady, open, visibleCount]);

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
              <LoadingState label={loadingLabel} />
            ) : filtered.length === 0 ? (
              <div className="px-2 py-4 text-center text-xs text-muted-foreground">
                没有匹配的字体
              </div>
            ) : (
              <>
                {visibleOptions.map((font) => (
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
                        value === font ? "opacity-100" : "opacity-0",
                      )}
                    />
                  </button>
                ))}
                {visibleOptions.length < filtered.length && (
                  <LoadingState compact label="继续加载字体" className="border-t" />
                )}
              </>
            )}
          </div>
        </ScrollArea>
      </PopoverContent>
    </Popover>
  );
}
