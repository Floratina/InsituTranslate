export interface LanguageOption {
  code: string;
  name: string;
  nativeName: string;
  flag: string;
  label: string;
}

export const AUTO_LANGUAGE_CODE = "auto";

export const LANGUAGES: LanguageOption[] = [
  { code: "zh-CN", name: "Chinese (Simplified)", nativeName: "简体中文", flag: "🇨🇳", label: "简体中文" },
  { code: "zh-HK", name: "Chinese (Traditional)", nativeName: "繁體中文", flag: "🇭🇰", label: "繁体中文" },
  { code: "ja", name: "Japanese", nativeName: "日本語", flag: "🇯🇵", label: "日语" },
  { code: "ko", name: "Korean", nativeName: "한국어", flag: "🇰🇷", label: "韩语" },
  { code: "en", name: "English", nativeName: "English", flag: "🇬🇧", label: "英语" },
  { code: "es", name: "Spanish", nativeName: "Español", flag: "🇪🇸", label: "西班牙语" },
  { code: "fr", name: "French", nativeName: "Français", flag: "🇫🇷", label: "法语" },
  { code: "de", name: "German", nativeName: "Deutsch", flag: "🇩🇪", label: "德语" },
  { code: "ru", name: "Russian", nativeName: "Русский", flag: "🇷🇺", label: "俄语" },
  { code: "it", name: "Italian", nativeName: "Italiano", flag: "🇮🇹", label: "意大利语" },
  { code: "pt-BR", name: "Portuguese (Brazil)", nativeName: "Português (Brasil)", flag: "🇧🇷", label: "葡萄牙语（巴西）" },
  { code: "pt-PT", name: "Portuguese (Portugal)", nativeName: "Português (Portugal)", flag: "🇵🇹", label: "葡萄牙语（葡萄牙）" },
  { code: "nl", name: "Dutch", nativeName: "Nederlands", flag: "🇳🇱", label: "荷兰语" },
  { code: "pl", name: "Polish", nativeName: "Polski", flag: "🇵🇱", label: "波兰语" },
  { code: "uk", name: "Ukrainian", nativeName: "Українська", flag: "🇺🇦", label: "乌克兰语" },
  { code: "vi", name: "Vietnamese", nativeName: "Tiếng Việt", flag: "🇻🇳", label: "越南语" },
  { code: "tr", name: "Turkish", nativeName: "Türkçe", flag: "🇹🇷", label: "土耳其语" },
  { code: "ar", name: "Arabic", nativeName: "العربية", flag: "🇸🇦", label: "阿拉伯语" },
  { code: "fa", name: "Persian", nativeName: "فارسی", flag: "🇮🇷", label: "波斯语" },
  { code: "hi", name: "Hindi", nativeName: "हिन्दी", flag: "🇮🇳", label: "印地语" },
  { code: "bn", name: "Bengali", nativeName: "বাংলা", flag: "🇧🇩", label: "孟加拉语" },
  { code: "th", name: "Thai", nativeName: "ไทย", flag: "🇹🇭", label: "泰语" },
  { code: "id", name: "Indonesian", nativeName: "Bahasa Indonesia", flag: "🇮🇩", label: "印度尼西亚语" },
  { code: "ms", name: "Malay", nativeName: "Bahasa Melayu", flag: "🇲🇾", label: "马来语" },
  { code: "tl", name: "Tagalog", nativeName: "Tagalog", flag: "🇵🇭", label: "他加禄语" },
  { code: "sv", name: "Swedish", nativeName: "Svenska", flag: "🇸🇪", label: "瑞典语" },
  { code: "no", name: "Norwegian", nativeName: "Norsk", flag: "🇳🇴", label: "挪威语" },
  { code: "da", name: "Danish", nativeName: "Dansk", flag: "🇩🇰", label: "丹麦语" },
  { code: "fi", name: "Finnish", nativeName: "Suomi", flag: "🇫🇮", label: "芬兰语" },
  { code: "cs", name: "Czech", nativeName: "Čeština", flag: "🇨🇿", label: "捷克语" },
  { code: "ro", name: "Romanian", nativeName: "Română", flag: "🇷🇴", label: "罗马尼亚语" },
  { code: "hu", name: "Hungarian", nativeName: "Magyar", flag: "🇭🇺", label: "匈牙利语" },
  { code: "el", name: "Greek", nativeName: "Ελληνικά", flag: "🇬🇷", label: "希腊语" },
  { code: "he", name: "Hebrew", nativeName: "עברית", flag: "🇮🇱", label: "希伯来语" },
  { code: "la", name: "Latin", nativeName: "Lingua Latina", flag: "🇻🇦", label: "拉丁语" },
];

const codeMap = new Map(LANGUAGES.map((language) => [language.code.toLowerCase(), language]));
const aliasMap = new Map<string, string>([
  ["simplified chinese", "zh-CN"],
  ["chinese (simplified)", "zh-CN"],
  ["zh-hans", "zh-CN"],
  ["zh-cn", "zh-CN"],
  ["traditional chinese", "zh-HK"],
  ["chinese (traditional)", "zh-HK"],
  ["traditional chinese (taiwan)", "zh-HK"],
  ["traditional chinese (hong kong)", "zh-HK"],
  ["zh-hant", "zh-HK"],
  ["zh-hant-tw", "zh-HK"],
  ["zh-tw", "zh-HK"],
  ["zh-hk", "zh-HK"],
  ["portuguese", "pt-BR"],
]);

for (const language of LANGUAGES) {
  aliasMap.set(language.name.toLowerCase(), language.code);
  aliasMap.set(language.code.toLowerCase(), language.code);
}

export function normalizeLanguageCode(value: string | null | undefined): string | null {
  const normalized = value?.trim();
  if (!normalized) return null;
  if (normalized.toLowerCase() === AUTO_LANGUAGE_CODE) return AUTO_LANGUAGE_CODE;
  return aliasMap.get(normalized.toLowerCase()) ?? null;
}

export function getLanguageOption(value: string | null | undefined): LanguageOption | null {
  const code = normalizeLanguageCode(value);
  if (!code || code === AUTO_LANGUAGE_CODE) return null;
  return codeMap.get(code.toLowerCase()) ?? null;
}

export function displayLanguage(value: string | null | undefined): string {
  const trimmed = value?.trim();
  if (!trimmed) return "";
  if (trimmed.toLowerCase() === AUTO_LANGUAGE_CODE) return "自动检测";
  return getLanguageOption(trimmed)?.label ?? trimmed;
}

export function displayLanguagePair(
  sourceLanguage: string,
  targetLanguage: string,
): string {
  return `${displayLanguage(sourceLanguage)} - ${displayLanguage(targetLanguage)}`;
}

export function sameLanguage(left: string, right: string): boolean {
  const leftCode = normalizeLanguageCode(left);
  const rightCode = normalizeLanguageCode(right);
  if (leftCode && rightCode) return leftCode === rightCode;
  return left.trim().toLowerCase() === right.trim().toLowerCase();
}

export function languageSearchText(language: LanguageOption): string {
  return [
    language.label,
    language.name,
    language.nativeName,
    language.code,
  ].join(" ").toLocaleLowerCase();
}
