use serde_json::{json, Value};

use crate::domain::{
    ProviderRuntimeConfig, RemoteModel, ThinkingConfig, ThinkingEffort, ThinkingMode,
    UnifiedChatRequest, UnifiedChatResponse, UnifiedContent, UnifiedMessage, UnifiedUsage,
};
use crate::providers::capabilities::ModelCapabilities;
use crate::providers::codec::{
    append_endpoint_suffix, EncodedRequest, EndpointPreview, HeaderDirective, HeaderMode,
    HttpMethod, JsonEventStreamDecoder, ProtocolCodec, ProtocolStreamDecoder,
};
use crate::providers::shared::{
    merge_custom_parameters, push_encrypted_thinking, push_thinking_text, remove_object_keys,
    set_optional_field, unified_response,
};
use crate::providers::thinking;

pub struct AnthropicCodec;

pub static CODEC: AnthropicCodec = AnthropicCodec;

impl ProtocolCodec for AnthropicCodec {
    fn id(&self) -> &'static str {
        "anthropic"
    }

    fn encode_model_list(&self, config: &ProviderRuntimeConfig) -> Result<EncodedRequest, String> {
        let suffix = if config.use_raw_base_url {
            "models"
        } else {
            "v1/models"
        };
        Ok(EncodedRequest {
            method: HttpMethod::Get,
            url: append_endpoint_suffix(&config.base_url, suffix),
            headers: protocol_headers(),
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
        let suffix = if config.use_raw_base_url {
            "messages"
        } else {
            "v1/messages"
        };
        Ok(EncodedRequest {
            method: HttpMethod::Post,
            url: append_endpoint_suffix(&config.base_url, suffix),
            headers: protocol_headers(),
            body: Some(build_body(request)?),
        })
    }

    fn decode_chat(&self, raw: Value) -> Result<UnifiedChatResponse, String> {
        decode_chat(raw)
    }

    fn finish_reason(&self, raw: &Value) -> Option<String> {
        raw.get("stop_reason")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn new_stream_decoder(&self) -> Box<dyn ProtocolStreamDecoder> {
        Box::new(JsonEventStreamDecoder::new(decode_chat))
    }

    fn infer_capabilities(&self, base_url: &str, model_id: &str) -> ModelCapabilities {
        let inferred = crate::features::anthropic_capabilities(base_url, model_id);
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
        _base_url: &str,
        _model_id: &str,
        reasoning: bool,
    ) -> Vec<ThinkingEffort> {
        crate::features::budget_thinking_efforts(reasoning)
    }

    fn resolve_thinking(
        &self,
        _base_url: &str,
        _model_id: &str,
        effort: ThinkingEffort,
    ) -> ThinkingConfig {
        let mut config = thinking::base_config(effort);
        config.budget_tokens = Some(thinking::budget_tokens(effort));
        config
    }

    fn preview_endpoints(&self, config: &ProviderRuntimeConfig) -> Result<EndpointPreview, String> {
        let chat_suffix = if config.use_raw_base_url {
            "messages"
        } else {
            "v1/messages"
        };
        let model_suffix = if config.use_raw_base_url {
            "models"
        } else {
            "v1/models"
        };
        Ok(EndpointPreview {
            chat: append_endpoint_suffix(&config.base_url, chat_suffix),
            models: Some(append_endpoint_suffix(&config.base_url, model_suffix)),
        })
    }
}

fn protocol_headers() -> Vec<HeaderDirective> {
    vec![HeaderDirective {
        name: "anthropic-version".into(),
        value: "2023-06-01".into(),
        mode: HeaderMode::IfAbsent,
    }]
}

fn decode_chat(raw: Value) -> Result<UnifiedChatResponse, String> {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut thinking = Vec::new();
    if let Some(content) = raw.get("content").and_then(Value::as_array) {
        for part in content {
            match part.get("type").and_then(Value::as_str) {
                Some("text") => {
                    text.push_str(part.get("text").and_then(Value::as_str).unwrap_or_default())
                }
                Some("thinking") => push_thinking_text(
                    &mut reasoning,
                    &mut thinking,
                    part.get("thinking")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    part.get("signature")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                ),
                Some("redacted_thinking") => {
                    if let Some(data) = part.get("data").and_then(Value::as_str) {
                        push_encrypted_thinking(&mut thinking, data);
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(delta) = raw.get("delta") {
        text.push_str(
            delta
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        push_thinking_text(
            &mut reasoning,
            &mut thinking,
            delta
                .get("thinking")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            None,
        );
    }
    let usage = raw.get("usage").map(|value| UnifiedUsage {
        input_tokens: value
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: value
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cached_tokens: value
            .get("cache_read_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    });
    Ok(unified_response(
        raw,
        text,
        reasoning,
        thinking,
        usage,
        Vec::new(),
    ))
}

pub(crate) fn build_body(request: &UnifiedChatRequest) -> Result<Value, String> {
    let mut system = Vec::new();
    let mut messages = Vec::new();
    for message in &request.messages {
        if message.role == "system" {
            system.extend(anthropic_content(message));
        } else {
            messages.push(json!({
                "role": message.role,
                "content": anthropic_content(message)
            }));
        }
    }
    let messages = ensure_alternating_roles(messages).unwrap_or_default();
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "max_tokens": request.max_output_tokens.unwrap_or(4096),
        "stream": request.stream
    });
    if !system.is_empty() {
        body["system"] = Value::Array(system);
    }
    set_optional_field(
        &mut body,
        "temperature",
        request.temperature.map(Value::from),
    );
    set_optional_field(&mut body, "top_p", request.top_p.map(Value::from));
    if let Some(thinking) = &request.thinking {
        body["thinking"] = thinking_value(thinking, request.max_output_tokens);
    }
    if request.web_search {
        body["tools"] = json!([{"type": "web_search_20250305", "name": "web_search"}]);
    }

    let mut body = merge_custom_parameters(body, &request.custom_parameters)?;
    remove_object_keys(
        &mut body,
        &[
            "temperature",
            "top_p",
            "thinking",
            "tools",
            "logprobs",
            "top_logprobs",
        ],
    );
    set_optional_field(
        &mut body,
        "temperature",
        request.temperature.map(Value::from),
    );
    set_optional_field(&mut body, "top_p", request.top_p.map(Value::from));
    if let Some(thinking) = &request.thinking {
        body["thinking"] = thinking_value(thinking, request.max_output_tokens);
    }
    if request.web_search {
        body["tools"] = json!([{"type": "web_search_20250305", "name": "web_search"}]);
    }
    Ok(body)
}

fn anthropic_content(message: &UnifiedMessage) -> Vec<Value> {
    message
        .content
        .iter()
        .map(|part| match part {
            UnifiedContent::Text { text } => json!({"type": "text", "text": text}),
            UnifiedContent::CacheableText { text } => json!({
                "type": "text",
                "text": text,
                "cache_control": {"type": "ephemeral"}
            }),
            UnifiedContent::Image { media_type, data } => json!({
                "type": "image",
                "source": {"type": "base64", "media_type": media_type, "data": data}
            }),
            UnifiedContent::Thinking {
                text,
                signature,
                encrypted_data,
            } => match encrypted_data {
                Some(data) => json!({"type": "redacted_thinking", "data": data}),
                None => json!({
                    "type": "thinking",
                    "thinking": text,
                    "signature": signature.clone().unwrap_or_default()
                }),
            },
        })
        .collect()
}

pub fn ensure_alternating_roles(messages: Vec<Value>) -> Result<Vec<Value>, String> {
    let mut output: Vec<Value> = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if output
            .last()
            .and_then(|item| item.get("role"))
            .and_then(Value::as_str)
            == Some(role)
        {
            let incoming = message
                .get("content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(parts) = output
                .last_mut()
                .and_then(|item| item.get_mut("content"))
                .and_then(Value::as_array_mut)
            {
                parts.extend(incoming);
            }
        } else {
            output.push(message);
        }
    }
    if output
        .first()
        .and_then(|item| item.get("role"))
        .and_then(Value::as_str)
        != Some("user")
    {
        return Err("The first Anthropic message must have the user role".into());
    }
    Ok(output)
}

fn thinking_value(thinking: &ThinkingConfig, max_output_tokens: Option<u32>) -> Value {
    let enabled =
        thinking.mode != ThinkingMode::Disabled && thinking.effort != Some(ThinkingEffort::None);
    if enabled {
        let max = max_output_tokens.unwrap_or(4096);
        let budget = thinking
            .budget_tokens
            .unwrap_or(1024)
            .max(1024)
            .min(max.saturating_sub(1));
        json!({"type": "enabled", "budget_tokens": budget})
    } else {
        json!({"type": "disabled"})
    }
}
