use serde_json::{json, Value};

use crate::domain::{
    ProviderRuntimeConfig, RemoteModel, ThinkingConfig, ThinkingEffort, ThinkingMode,
    UnifiedChatRequest, UnifiedChatResponse, UnifiedContent, UnifiedUsage,
};
use crate::features::{is_feature_supported, FeatureId};
use crate::providers::capabilities::ModelCapabilities;
use crate::providers::codec::{
    append_endpoint_suffix, EncodedRequest, EndpointPreview, HttpMethod, JsonEventStreamDecoder,
    ProtocolCodec, ProtocolStreamDecoder,
};
use crate::providers::shared::{
    disable_gemini_logprobs, enable_gemini_logprobs, merge_custom_parameters, push_thinking_text,
    remove_object_keys, unified_response,
};
use crate::providers::thinking;

pub struct GeminiCodec;

pub static CODEC: GeminiCodec = GeminiCodec;

impl ProtocolCodec for GeminiCodec {
    fn id(&self) -> &'static str {
        "gemini"
    }

    fn encode_model_list(&self, config: &ProviderRuntimeConfig) -> Result<EncodedRequest, String> {
        let suffix = if config.use_raw_base_url {
            "models"
        } else {
            "v1beta/models"
        };
        Ok(EncodedRequest {
            method: HttpMethod::Get,
            url: append_endpoint_suffix(&config.base_url, suffix),
            headers: Vec::new(),
            body: None,
        })
    }

    fn decode_model_list(&self, raw: &Value) -> Result<Vec<RemoteModel>, String> {
        Ok(google_models(raw, false, false))
    }

    fn encode_chat(
        &self,
        config: &ProviderRuntimeConfig,
        request: &UnifiedChatRequest,
    ) -> Result<EncodedRequest, String> {
        let suffix = format!(
            "{}models/{}:{}",
            if config.use_raw_base_url {
                ""
            } else {
                "v1beta/"
            },
            request.model.trim_start_matches("models/"),
            if request.stream {
                "streamGenerateContent?alt=sse"
            } else {
                "generateContent"
            }
        );
        Ok(EncodedRequest {
            method: HttpMethod::Post,
            url: append_endpoint_suffix(&config.base_url, &suffix),
            headers: Vec::new(),
            body: Some(build_body(&config.base_url, request)?),
        })
    }

    fn decode_chat(&self, raw: Value) -> Result<UnifiedChatResponse, String> {
        decode_chat(raw)
    }

    fn finish_reason(&self, raw: &Value) -> Option<String> {
        finish_reason(raw)
    }

    fn new_stream_decoder(&self) -> Box<dyn ProtocolStreamDecoder> {
        Box::new(JsonEventStreamDecoder::new(self.id(), decode_chat))
    }

    fn infer_capabilities(&self, base_url: &str, model_id: &str) -> ModelCapabilities {
        let inferred = crate::features::gemini_capabilities(model_id);
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
        crate::features::gemini_thinking_efforts(base_url, model_id, reasoning)
    }

    fn resolve_thinking(
        &self,
        base_url: &str,
        model_id: &str,
        effort: ThinkingEffort,
    ) -> ThinkingConfig {
        let mut config = thinking::base_config(effort);
        if is_feature_supported(FeatureId::GeminiThinkingLevel, base_url, model_id) {
            config.effort = Some(thinking::gemini_level_effort(effort));
        } else {
            config.budget_tokens = Some(thinking::budget_tokens(effort));
        }
        config
    }

    fn preview_endpoints(&self, config: &ProviderRuntimeConfig) -> Result<EndpointPreview, String> {
        let chat_suffix = format!(
            "{}models/{{model}}:generateContent",
            if config.use_raw_base_url {
                ""
            } else {
                "v1beta/"
            }
        );
        let model_suffix = if config.use_raw_base_url {
            "models"
        } else {
            "v1beta/models"
        };
        Ok(EndpointPreview {
            chat: append_endpoint_suffix(&config.base_url, &chat_suffix),
            models: Some(append_endpoint_suffix(&config.base_url, model_suffix)),
        })
    }
}

