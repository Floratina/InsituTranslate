use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;

use crate::domain::{LogprobStats, UnifiedChatResponse, UnifiedContent, UnifiedUsage};

const PROTECTED_CUSTOM_PARAMETER_KEYS: &[&str] = &[
    "model",
    "messages",
    "message",
    "input",
    "instructions",
    "contents",
    "system",
    "systemInstruction",
    "system_instruction",
    "system_prompt",
    "systemPrompt",
    "prompt",
    "tools",
    "insituTools",
    "tool_choice",
    "toolChoice",
    "toolConfig",
    "tool_config",
    "stream",
    "stream_options",
    "streamOptions",
];

pub fn merge_custom_parameters(
    mut body: Value,
    custom_parameters: &Value,
) -> Result<Value, String> {
    if custom_parameters.is_null() {
        return Ok(body);
    }
    let Some(custom) = custom_parameters.as_object() else {
        return Err("Custom request body parameters must be a JSON object".into());
    };
    if custom.is_empty() {
        return Ok(body);
    }
    let Some(body_object) = body.as_object_mut() else {
        return Err("Provider request body must be a JSON object".into());
    };
    let sanitized = custom
        .iter()
        .filter(|(key, _)| !PROTECTED_CUSTOM_PARAMETER_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    deep_merge_object(body_object, &sanitized);
    Ok(body)
}

pub fn remove_object_keys(value: &mut Value, keys: &[&str]) {
    if let Some(object) = value.as_object_mut() {
        for key in keys {
            object.remove(*key);
        }
    }
}

pub fn set_optional_field(body: &mut Value, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        body[key] = value;
    }
}

pub fn merge_object(target: &mut Value, source: Value) {
    if let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
}

