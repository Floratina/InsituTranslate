#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language {
    pub code: &'static str,
    pub name: &'static str,
}

pub const AUTO_LANGUAGE_CODE: &str = "auto";
pub const DEFAULT_SOURCE_LANGUAGE: &str = AUTO_LANGUAGE_CODE;
pub const DEFAULT_TARGET_LANGUAGE: &str = "zh-CN";

pub const LANGUAGES: &[Language] = &[
    Language { code: "zh-CN", name: "Chinese (Simplified)" },
    Language { code: "zh-HK", name: "Chinese (Traditional)" },
    Language { code: "ja", name: "Japanese" },
    Language { code: "ko", name: "Korean" },
    Language { code: "en", name: "English" },
    Language { code: "es", name: "Spanish" },
    Language { code: "fr", name: "French" },
    Language { code: "de", name: "German" },
    Language { code: "ru", name: "Russian" },
    Language { code: "it", name: "Italian" },
    Language { code: "pt-BR", name: "Portuguese (Brazil)" },
    Language { code: "pt-PT", name: "Portuguese (Portugal)" },
    Language { code: "nl", name: "Dutch" },
    Language { code: "pl", name: "Polish" },
    Language { code: "uk", name: "Ukrainian" },
    Language { code: "vi", name: "Vietnamese" },
    Language { code: "tr", name: "Turkish" },
    Language { code: "ar", name: "Arabic" },
    Language { code: "fa", name: "Persian" },
    Language { code: "hi", name: "Hindi" },
    Language { code: "bn", name: "Bengali" },
    Language { code: "th", name: "Thai" },
    Language { code: "id", name: "Indonesian" },
    Language { code: "ms", name: "Malay" },
    Language { code: "tl", name: "Tagalog" },
    Language { code: "sv", name: "Swedish" },
    Language { code: "no", name: "Norwegian" },
    Language { code: "da", name: "Danish" },
    Language { code: "fi", name: "Finnish" },
    Language { code: "cs", name: "Czech" },
    Language { code: "ro", name: "Romanian" },
    Language { code: "hu", name: "Hungarian" },
    Language { code: "el", name: "Greek" },
    Language { code: "he", name: "Hebrew" },
    Language { code: "la", name: "Latin" },
];

pub fn normalize_language_code(value: &str) -> Option<&'static str> {
    let normalized = value.trim();
    if normalized.eq_ignore_ascii_case(AUTO_LANGUAGE_CODE) {
        return Some(AUTO_LANGUAGE_CODE);
    }
    for language in LANGUAGES {
        if normalized.eq_ignore_ascii_case(language.code)
            || normalized.eq_ignore_ascii_case(language.name)
        {
            return Some(language.code);
        }
    }
    match normalized.to_ascii_lowercase().as_str() {
        "simplified chinese" | "chinese simplified" | "zh-hans" | "zh-hans-cn" => {
            Some("zh-CN")
        }
        "traditional chinese"
        | "chinese traditional"
        | "traditional chinese (taiwan)"
        | "traditional chinese (hong kong)"
        | "zh-hant"
        | "zh-hant-tw"
        | "zh-tw" => Some("zh-HK"),
        "portuguese" => Some("pt-BR"),
        _ => None,
    }
}

pub fn normalize_source_language(value: &str) -> Result<String, String> {
    normalize_language_code(value)
        .map(str::to_string)
        .ok_or_else(|| "Source language must be selected from the supported language list".into())
}

pub fn normalize_target_language(value: &str) -> Result<String, String> {
    let code = normalize_language_code(value)
        .ok_or_else(|| "Target language must be selected from the supported language list".to_string())?;
    if code == AUTO_LANGUAGE_CODE {
        return Err("Target language must be selected from the supported language list".into());
    }
    Ok(code.to_string())
}

pub fn target_language_name(value: &str) -> Result<&'static str, String> {
    let code = normalize_target_language(value)?;
    LANGUAGES
        .iter()
        .find(|language| language.code == code)
        .map(|language| language.name)
        .ok_or_else(|| "Target language must be selected from the supported language list".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_codes_and_legacy_names() {
        assert_eq!(normalize_language_code(" zh-CN "), Some("zh-CN"));
        assert_eq!(normalize_language_code("Simplified Chinese"), Some("zh-CN"));
        assert_eq!(normalize_language_code("Chinese (Traditional)"), Some("zh-HK"));
        assert_eq!(normalize_language_code("zh-TW"), Some("zh-HK"));
        assert_eq!(normalize_language_code("Korean"), Some("ko"));
        assert_eq!(normalize_language_code("__other__"), None);
    }

    #[test]
    fn maps_target_code_to_prompt_name() {
        assert_eq!(target_language_name("zh-CN").unwrap(), "Chinese (Simplified)");
        assert_eq!(target_language_name("ko").unwrap(), "Korean");
        assert!(target_language_name("auto").is_err());
    }
}
