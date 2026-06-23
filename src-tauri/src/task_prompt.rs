use serde::{Deserialize, Serialize};

use crate::domain::{UnifiedContent, UnifiedMessage};

pub const TARGET_LANGUAGE_PLACEHOLDER: &str = "[Target Language]";
const ASSISTANT_INSTRUCTIONS_HEADING: &str = "# Assistant Instructions";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DocumentFormat {
    Pdf,
    Markdown,
    Epub,
    Html,
    Txt,
    Json,
    Docx,
    Xlsx,
    Srt,
    Ass,
    Lrc,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ContentFormat {
    PlainText,
    Markdown,
    Html,
    Xhtml,
    Xml,
    Json,
    Srt,
    Ass,
    Lrc,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskChunkInput {
    pub text: String,
    pub document_format: DocumentFormat,
    pub content_format: ContentFormat,
}

pub fn validate_target_language(target_language: &str) -> Result<(), String> {
    if target_language.is_empty() || target_language.len() > 64 || !target_language.is_ascii() {
        return Err("Target language must be a valid BCP-47 code or English language name".into());
    }

    let mut subtags = target_language.split('-');
    let language = subtags.next().unwrap_or_default();
    let valid_bcp47 = (2..=8).contains(&language.len())
        && language.bytes().all(|byte| byte.is_ascii_alphabetic())
        && !subtags.any(|subtag| {
            subtag.is_empty()
                || subtag.len() > 8
                || !subtag.bytes().all(|byte| byte.is_ascii_alphanumeric())
        });
    let valid_english_name = target_language.split([' ', '-']).all(|word| {
        let mut bytes = word.bytes();
        word.len() >= 2
            && bytes.next().is_some_and(|byte| byte.is_ascii_uppercase())
            && bytes.all(|byte| byte.is_ascii_lowercase())
    });
    if !valid_bcp47 && !valid_english_name {
        return Err("Target language must be a valid BCP-47 code or English language name".into());
    }
    Ok(())
}

pub fn compose_system_prompt(
    target_language: &str,
    assistant_system_prompt: Option<&str>,
    mandatory_policy_precedence: &str,
    mandatory_prompt_template: &str,
) -> String {
    let mandatory_prompt =
        mandatory_prompt_template.replace(TARGET_LANGUAGE_PLACEHOLDER, target_language);
    match assistant_system_prompt
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty())
    {
        Some(assistant_prompt) => format!(
            "{ASSISTANT_INSTRUCTIONS_HEADING}\n{assistant_prompt}\n\n{mandatory_policy_precedence}\n\n{mandatory_prompt}"
        ),
        None => mandatory_prompt,
    }
}

pub fn system_user_messages(system_prompt: String, user_text: String) -> Vec<UnifiedMessage> {
    vec![
        text_message("system", system_prompt),
        text_message("user", user_text),
    ]
}

fn text_message(role: &str, text: String) -> UnifiedMessage {
    UnifiedMessage {
        role: role.into(),
        content: vec![UnifiedContent::Text { text }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serializes_all_document_and_content_formats() {
        let document_formats = [
            (DocumentFormat::Pdf, "pdf"),
            (DocumentFormat::Markdown, "markdown"),
            (DocumentFormat::Epub, "epub"),
            (DocumentFormat::Html, "html"),
            (DocumentFormat::Txt, "txt"),
            (DocumentFormat::Json, "json"),
            (DocumentFormat::Docx, "docx"),
            (DocumentFormat::Xlsx, "xlsx"),
            (DocumentFormat::Srt, "srt"),
            (DocumentFormat::Ass, "ass"),
            (DocumentFormat::Lrc, "lrc"),
        ];
        let content_formats = [
            (ContentFormat::PlainText, "plain-text"),
            (ContentFormat::Markdown, "markdown"),
            (ContentFormat::Html, "html"),
            (ContentFormat::Xhtml, "xhtml"),
            (ContentFormat::Xml, "xml"),
            (ContentFormat::Json, "json"),
            (ContentFormat::Srt, "srt"),
            (ContentFormat::Ass, "ass"),
            (ContentFormat::Lrc, "lrc"),
        ];

        for (format, expected) in document_formats {
            assert_eq!(serde_json::to_value(format).unwrap(), json!(expected));
        }
        for (format, expected) in content_formats {
            assert_eq!(serde_json::to_value(format).unwrap(), json!(expected));
        }
    }
}
