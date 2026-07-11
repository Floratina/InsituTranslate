import { useCallback, useDeferredValue, useEffect, useMemo, useState } from "react";
import { Check, ChevronDown, Search } from "lucide-react";

import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  selectItemClassName,
  selectTriggerClassName,
} from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { useToast } from "@/components/ui/toast-stack";
import {
  getCachedSystemFonts,
  isTauriRuntime,
  refreshSystemFontsCache,
  type FontCacheRefresh,
} from "@/features/appearance/api";
import { SYSTEM_FONT_VALUE } from "@/features/appearance/constants";
import { cn } from "@/lib/utils";

const PREVIEW_FONTS = [
  "Arial",
  "Calibri",
  "Microsoft YaHei",
  "Segoe UI",
  "SimSun",
  "Times New Roman",
];

interface FontState {
  fonts: string[];
  cacheLoaded: boolean;
  refreshing: boolean;
}

let fontStore: string[] | null = null;
let fontCachePromise: Promise<string[]> | null = null;
let fontRefreshPromise: Promise<FontCacheRefresh> | null = null;

interface FontPickerProps {
  value: string;
  onValueChange: (value: string) => void;
}

function initialFontState(): FontState {
  if (!isTauriRuntime()) {
    fontStore ??= PREVIEW_FONTS;
    return { fonts: fontStore, cacheLoaded: true, refreshing: false };
  }
  return {
    fonts: fontStore ?? [],
    cacheLoaded: fontStore !== null,
    refreshing: false,
  };
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

function loadCachedFonts(): Promise<string[]> {
  if (!isTauriRuntime()) return Promise.resolve(PREVIEW_FONTS);
  fontCachePromise ??= getCachedSystemFonts().finally(() => {
    fontCachePromise = null;
  });
  return fontCachePromise;
}

function refreshFontCache(): Promise<FontCacheRefresh> {
  if (!isTauriRuntime()) {
    return Promise.resolve({ changed: true, fonts: PREVIEW_FONTS });
  }
  fontRefreshPromise ??= refreshSystemFontsCache().finally(() => {
    fontRefreshPromise = null;
  });
  return fontRefreshPromise;
}

function FontListSkeleton() {
  return (
    <div className="grid gap-1 p-2" aria-label="正在加载可用界面字体">
      {Array.from({ length: 8 }).map((_, index) => (
        <Skeleton key={index} className="h-8 w-full rounded-[6px]" />
      ))}
    </div>
  );
}

export function FontPicker({ value, onValueChange }: FontPickerProps) {
  const { pushToast } = useToast();
  const [fontState, setFontState] = useState<FontState>(initialFontState);
  const [open, setOpen] = useState<boolean>(false);
  const [query, setQuery] = useState<string>("");
  const deferredQuery = useDeferredValue(query);
  const fonts = fontState.fonts;

  useEffect(() => {
    if (!open) return;
    if (!isTauriRuntime()) {
      setFontState({
        fonts: PREVIEW_FONTS,
        cacheLoaded: true,
        refreshing: false,
      });
      return;
    }

    let disposed = false;

    if (!fontState.cacheLoaded) {
      void loadCachedFonts()
        .then((cachedFonts) => {
          if (disposed) return;
          fontStore = cachedFonts;
          setFontState((current) => ({
            ...current,
            fonts: cachedFonts,
            cacheLoaded: true,
          }));
        })
        .catch((error: unknown) => {
          console.error("Unable to load cached system fonts.", error);
          if (!disposed) {
            setFontState((current) => ({ ...current, cacheLoaded: true }));
          }
        });
    }

    setFontState((current) => ({ ...current, refreshing: true }));
    void refreshFontCache()
      .then((result) => {
        if (disposed) return;
        if (result.changed && result.fonts) {
          fontStore = result.fonts;
          setFontState({
            fonts: result.fonts,
            cacheLoaded: true,
            refreshing: false,
          });
          return;
        }
        setFontState((current) => ({
          ...current,
          cacheLoaded: true,
          refreshing: false,
        }));
      })
      .catch((error: unknown) => {
        console.error("Unable to refresh system fonts.", error);
        if (!disposed) {
          setFontState((current) => ({
            ...current,
            cacheLoaded: true,
            refreshing: false,
          }));
        }
      });

    return () => {
      disposed = true;
    };
  }, [fontState.cacheLoaded, open]);

  const availableFonts = useMemo(() => buildFontLookup(fonts), [fonts]);
  const options = useMemo(() => {
    const selectedFontName = normalizeFontName(value);
    const selectedFont =
      value !== SYSTEM_FONT_VALUE &&
      selectedFontName &&
      !availableFonts.has(selectedFontName)
        ? [value]
        : [];
    return [SYSTEM_FONT_VALUE, ...selectedFont, ...fonts];
  }, [availableFonts, fonts, value]);
  const filtered = useMemo(() => {
    const normalized = normalizeFontName(deferredQuery.trim());
    return normalized
      ? options.filter((font) => normalizeFontName(displayName(font)).includes(normalized))
      : options;
  }, [options, deferredQuery]);
  const showLoadingState = fonts.length === 0 && (!fontState.cacheLoaded || fontState.refreshing);
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
        if (nextOpen && fontStore) {
          setFontState((current) => ({ ...current, fonts: fontStore ?? current.fonts }));
        }
        setOpen(nextOpen);
        if (!nextOpen) setQuery("");
      }}
    >
      <PopoverTrigger asChild>
        <button
          type="button"
          className={cn(selectTriggerClassName, "justify-between font-normal")}
        >
          <span className="truncate">{displayName(value)}</span>
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
        <ScrollArea className="h-64" viewportClassName="overscroll-contain">
          <div>
            {showLoadingState ? (
              <FontListSkeleton />
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
                  <span className="truncate">{displayName(font)}</span>
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