pub fn content_text(parts: &[UnifiedContent]) -> String {
    parts
        .iter()
        .filter_map(|part| match part {
            UnifiedContent::Text { text } | UnifiedContent::CacheableText { text } => {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn enable_openai_response_logprobs(body: &mut Value) {
    const INCLUDE: &str = "message.output_text.logprobs";
    let Some(object) = body.as_object_mut() else {
        return;
    };
    match object.get_mut("include") {
        Some(Value::Array(items)) => {
            if !items.iter().any(|item| item.as_str() == Some(INCLUDE)) {
                items.push(Value::String(INCLUDE.into()));
            }
        }
        Some(value @ Value::String(_)) => {
            let existing = value.as_str().unwrap_or_default().to_string();
            if existing != INCLUDE {
                *value = serde_json::json!([existing, INCLUDE]);
            }
        }
        Some(value) if value.is_null() => *value = serde_json::json!([INCLUDE]),
        Some(_) => {}
        None => {
            object.insert("include".into(), serde_json::json!([INCLUDE]));
        }
    }
}

pub fn disable_openai_response_logprobs(body: &mut Value) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    object.remove("logprobs");
    object.remove("top_logprobs");
    let remove_include = if let Some(Value::Array(items)) = object.get_mut("include") {
        items.retain(|item| item.as_str() != Some("message.output_text.logprobs"));
        items.is_empty()
    } else {
        false
    };
    if remove_include {
        object.remove("include");
    }
}

pub fn enable_gemini_logprobs(body: &mut Value) {
    if !body.get("generationConfig").is_some_and(Value::is_object) {
        body["generationConfig"] = serde_json::json!({});
    }
    if let Some(generation) = body
        .get_mut("generationConfig")
        .and_then(Value::as_object_mut)
    {
        generation.insert("responseLogprobs".into(), Value::Bool(true));
    }
}

pub fn disable_gemini_logprobs(body: &mut Value) {
    if let Some(object) = body.as_object_mut() {
        object.remove("logprobs");
        object.remove("top_logprobs");
        if let Some(generation) = object
            .get_mut("generationConfig")
            .and_then(Value::as_object_mut)
        {
            generation.remove("responseLogprobs");
            generation.remove("logprobs");
        }
    }
}

fn deep_merge_object(
    target: &mut serde_json::Map<String, Value>,
    incoming: &serde_json::Map<String, Value>,
) {
    for (key, value) in incoming {
        match (target.get_mut(key), value) {
            (Some(Value::Object(target_object)), Value::Object(incoming_object)) => {
                deep_merge_object(target_object, incoming_object);
            }
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

pub fn push_thinking_text(
    reasoning: &mut String,
    thinking: &mut Vec<UnifiedContent>,
    text: &str,
    signature: Option<String>,
) {
    if text.is_empty() {
        return;
    }
    reasoning.push_str(text);
    thinking.push(UnifiedContent::Thinking {
        text: text.to_string(),
        signature,
        encrypted_data: None,
    });
}

pub fn push_encrypted_thinking(thinking: &mut Vec<UnifiedContent>, encrypted_data: &str) {
    if !encrypted_data.is_empty() {
        thinking.push(UnifiedContent::Thinking {
            text: String::new(),
            signature: None,
            encrypted_data: Some(encrypted_data.to_string()),
        });
    }
}

pub fn append_openai_reasoning_details(
    value: Option<&Value>,
    reasoning: &mut String,
    thinking: &mut Vec<UnifiedContent>,
) {
    let Some(details) = value.and_then(Value::as_array) else {
        return;
    };
    for detail in details {
        match detail.get("type").and_then(Value::as_str) {
            Some("reasoning.encrypted") => {
                if let Some(data) = detail
                    .get("data")
                    .or_else(|| detail.get("encrypted_content"))
                    .and_then(Value::as_str)
                {
                    push_encrypted_thinking(thinking, data);
                }
            }
            Some("reasoning.text") | Some("reasoning.summary") => {
                if let Some(text) = detail.get("text").and_then(Value::as_str) {
                    push_thinking_text(
                        reasoning,
                        thinking,
                        text,
                        detail
                            .get("signature")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    );
                }
            }
            _ => {
                if let Some(text) = detail.get("text").and_then(Value::as_str) {
                    push_thinking_text(reasoning, thinking, text, None);
                }
            }
        }
    }
}

pub fn append_responses_output_item(
    item: &Value,
    text: &mut String,
    reasoning: &mut String,
    thinking: &mut Vec<UnifiedContent>,
) {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => {
            if let Some(content) = item.get("content").and_then(Value::as_array) {
                for part in content {
                    match part.get("type").and_then(Value::as_str) {
                        Some("output_text") | Some("refusal") => text
                            .push_str(part.get("text").and_then(Value::as_str).unwrap_or_default()),
                        Some("reasoning_text") | Some("summary_text") => push_thinking_text(
                            reasoning,
                            thinking,
                            part.get("text").and_then(Value::as_str).unwrap_or_default(),
                            None,
                        ),
                        _ => {}
                    }
                }
            }
        }
        Some("reasoning") => {
            if let Some(data) = item.get("encrypted_content").and_then(Value::as_str) {
                push_encrypted_thinking(thinking, data);
            }
            for key in ["summary", "content"] {
                if let Some(parts) = item.get(key).and_then(Value::as_array) {
                    for part in parts {
                        if let Some(value) = part.get("text").and_then(Value::as_str) {
                            push_thinking_text(reasoning, thinking, value, None);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

pub fn usage_from_openai(value: Option<&Value>) -> Option<UnifiedUsage> {
    value.map(|value| UnifiedUsage {
        input_tokens: value
            .get("prompt_tokens")
            .or_else(|| value.get("input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: value
            .get("completion_tokens")
            .or_else(|| value.get("output_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cached_tokens: value
            .pointer("/prompt_tokens_details/cached_tokens")
            .or_else(|| value.pointer("/input_tokens_details/cached_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
    })
}

pub fn filtered_token_logprobs(content: &[Value], logprob_field: &str) -> Vec<f64> {
    let mut output = Vec::new();
    let mut skipping_placeholder = false;
    for item in content {
        let token = item
            .get("token")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let trimmed = token.trim();
        let placeholder_piece = placeholder_tag_piece(trimmed);
        if skipping_placeholder || placeholder_piece {
            skipping_placeholder = !trimmed.contains('>');
            continue;
        }
        if !trimmed.is_empty() && !punctuation_regex().is_match(trimmed) {
            if let Some(logprob) = item.get(logprob_field).and_then(Value::as_f64) {
                if logprob.is_finite() {
                    output.push(logprob);
                }
            }
        }
    }
    output
}

pub fn unified_response(
    raw: Value,
    mut text: String,
    mut reasoning: String,
    mut thinking: Vec<UnifiedContent>,
    usage: Option<UnifiedUsage>,
    logprobs: Vec<f64>,
) -> UnifiedChatResponse {
    strip_leading_inline_thinking(&mut text, &mut reasoning, &mut thinking);
    UnifiedChatResponse {
        text,
        reasoning,
        thinking,
        usage,
        logprob_stats: confidence_index(&logprobs),
        raw,
    }
}

fn strip_leading_inline_thinking(
    text: &mut String,
    reasoning: &mut String,
    thinking: &mut Vec<UnifiedContent>,
) {
    let trimmed = text.trim_start();
    let leading_whitespace = text.len() - trimmed.len();
    let lower = trimmed.to_ascii_lowercase();
    let Some((open_tag, close_tag)) = [("<thinking>", "</thinking>"), ("<think>", "</think>")]
        .into_iter()
        .find(|(open, _)| lower.starts_with(open))
    else {
        return;
    };
    let content_start = leading_whitespace + open_tag.len();
    let after_open = &text[content_start..];
    let Some(close_start_relative) = after_open.to_ascii_lowercase().find(close_tag) else {
        let value = after_open.to_string();
        push_thinking_text(reasoning, thinking, value.trim(), None);
        text.clear();
        return;
    };
    let close_end = content_start + close_start_relative + close_tag.len();
    let value = text[content_start..content_start + close_start_relative].to_string();
    let remaining = text[close_end..].trim_start().to_string();
    push_thinking_text(reasoning, thinking, value.trim(), None);
    *text = remaining;
}

fn placeholder_tag_piece(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    if placeholder_tag_regex().is_match(token) {
        return true;
    }
    let lower = token.to_ascii_lowercase();
    lower == "<"
        || lower == "</"
        || lower == "/"
        || lower == ">"
        || lower.starts_with("<t")
        || lower.starts_with("</t")
}

fn placeholder_tag_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?i)^</?t\d+>$").expect("static placeholder regex"))
}

fn punctuation_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^\p{P}+$").expect("static punctuation regex"))
}

fn confidence_index(logprobs: &[f64]) -> Option<LogprobStats> {
    if logprobs.is_empty() {
        return None;
    }
    let probabilities = logprobs
        .iter()
        .map(|value| value.exp().clamp(0.0, 1.0))
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    if probabilities.is_empty() {
        return None;
    }
    let token_count = probabilities.len() as u64;
    let average_probability = probabilities.iter().sum::<f64>() / probabilities.len() as f64;
    let variance = probabilities
        .iter()
        .map(|value| {
            let difference = value - average_probability;
            difference * difference
        })
        .sum::<f64>()
        / probabilities.len() as f64;
    let standard_deviation = variance.sqrt();
    Some(LogprobStats {
        token_count,
        average_probability,
        standard_deviation,
        confidence: (average_probability - 0.5 * standard_deviation).clamp(0.0, 1.0),
    })
}
