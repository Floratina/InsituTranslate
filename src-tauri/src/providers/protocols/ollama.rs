use serde_json::{json, Value};

use crate::domain::{
    ProviderRuntimeConfig, RemoteModel, ThinkingConfig, ThinkingEffort, UnifiedChatRequest,
    UnifiedChatResponse, UnifiedContent, UnifiedUsage,
};
use crate::providers::capabilities::ModelCapabilities;
use crate::providers::codec::{
    append_endpoint_suffix, endpoint_base_url, EncodedRequest, EndpointPreview, HttpMethod,
    JsonEventStreamDecoder, ProtocolCodec, ProtocolStreamDecoder,
};
use crate::providers::shared::{content_text, merge_custom_parameters, remove_object_keys};
use crate::providers::thinking;

pub struct OllamaCodec;

pub static CODEC: OllamaCodec = OllamaCodec;

impl ProtocolCodec for OllamaCodec {
    fn id(&self) -> &'static str {
        "ollama"
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

    fn new_stream_decoder(&self) -> Box<dyn ProtocolStreamDecoder> {
        Box::new(JsonEventStreamDecoder::new(decode_chat))
    }

    fn infer_capabilities(&self, _base_url: &str, model_id: &str) -> ModelCapabilities {
        let inferred = crate::features::ollama_capabilities(model_id);
        ModelCapabilities {
            reasoning: inferred.reasoning,
            web: inferred.web,
            thinking_efforts: self.supported_thinking_efforts("", model_id, inferred.reasoning),
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
        config.effort = Some(thinking::ollama_effort(effort));
        config
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
    let usage = Some(UnifiedUsage {
        input_tokens: raw
            .get("prompt_eval_count")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: raw.get("eval_count").and_then(Value::as_u64).unwrap_or(0),
        cached_tokens: 0,
    });
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
        "stream": request.stream
    });
    if let Some(thinking) = &request.thinking {
        body["think"] = think_value(thinking);
    }
    let mut options = json!({});
    if let Some(tokens) = request.max_output_tokens {
        options["num_predict"] = json!(tokens);
    }
    if let Some(temperature) = request.temperature {
        options["temperature"] = json!(temperature);
    }
    if let Some(top_p) = request.top_p {
        options["top_p"] = json!(top_p);
    }
    if options.as_object().is_some_and(|value| !value.is_empty()) {
        body["options"] = options;
    }

    let mut body = merge_custom_parameters(body, &request.custom_parameters)?;
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