pub(super) fn decode_chat(raw: Value) -> Result<UnifiedChatResponse, String> {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut thinking = Vec::new();
    if let Some(parts) = raw
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
    {
        for part in parts {
            if part.get("thought").and_then(Value::as_bool) == Some(true) {
                push_thinking_text(
                    &mut reasoning,
                    &mut thinking,
                    part.get("text").and_then(Value::as_str).unwrap_or_default(),
                    part.get("thoughtSignature")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                );
            } else {
                text.push_str(part.get("text").and_then(Value::as_str).unwrap_or_default());
            }
        }
    }
    let usage = raw.get("usageMetadata").map(|value| UnifiedUsage {
        input_tokens: value
            .get("promptTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: value
            .get("candidatesTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            + value
                .get("thoughtsTokenCount")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        cached_tokens: value
            .get("cachedContentTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    });
    let logprobs = raw
        .pointer("/candidates/0/logprobsResult/chosenCandidates")
        .or_else(|| raw.pointer("/candidates/0/logprobs_result/chosen_candidates"))
        .and_then(Value::as_array)
        .map(|items| {
            let mut values =
                crate::providers::shared::filtered_token_logprobs(items, "logProbability");
            if values.is_empty() {
                values =
                    crate::providers::shared::filtered_token_logprobs(items, "log_probability");
            }
            values
        })
        .unwrap_or_default();
    Ok(unified_response(
        raw, text, reasoning, thinking, usage, logprobs,
    ))
}

pub(super) fn finish_reason(raw: &Value) -> Option<String> {
    raw.pointer("/candidates/0/finishReason")
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub(crate) fn build_body(base_url: &str, request: &UnifiedChatRequest) -> Result<Value, String> {
    let mut system_parts = Vec::new();
    let mut contents = Vec::new();
    for message in &request.messages {
        let parts = message
            .content
            .iter()
            .map(|part| match part {
                UnifiedContent::Text { text } | UnifiedContent::CacheableText { text } => {
                    json!({"text": text})
                }
                UnifiedContent::Image { media_type, data } => {
                    json!({"inlineData": {"mimeType": media_type, "data": data}})
                }
                UnifiedContent::Thinking {
                    text, signature, ..
                } => {
                    let mut part = json!({"text": text, "thought": true});
                    if let Some(signature) = signature.as_deref().filter(|value| !value.is_empty())
                    {
                        part["thoughtSignature"] = json!(signature);
                    }
                    part
                }
            })
            .collect::<Vec<_>>();
        if message.role == "system" {
            system_parts.extend(parts);
        } else if !parts.is_empty() {
            contents.push(json!({
                "role": if message.role == "assistant" { "model" } else { "user" },
                "parts": parts
            }));
        }
    }
    let mut generation = json!({});
    if let Some(tokens) = request.max_output_tokens {
        generation["maxOutputTokens"] = json!(tokens);
    }
    if let Some(temperature) = request.temperature {
        generation["temperature"] = json!(temperature);
    }
    if let Some(top_p) = request.top_p {
        generation["topP"] = json!(top_p);
    }
    if let Some(thinking) = &request.thinking {
        generation["thinkingConfig"] = thinking_config(base_url, &request.model, thinking);
    }
    if request.logprobs {
        generation["responseLogprobs"] = json!(true);
    }
    let mut body = json!({"contents": contents, "generationConfig": generation});
    if !system_parts.is_empty() {
        body["systemInstruction"] = json!({"parts": system_parts});
    }
    if request.web_search {
        body["tools"] = json!([{"googleSearch": {}}]);
    }

    let mut body = merge_custom_parameters(body, &request.custom_parameters)?;
    remove_object_keys(
        &mut body,
        &[
            "temperature",
            "top_p",
            "topP",
            "thinking",
            "reasoning",
            "web_search_options",
        ],
    );
    if !body.get("generationConfig").is_some_and(Value::is_object) {
        body["generationConfig"] = json!({});
    }
    if let Some(generation) = body
        .get_mut("generationConfig")
        .and_then(Value::as_object_mut)
    {
        generation.remove("temperature");
        generation.remove("topP");
        generation.remove("thinkingConfig");
        if let Some(temperature) = request.temperature {
            generation.insert("temperature".into(), json!(temperature));
        }
        if let Some(top_p) = request.top_p {
            generation.insert("topP".into(), json!(top_p));
        }
        if let Some(thinking) = &request.thinking {
            generation.insert(
                "thinkingConfig".into(),
                thinking_config(base_url, &request.model, thinking),
            );
        }
    }
    remove_object_keys(&mut body, &["tools", "toolConfig"]);
    if request.web_search {
        body["tools"] = json!([{"googleSearch": {}}]);
    }
    if request.logprobs {
        enable_gemini_logprobs(&mut body);
    } else {
        disable_gemini_logprobs(&mut body);
    }
    Ok(body)
}

fn thinking_config(base_url: &str, model_id: &str, thinking: &ThinkingConfig) -> Value {
    let enabled =
        thinking.mode != ThinkingMode::Disabled && thinking.effort != Some(ThinkingEffort::None);
    if is_feature_supported(FeatureId::GeminiThinkingLevel, base_url, model_id) {
        json!({
            "includeThoughts": false,
            "thinkingLevel": if enabled { gemini_thinking_level(thinking.effort) } else { "minimal" }
        })
    } else {
        json!({
            "includeThoughts": false,
            "thinkingBudget": if enabled {
                thinking.budget_tokens.map(Value::from).unwrap_or(json!(-1))
            } else {
                json!(0)
            }
        })
    }
}

fn gemini_thinking_level(effort: Option<ThinkingEffort>) -> &'static str {
    match effort.unwrap_or(ThinkingEffort::Medium) {
        ThinkingEffort::None | ThinkingEffort::Minimal => "minimal",
        ThinkingEffort::Low => "low",
        ThinkingEffort::Medium => "medium",
        ThinkingEffort::High | ThinkingEffort::Xhigh | ThinkingEffort::Max => "high",
    }
}

pub fn google_models(raw: &Value, normalize_name: bool, gemini_only: bool) -> Vec<RemoteModel> {
    let values = raw
        .get("publisherModels")
        .or_else(|| raw.get("models"))
        .and_then(Value::as_array);
    let mut models = values
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let request_name = item
                .get("id")
                .or_else(|| item.get("name"))
                .and_then(Value::as_str)?;
            let request_name = if normalize_name {
                crate::vertex_ai::model_id(request_name)
            } else {
                request_name.to_string()
            };
            if request_name.is_empty()
                || (gemini_only && !request_name.to_ascii_lowercase().starts_with("gemini"))
            {
                return None;
            }
            let alias = item
                .get("display_name")
                .or_else(|| item.get("displayName"))
                .and_then(Value::as_str)
                .unwrap_or(&request_name)
                .to_string();
            Some(RemoteModel {
                request_name,
                alias,
                added: false,
            })
        })
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.request_name.cmp(&right.request_name));
    models
}
