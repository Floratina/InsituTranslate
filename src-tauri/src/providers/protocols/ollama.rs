use serde_json::{json, Value};

use crate::domain::{
    ProviderRuntimeConfig, RemoteModel, ThinkingConfig, ThinkingEffort, UnifiedChatRequest,
    UnifiedChatResponse, UnifiedContent,
};
use crate::providers::budget::{
    normalize_completion_budget, CompletionBudgetAlias, OLLAMA_ALIASES,
};
use crate::providers::codec::{
    append_endpoint_suffix, endpoint_base_url, EncodedRequest, EndpointPreview, HttpMethod,
    ProtocolCodec,
};
use crate::providers::shared::{
    content_text, merge_custom_parameters, normalize_usage, optional_usage_u64, remove_object_keys,
};

pub struct OllamaCodec;

pub static CODEC: OllamaCodec = OllamaCodec;

impl ProtocolCodec for OllamaCodec {
    fn id(&self) -> &'static str {
        "ollama"
    }

    fn completion_budget_aliases(&self) -> &'static [CompletionBudgetAlias] {
        OLLAMA_ALIASES
    }

    fn encode_model_list(&self, config: &ProviderRuntimeConfig) -> Result<EncodedRequest, String> {
        Ok(EncodedRequest {
            method: HttpMethod::Get,
            url: ollama_url(config, "tags"),
            headers: Vec::new(),
            body: None,
        })
    }

    fn decode_model_list(&self, raw: &Value) -> Result<Vec<RemoteModel>, String> {
        let mut models = raw
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let request_name = item
                    .get("name")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)?
                    .to_string();
                if request_name.is_empty() {
                    return None;
                }
                Some(RemoteModel {
                    alias: request_name.clone(),
                    request_name,
                    added: false,
                })
            })
            .collect::<Vec<_>>();
        models.sort_by(|left, right| left.request_name.cmp(&right.request_name));
        Ok(models)
    }

    fn encode_chat(
        &self,
        config: &ProviderRuntimeConfig,
        request: &UnifiedChatRequest,
    ) -> Result<EncodedRequest, String> {
        Ok(EncodedRequest {
            method: HttpMethod::Post,
            url: ollama_url(config, "chat"),
            headers: Vec::new(),
            body: Some(build_body(request)?),
        })
    }

    fn decode_chat(&self, raw: Value) -> Result<UnifiedChatResponse, String> {
        decode_chat(raw)
    }

    fn finish_reason(&self, raw: &Value) -> Option<String> {
        raw.get("done_reason")
            .or_else(|| raw.get("doneReason"))
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn preview_endpoints(&self, config: &ProviderRuntimeConfig) -> Result<EndpointPreview, String> {
        Ok(EndpointPreview {
            chat: ollama_url(config, "chat"),
            models: Some(ollama_url(config, "tags")),
        })
    }
}

