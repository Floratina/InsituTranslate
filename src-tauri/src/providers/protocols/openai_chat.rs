use serde_json::{json, Value};

use crate::domain::{
    ProviderRuntimeConfig, RemoteModel, ThinkingConfig, ThinkingEffort, ThinkingMode,
    ThinkingSummary, UnifiedChatRequest, UnifiedChatResponse, UnifiedContent, UnifiedMessage,
};
use crate::features::{is_feature_supported, openai_chat_capabilities, FeatureId};
use crate::providers::capabilities::ModelCapabilities;
use crate::providers::codec::{
    openai_endpoint, EncodedRequest, EndpointPreview, HttpMethod, JsonEventStreamDecoder,
    ProtocolCodec, ProtocolStreamDecoder,
};
use crate::providers::shared::{
    append_openai_reasoning_details, filtered_token_logprobs, merge_custom_parameters,
    merge_object, remove_object_keys, set_optional_field, unified_response, usage_from_openai,
};
use crate::providers::thinking;

pub struct OpenAiChatCodec;

pub static CODEC: OpenAiChatCodec = OpenAiChatCodec;

impl ProtocolCodec for OpenAiChatCodec {
    fn id(&self) -> &'static str {
        "openai-chat"
    }

    fn encode_model_list(&self, config: &ProviderRuntimeConfig) -> Result<EncodedRequest, String> {
        Ok(EncodedRequest {
            method: HttpMethod::Get,
            url: openai_endpoint(config, "models"),
            headers: Vec::new(),
            body: None,
        })
    }

    fn decode_model_list(&self, raw: &Value) -> Result<Vec<RemoteModel>, String> {
        Ok(super::openai_responses::openai_models(raw))
    }

    fn encode_chat(
        &self,
        config: &ProviderRuntimeConfig,
        request: &UnifiedChatRequest,
    ) -> Result<EncodedRequest, String> {
        Ok(EncodedRequest {
            method: HttpMethod::Post,
            url: openai_endpoint(config, "chat/completions"),
            headers: Vec::new(),
            body: Some(build_body(&config.base_url, request)?),
        })
    }

    fn decode_chat(&self, raw: Value) -> Result<UnifiedChatResponse, String> {
        decode_chat(raw)
    }

    fn finish_reason(&self, raw: &Value) -> Option<String> {
        raw.pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn new_stream_decoder(&self) -> Box<dyn ProtocolStreamDecoder> {
        Box::new(JsonEventStreamDecoder::new(self.id(), decode_chat))
    }

    fn infer_capabilities(&self, base_url: &str, model_id: &str) -> ModelCapabilities {
        let inferred = openai_chat_capabilities(base_url, model_id);
        ModelCapabilities {
            reasoning: inferred.reasoning,
            web: inferred.web,
            thinking_efforts: self.supported_thinking_efforts(
                base_url,
                model_id,
                inferred.reasoning,
            ),
        }
    }

    fn supported_thinking_efforts(
        &self,
        base_url: &str,
        model_id: &str,
        reasoning: bool,
    ) -> Vec<ThinkingEffort> {
        crate::features::openai_chat_thinking_efforts(base_url, model_id, reasoning)
    }

    fn resolve_thinking(
        &self,
        base_url: &str,
        model_id: &str,
        effort: ThinkingEffort,
    ) -> ThinkingConfig {
        let mut config = thinking::base_config(effort);
        if is_feature_supported(FeatureId::OpenAiDeepSeekReasoningEffort, base_url, model_id) {
            config.effort = Some(thinking::deepseek_effort(effort));
        } else if is_feature_supported(FeatureId::OpenAiEnableThinking, base_url, model_id) {
            config.effort = Some(thinking::openai_effort(effort));
            if is_feature_supported(FeatureId::OpenAiThinkingBudget, base_url, model_id) {
                config.budget_tokens = Some(thinking::budget_tokens(effort));
            }
        } else if is_feature_supported(FeatureId::OpenAiReasoningEffort, base_url, model_id) {
            config.effort = Some(thinking::volc_effort(effort));
        } else {
            config.effort = Some(thinking::openai_effort(effort));
        }
        config
    }

    fn preview_endpoints(&self, config: &ProviderRuntimeConfig) -> Result<EndpointPreview, String> {
        Ok(EndpointPreview {
            chat: openai_endpoint(config, "chat/completions"),
            models: Some(openai_endpoint(config, "models")),
        })
    }
}

