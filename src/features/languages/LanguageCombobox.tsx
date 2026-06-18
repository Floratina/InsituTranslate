import { useMemo, useState } from "react";
import { Check, ChevronDown, Search } from "lucide-react";

import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  selectItemClassName,
  selectTriggerClassName,
} from "@/components/ui/select";
import {
  LanguageFlag,
  type LanguageFlagCode,
} from "@/features/translation/LanguageFlag";
import { cn } from "@/lib/utils";

import {
  AUTO_LANGUAGE_CODE,
  displayLanguage,
  LANGUAGES,
  languageSearchText,
  normalizeLanguageCode,
  type LanguageOption,
} from "./languageOptions";

interface LanguageComboboxProps {
  value: string;
  onValueChange: (value: string) => void;
  placeholder?: string;
  searchPlaceholder?: string;
  disabled?: boolean;
  includeAuto?: boolean;
  autoLabel?: string;
  allValue?: string;
  allLabel?: string;
  className?: string;
}

interface LanguagePickerItem {
  value: string;
  label: string;
  searchText: string;
  flagCode?: LanguageFlagCode;
}

const LANGUAGE_FLAG_CODES: Record<string, LanguageFlagCode> = {
  "zh-CN": "cn",
  "zh-HK": "hk",
  ja: "jp",
  ko: "kr",
  en: "gb",
  es: "es",
  fr: "fr",
  de: "de",
  ru: "ru",
  it: "it",
  "pt-BR": "br",
  "pt-PT": "pt",
  nl: "nl",
  pl: "pl",
  uk: "ua",
  vi: "vn",
  tr: "tr",
  ar: "sa",
  fa: "ir",
  hi: "in",
  bn: "bd",
  th: "th",
  id: "id",
  ms: "my",
  tl: "ph",
  sv: "se",
  no: "no",
  da: "dk",
  fi: "fi",
  cs: "cz",
  ro: "ro",
  hu: "hu",
  el: "gr",
  he: "il",
  la: "va",
};

function languageItem(language: LanguageOption): LanguagePickerItem {
  return {
    value: language.code,
    label: language.label,
    searchText: languageSearchText(language),
    flagCode: LANGUAGE_FLAG_CODES[language.code],
  };
}

export function LanguageCombobox({
  value,
  onValueChange,
  placeholder = "选择语言",
  searchPlaceholder = "搜索语言",
  disabled,
  includeAuto = false,
  autoLabel = "自动检测",
  allValue,
  allLabel,
  className,
}: LanguageComboboxProps) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");

  const options = useMemo<LanguagePickerItem[]>(() => {
    const items = LANGUAGES.map(languageItem);
    if (includeAuto) {
      items.unshift({
        value: AUTO_LANGUAGE_CODE,
        label: autoLabel,
        searchText: `自动检测 auto ${autoLabel}`.toLocaleLowerCase(),
      });
    }
    if (allValue && allLabel) {
      items.unshift({
        value: allValue,
        label: allLabel,
        searchText: allLabel.toLocaleLowerCase(),
      });
    }
    return items;
  }, [allLabel, allValue, autoLabel, includeAuto]);

  const selectedOption = useMemo(() => {
    if (allValue && value === allValue) {
      return options.find((option) => option.value === allValue) ?? null;
    }
    if (value === AUTO_LANGUAGE_CODE) {
      return options.find((option) => option.value === AUTO_LANGUAGE_CODE) ?? null;
    }
    const normalized = normalizeLanguageCode(value);
    return normalized
      ? (options.find((option) => option.value === normalized) ?? null)
      : null;
  }, [allValue, options, value]);

  const selectedLabel = useMemo(() => {
    if (selectedOption) return selectedOption.label;
    if (allValue && value === allValue) return allLabel ?? placeholder;
    if (value === AUTO_LANGUAGE_CODE) return autoLabel;
    const normalized = normalizeLanguageCode(value);
    return normalized ? displayLanguage(normalized) : placeholder;
  }, [allLabel, allValue, autoLabel, placeholder, selectedOption, value]);

  const filtered = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase();
    if (!normalizedQuery) return options;
    return options.filter((option) => option.searchText.includes(normalizedQuery));
  }, [options, query]);

  function selectValue(nextValue: string): void {
    onValueChange(nextValue);
    setOpen(false);
    setQuery("");
  }

  return (
    <Popover
      open={open}
      onOpenChange={(nextOpen) => {
        if (disabled) return;
        setOpen(nextOpen);
        if (!nextOpen) setQuery("");
      }}
    >
      <PopoverTrigger asChild>
        <button
          type="button"
          disabled={disabled}
          className={cn(selectTriggerClassName, "justify-between font-normal", className)}
        >
          <span className="flex min-w-0 flex-1 items-center gap-2 text-left">
            {selectedOption?.flagCode && (
              <LanguageFlag code={selectedOption.flagCode} />
            )}
            <span className="min-w-0 truncate">{selectedLabel}</span>
          </span>
          <ChevronDown className="size-4 shrink-0 text-muted-foreground" strokeWidth={1.8} />
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        collisionPadding={12}
        sideOffset={4}
        className="!w-72 max-w-[calc(100vw-1.5rem)] overflow-hidden p-0"
      >
        <div className="border-b p-2">
          <div className="relative">
            <Search className="pointer-events-none absolute top-2 left-2.5 size-3.5 text-muted-foreground" />
            <Input
              autoFocus
              value={query}
              placeholder={searchPlaceholder}
              className="pl-8"
              onChange={(event) => setQuery(event.target.value)}
            />
          </div>
        </div>
        <ScrollArea className="h-64">
          {filtered.length === 0 ? (
            <div className="px-2 py-4 text-center text-xs text-muted-foreground">
              没有匹配的语言
            </div>
          ) : (
            <div>
              {filtered.map((option) => (
                <button
                  key={option.value}
                  type="button"
                  className={cn(selectItemClassName, "font-normal")}
                  onClick={() => selectValue(option.value)}
                >
                  <span className="flex min-w-0 items-center gap-2">
                    {option.flagCode && <LanguageFlag code={option.flagCode} />}
                    <span className="truncate">{option.label}</span>
                  </span>
                  <Check
                    className={cn(
                      "absolute right-3 size-3.5",
                      value === option.value ? "opacity-100" : "opacity-0",
                    )}
                  />
                </button>
              ))}
            </div>
          )}
        </ScrollArea>
      </PopoverContent>
    </Popover>
  );
}