fn decode_chat(raw: Value) -> Result<UnifiedChatResponse, String> {
    let text = raw
        .pointer("/message/content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let reasoning = raw
        .pointer("/message/thinking")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let thinking = if reasoning.is_empty() {
        Vec::new()
    } else {
        vec![UnifiedContent::Thinking {
            text: reasoning.clone(),
            signature: None,
            encrypted_data: None,
        }]
    };
    let input_tokens = optional_usage_u64(&raw, "/prompt_eval_count", "Ollama")?;
    let output_tokens = optional_usage_u64(&raw, "/eval_count", "Ollama")?;
    let usage = if input_tokens.is_some() || output_tokens.is_some() {
        Some(normalize_usage(
            "Ollama",
            input_tokens,
            output_tokens,
            None,
            None,
            None,
            true,
        )?)
    } else {
        None
    };
    Ok(crate::providers::shared::unified_response(
        raw,
        text,
        reasoning,
        thinking,
        usage,
        Vec::new(),
    ))
}

pub(crate) fn build_body(request: &UnifiedChatRequest) -> Result<Value, String> {
    let completion_budget = normalize_completion_budget(
        request.max_output_tokens,
        &request.custom_parameters,
        OLLAMA_ALIASES,
    )?;
    let max_output_tokens = completion_budget.resolved_max_output_tokens(None);
    let messages = request
        .messages
        .iter()
        .map(|message| {
            let mut item = json!({
                "role": message.role,
                "content": content_text(&message.content)
            });
            let thinking = message
                .content
                .iter()
                .filter_map(|part| match part {
                    UnifiedContent::Thinking { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            if !thinking.is_empty() && message.role == "assistant" {
                item["thinking"] = json!(thinking);
            }
            item
        })
        .collect::<Vec<_>>();
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "stream": false
    });
    if let Some(thinking) = &request.thinking {
        body["think"] = think_value(thinking);
    }
    let mut options = json!({});
    if let Some(temperature) = request.temperature {
        options["temperature"] = json!(temperature);
    }
    if let Some(top_p) = request.top_p {
        options["top_p"] = json!(top_p);
    }
    if options.as_object().is_some_and(|value| !value.is_empty()) {
        body["options"] = options;
    }

    let mut body = merge_custom_parameters(body, completion_budget.custom_parameters())?;
    remove_object_keys(
        &mut body,
        &[
            "think",
            "thinking",
            "reasoning",
            "temperature",
            "top_p",
            "topP",
            "web_search_options",
            "tools",
            "logprobs",
            "top_logprobs",
        ],
    );
    if let Some(thinking) = &request.thinking {
        body["think"] = think_value(thinking);
    }
    if !body.get("options").is_some_and(Value::is_object) {
        body["options"] = json!({});
    }
    if let Some(options) = body.get_mut("options").and_then(Value::as_object_mut) {
        options.remove("temperature");
        options.remove("top_p");
        options.remove("num_predict");
        if let Some(tokens) = max_output_tokens {
            options.insert("num_predict".into(), json!(tokens));
        }
        if let Some(temperature) = request.temperature {
            options.insert("temperature".into(), json!(temperature));
        }
        if let Some(top_p) = request.top_p {
            options.insert("top_p".into(), json!(top_p));
        }
    }
    if body
        .get("options")
        .and_then(Value::as_object)
        .is_some_and(serde_json::Map::is_empty)
    {
        remove_object_keys(&mut body, &["options"]);
    }
    Ok(body)
}

fn think_value(thinking: &ThinkingConfig) -> Value {
    match thinking.effort {
        Some(ThinkingEffort::None) => json!(false),
        Some(ThinkingEffort::Minimal | ThinkingEffort::Low) => json!("low"),
        Some(ThinkingEffort::Medium) => json!("medium"),
        Some(ThinkingEffort::High | ThinkingEffort::Xhigh | ThinkingEffort::Max) => json!("high"),
        None => json!(thinking.mode != crate::domain::ThinkingMode::Disabled),
    }
}

fn ollama_url(config: &ProviderRuntimeConfig, suffix: &str) -> String {
    let base = endpoint_base_url(&config.base_url).trim_end_matches('/');
    if config.use_raw_base_url {
        append_endpoint_suffix(base, suffix)
    } else if base.ends_with("/api") {
        format!("{base}/{suffix}")
    } else {
        format!("{base}/api/{suffix}")
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[test]
    fn ollama_usage_marks_generated_count_as_unsplit() {
        let response = decode_chat(json!({
            "message": {"content": "done", "thinking": "reason"},
            "prompt_eval_count": 11,
            "eval_count": 18
        }))
        .expect("valid response");
        let usage = response.usage.expect("usage");

        assert_eq!(usage.input_tokens, 11);
        assert_eq!(usage.output_tokens, 18);
        assert_eq!(usage.thinking_tokens, 0);
        assert_eq!(usage.total_tokens, 29);
        assert!(usage.provenance.output_includes_unreported_thinking);
    }

    #[test]
    fn ollama_response_without_counts_has_no_usage() {
        let response =
            decode_chat(json!({"message": {"content": "done"}})).expect("valid response");
        assert!(response.usage.is_none());
    }
}