fn decode_chat(raw: Value) -> Result<UnifiedChatResponse, String> {
    let mut raw = raw;
    if raw.get("choices").is_none() {
        if let Some(data) = raw
            .get("data")
            .filter(|data| data.get("choices").is_some())
            .cloned()
        {
            raw = data;
        }
    }
    let message = raw
        .pointer("/choices/0/message")
        .or_else(|| raw.pointer("/choices/0/delta"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut reasoning = message
        .get("reasoning_content")
        .or_else(|| message.get("reasoning"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut thinking = if reasoning.is_empty() {
        Vec::new()
    } else {
        vec![UnifiedContent::Thinking {
            text: reasoning.clone(),
            signature: None,
            encrypted_data: None,
        }]
    };
    append_openai_reasoning_details(
        message.get("reasoning_details"),
        &mut reasoning,
        &mut thinking,
    );
    let mut logprobs = raw
        .pointer("/choices/0/logprobs/content")
        .and_then(Value::as_array)
        .map(|items| filtered_token_logprobs(items, "logprob"))
        .unwrap_or_default();
    if logprobs.is_empty() {
        if let Some(values) = raw
            .pointer("/choices/0/logprobs/token_logprobs")
            .and_then(Value::as_array)
        {
            logprobs.extend(
                values
                    .iter()
                    .filter_map(Value::as_f64)
                    .filter(|value| value.is_finite()),
            );
        }
    }
    let usage = usage_from_openai(raw.get("usage"));
    Ok(unified_response(
        raw, text, reasoning, thinking, usage, logprobs,
    ))
}

pub(crate) fn build_body(base_url: &str, request: &UnifiedChatRequest) -> Result<Value, String> {
    let cache_control =
        is_feature_supported(FeatureId::OpenAiCacheControl, base_url, &request.model);
    let mut body = json!({
        "model": request.model,
        "messages": openai_messages(&request.messages, cache_control, base_url, &request.model),
        "stream": request.stream
    });
    if request.stream {
        body["stream_options"] = json!({"include_usage": true});
    }
    if let Some(tokens) = request.max_output_tokens {
        if is_feature_supported(
            FeatureId::OpenAiOnlyMaxCompletionTokens,
            base_url,
            &request.model,
        ) {
            body["max_completion_tokens"] = json!(tokens);
        } else if is_feature_supported(FeatureId::OpenAiOnlyMaxTokens, base_url, &request.model) {
            body["max_tokens"] = json!(tokens);
        } else {
            body["max_tokens"] = json!(tokens);
            body["max_completion_tokens"] = json!(tokens);
        }
    }
    set_optional_field(
        &mut body,
        "temperature",
        request.temperature.map(Value::from),
    );
    set_optional_field(&mut body, "top_p", request.top_p.map(Value::from));
    apply_structured_overrides(&mut body, base_url, request);
    if request.logprobs && logprobs_supported(base_url) {
        body["logprobs"] = json!(true);
    }

    let mut body = merge_custom_parameters(body, &request.custom_parameters)?;
    apply_structured_overrides(&mut body, base_url, request);
    if request.logprobs && logprobs_supported(base_url) {
        body["logprobs"] = json!(true);
    } else {
        remove_object_keys(&mut body, &["logprobs", "top_logprobs"]);
    }
    Ok(body)
}

fn apply_structured_overrides(body: &mut Value, base_url: &str, request: &UnifiedChatRequest) {
    remove_object_keys(
        body,
        &[
            "temperature",
            "top_p",
            "reasoning",
            "thinking",
            "reasoning_effort",
            "disable_reasoning",
            "enable_thinking",
            "thinking_budget",
            "thinking_strategy",
            "clear_thinking",
            "reasoning_split",
            "web_search_options",
        ],
    );
    set_optional_field(body, "temperature", request.temperature.map(Value::from));
    set_optional_field(body, "top_p", request.top_p.map(Value::from));
    if let Some(thinking) = &request.thinking {
        merge_object(
            body,
            reasoning_params(
                base_url,
                &request.model,
                thinking,
                request.max_output_tokens,
            ),
        );
    }
    if is_feature_supported(FeatureId::OpenAiClearThinking, base_url, &request.model) {
        body["clear_thinking"] = json!(false);
    }
    if is_feature_supported(FeatureId::OpenAiReasoningSplit, base_url, &request.model) {
        body["reasoning_split"] = json!(true);
    }
    if request.web_search {
        body["web_search_options"] = json!({});
    }
}

fn openai_messages(
    messages: &[UnifiedMessage],
    cache_control: bool,
    base_url: &str,
    model: &str,
) -> Vec<Value> {
    let mut output = Vec::new();
    for message in messages {
        let mut text_parts = Vec::new();
        let mut reasoning_texts = Vec::new();
        let mut reasoning_details = Vec::new();
        for content in &message.content {
            match content {
                UnifiedContent::Text { text } => {
                    text_parts.push(json!({"type": "text", "text": text}));
                }
                UnifiedContent::CacheableText { text } => {
                    let mut part = json!({"type": "text", "text": text});
                    if cache_control {
                        part["cache_control"] = json!({"type": "ephemeral"});
                    }
                    text_parts.push(part);
                }
                UnifiedContent::Image { media_type, data } => text_parts.push(json!({
                    "type": "image_url",
                    "image_url": {"url": format!("data:{media_type};base64,{data}")}
                })),
                UnifiedContent::Thinking {
                    text,
                    signature,
                    encrypted_data,
                } if message.role == "assistant" => {
                    if is_feature_supported(FeatureId::OpenAiReasoningDetails, base_url, model) {
                        if let Some(data) = encrypted_data {
                            reasoning_details.push(json!({
                                "type": "reasoning.encrypted",
                                "data": data
                            }));
                        } else if !text.is_empty() {
                            let mut detail = json!({
                                "type": "reasoning.text",
                                "text": text
                            });
                            if let Some(signature) =
                                signature.as_deref().filter(|value| !value.is_empty())
                            {
                                detail["signature"] = json!(signature);
                            }
                            reasoning_details.push(detail);
                        }
                    } else if !text.is_empty() {
                        reasoning_texts.push(text.clone());
                    }
                }
                UnifiedContent::Thinking { .. } => {}
            }
        }
        let supports_reasoning_text =
            is_feature_supported(FeatureId::OpenAiReasoningField, base_url, model)
                || is_feature_supported(FeatureId::OpenAiReasoningContent, base_url, model);
        if text_parts.is_empty()
            && (reasoning_texts.is_empty() || !supports_reasoning_text)
            && reasoning_details.is_empty()
        {
            continue;
        }
        let content = if text_parts.is_empty() {
            Value::String(String::new())
        } else if text_parts.iter().all(is_plain_text_part) {
            Value::String(
                text_parts
                    .iter()
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            )
        } else {
            Value::Array(text_parts)
        };
        let mut item = json!({"role": message.role, "content": content});
        if !reasoning_details.is_empty() {
            item["reasoning_details"] = Value::Array(reasoning_details);
        } else if !reasoning_texts.is_empty() {
            let value = reasoning_texts.join("");
            if is_feature_supported(FeatureId::OpenAiReasoningField, base_url, model) {
                item["reasoning"] = json!(value);
            } else if is_feature_supported(FeatureId::OpenAiReasoningContent, base_url, model) {
                item["reasoning_content"] = json!(value);
            }
        }
        output.push(item);
    }
    output
}

fn is_plain_text_part(part: &Value) -> bool {
    part.get("type").and_then(Value::as_str) == Some("text")
        && part.get("text").and_then(Value::as_str).is_some()
        && part.get("cache_control").is_none()
}

pub(crate) fn reasoning_params(
    base_url: &str,
    model: &str,
    thinking: &ThinkingConfig,
    max_output_tokens: Option<u32>,
) -> Value {
    let disabled = thinking.mode == ThinkingMode::Disabled
        || thinking.effort == Some(ThinkingEffort::None)
        || thinking.budget_tokens == Some(0);
    let mut output = json!({});
    if is_feature_supported(FeatureId::OpenAiReasoningObject, base_url, model) {
        output["reasoning"] = if disabled {
            json!({"enabled": false})
        } else if let Some(tokens) = thinking.budget_tokens {
            json!({
                "max_tokens": max_output_tokens
                    .map(|max| tokens.min(max.saturating_sub(1)))
                    .unwrap_or(tokens)
            })
        } else if let Some(effort) = thinking.effort {
            json!({"effort": effort_name(effort)})
        } else {
            json!({"enabled": true})
        };
    } else if is_feature_supported(FeatureId::OpenAiThinkingObject, base_url, model) {
        output["thinking"] = json!({"type": if disabled { "disabled" } else { "enabled" }});
        if is_feature_supported(FeatureId::OpenAiDeepSeekReasoningEffort, base_url, model)
            && !disabled
        {
            if let Some(effort) = thinking.effort {
                output["reasoning_effort"] = json!(if matches!(
                    effort,
                    ThinkingEffort::Max | ThinkingEffort::Xhigh
                ) {
                    "max"
                } else {
                    "high"
                });
            }
        } else if is_feature_supported(FeatureId::OpenAiReasoningEffort, base_url, model) {
            output["reasoning_effort"] =
                json!(thinking.effort.map(effort_name).unwrap_or("medium"));
        }
    } else if is_feature_supported(FeatureId::OpenAiDisableReasoning, base_url, model) {
        output["disable_reasoning"] = json!(disabled);
    } else if is_feature_supported(FeatureId::OpenAiEnableThinking, base_url, model) {
        output["enable_thinking"] = json!(!disabled);
        if !disabled && is_feature_supported(FeatureId::OpenAiThinkingBudget, base_url, model) {
            if let Some(tokens) = thinking.budget_tokens {
                output["thinking_budget"] = json!(tokens);
            }
        }
    } else {
        output["reasoning_effort"] = json!(if disabled {
            "none"
        } else {
            thinking.effort.map(effort_name).unwrap_or("medium")
        });
    }
    if is_feature_supported(FeatureId::OpenAiThinkingStrategy, base_url, model) && !disabled {
        if let Some(summary) = thinking.summary {
            match summary {
                ThinkingSummary::Concise => output["thinking_strategy"] = json!("short_think"),
                ThinkingSummary::Detailed => output["thinking_strategy"] = json!("chain_of_draft"),
                ThinkingSummary::None | ThinkingSummary::Auto => {}
            }
        }
    }
    output
}

fn effort_name(effort: ThinkingEffort) -> &'static str {
    match effort {
        ThinkingEffort::None => "none",
        ThinkingEffort::Minimal => "minimal",
        ThinkingEffort::Low => "low",
        ThinkingEffort::Medium => "medium",
        ThinkingEffort::High => "high",
        ThinkingEffort::Xhigh | ThinkingEffort::Max => "xhigh",
    }
}

fn logprobs_supported(base_url: &str) -> bool {
    let host = url::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_default();
    host.is_empty()
        || (!(host.contains("dashscope") && host.ends_with("aliyuncs.com"))
            && !host.ends_with("modelscope.cn")
            && !host.contains(".modelscope.cn"))
}
