use serde_json::{json, Value};

use crate::domain::{
    ProviderRuntimeConfig, RemoteModel, ThinkingConfig, ThinkingEffort, ThinkingMode,
    ThinkingSummary, UnifiedChatRequest, UnifiedChatResponse, UnifiedContent, UnifiedMessage,
};
use crate::providers::budget::{
    normalize_completion_budget, CompletionBudgetAlias, OPENAI_RESPONSES_ALIASES,
};
use crate::providers::codec::{
    openai_endpoint, EncodedRequest, EndpointPreview, HttpMethod, ProtocolCodec,
};
use crate::providers::shared::{
    append_responses_output_item, disable_openai_response_logprobs,
    enable_openai_response_logprobs, filtered_token_logprobs, merge_custom_parameters,
    push_thinking_text, remove_object_keys, set_optional_field, unified_response,
    usage_from_openai,
};

pub struct OpenAiResponsesCodec;

pub static CODEC: OpenAiResponsesCodec = OpenAiResponsesCodec;

pub fn openai_models(raw: &Value) -> Vec<RemoteModel> {
    let mut models = raw
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let request_name = item
                .get("id")
                .or_else(|| item.get("name"))
                .and_then(Value::as_str)?
                .to_string();
            if request_name.is_empty() {
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

impl ProtocolCodec for OpenAiResponsesCodec {
    fn id(&self) -> &'static str {
        "openai-responses"
    }

    fn completion_budget_aliases(&self) -> &'static [CompletionBudgetAlias] {
        OPENAI_RESPONSES_ALIASES
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
        Ok(openai_models(raw))
    }

    fn encode_chat(
        &self,
        config: &ProviderRuntimeConfig,
        request: &UnifiedChatRequest,
    ) -> Result<EncodedRequest, String> {
        Ok(EncodedRequest {
            method: HttpMethod::Post,
            url: openai_endpoint(config, "responses"),
            headers: Vec::new(),
            body: Some(build_body(&config.base_url, request)?),
        })
    }

    fn decode_chat(&self, raw: Value) -> Result<UnifiedChatResponse, String> {
        decode_chat(raw)
    }

    fn finish_reason(&self, raw: &Value) -> Option<String> {
        raw.pointer("/incomplete_details/reason")
            .or_else(|| raw.pointer("/response/incomplete_details/reason"))
            .or_else(|| raw.get("status"))
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn preview_endpoints(&self, config: &ProviderRuntimeConfig) -> Result<EndpointPreview, String> {
        Ok(EndpointPreview {
            chat: openai_endpoint(config, "responses"),
            models: Some(openai_endpoint(config, "models")),
        })
    }
}

fn decode_chat(raw: Value) -> Result<UnifiedChatResponse, String> {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut thinking = Vec::new();
    if let Some(event_type) = raw.get("type").and_then(Value::as_str) {
        match event_type {
            "response.output_text.delta" | "response.refusal.delta" => {
                text.push_str(raw.get("delta").and_then(Value::as_str).unwrap_or_default());
            }
            "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
                push_thinking_text(
                    &mut reasoning,
                    &mut thinking,
                    raw.get("delta").and_then(Value::as_str).unwrap_or_default(),
                    None,
                );
            }
            "response.output_item.done" => {
                if let Some(item) = raw.get("item") {
                    append_responses_output_item(item, &mut text, &mut reasoning, &mut thinking);
                }
            }
            "response.completed" => {
                if let Some(output) = raw
                    .get("response")
                    .and_then(|response| response.get("output"))
                    .and_then(Value::as_array)
                {
                    for item in output {
                        append_responses_output_item(
                            item,
                            &mut text,
                            &mut reasoning,
                            &mut thinking,
                        );
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(output) = raw.get("output").and_then(Value::as_array) {
        for item in output {
            append_responses_output_item(item, &mut text, &mut reasoning, &mut thinking);
        }
    }
    text.push_str(raw.get("delta").and_then(Value::as_str).unwrap_or_default());
    let usage_value = raw
        .get("response")
        .and_then(|response| response.get("usage"))
        .or_else(|| raw.get("usage"));
    let usage = usage_from_openai(usage_value)?;
    let mut logprobs = Vec::new();
    collect_logprob_arrays(raw.pointer("/output"), &mut logprobs);
    if logprobs.is_empty() {
        collect_logprob_arrays(raw.pointer("/response/output"), &mut logprobs);
    }
    Ok(unified_response(
        raw, text, reasoning, thinking, usage, logprobs,
    ))
}

pub(crate) fn build_body(base_url: &str, request: &UnifiedChatRequest) -> Result<Value, String> {
    let completion_budget = normalize_completion_budget(
        request.max_output_tokens,
        &request.custom_parameters,
        OPENAI_RESPONSES_ALIASES,
    )?;
    let max_output_tokens = completion_budget.resolved_max_output_tokens(None);
    let mut body = json!({
        "model": request.model,
        "input": responses_input(&request.messages)
    });
    set_optional_field(
        &mut body,
        "temperature",
        request.temperature.map(Value::from),
    );
    set_optional_field(&mut body, "top_p", request.top_p.map(Value::from));
    if let Some(thinking) = &request.thinking {
        apply_thinking(&mut body, base_url, &request.model, thinking);
    }
    if request.web_search {
        body["tools"] = json!([{"type": "web_search"}]);
    }
    if request.logprobs {
        enable_openai_response_logprobs(&mut body);
    }

    let mut body = merge_custom_parameters(body, completion_budget.custom_parameters())?;
    remove_object_keys(
        &mut body,
        &[
            "temperature",
            "top_p",
            "reasoning",
            "thinking",
            "tools",
            "web_search_options",
        ],
    );
    set_optional_field(
        &mut body,
        "temperature",
        request.temperature.map(Value::from),
    );
    set_optional_field(&mut body, "top_p", request.top_p.map(Value::from));
    if let Some(thinking) = &request.thinking {
        apply_thinking(&mut body, base_url, &request.model, thinking);
    }
    if request.web_search {
        body["tools"] = json!([{"type": "web_search"}]);
    }
    if request.logprobs {
        enable_openai_response_logprobs(&mut body);
    } else {
        disable_openai_response_logprobs(&mut body);
    }
    remove_object_keys(&mut body, &["max_output_tokens"]);
    if let Some(tokens) = max_output_tokens {
        body["max_output_tokens"] = json!(tokens);
    }
    Ok(body)
}

fn apply_thinking(body: &mut Value, base_url: &str, model: &str, thinking: &ThinkingConfig) {
    let disabled =
        thinking.mode == ThinkingMode::Disabled || thinking.effort == Some(ThinkingEffort::None);
    body["reasoning"] = json!({
        "effort": if disabled { "none" } else { thinking.effort.map(effort_name).unwrap_or("medium") },
        "summary": thinking.summary.map(|summary| match summary {
            ThinkingSummary::None => "none",
            ThinkingSummary::Auto => "auto",
            ThinkingSummary::Concise => "concise",
            ThinkingSummary::Detailed => "detailed",
        }).unwrap_or("auto")
    });
    if crate::features::is_feature_supported(
        crate::features::FeatureId::OpenAiThinkingObject,
        base_url,
        model,
    ) {
        body["thinking"] = json!({"type": if disabled { "disabled" } else { "enabled" }});
    }
}

fn responses_input(messages: &[UnifiedMessage]) -> Vec<Value> {
    let mut output = Vec::new();
    for (message_index, message) in messages.iter().enumerate() {
        let role = match message.role.as_str() {
            "assistant" => "assistant",
            "system" => "system",
            _ => "user",
        };
        let mut content = Vec::new();
        for (part_index, part) in message.content.iter().enumerate() {
            match part {
                UnifiedContent::Text { text } | UnifiedContent::CacheableText { text } => {
                    if !text.trim().is_empty() {
                        content.push(json!({
                            "type": if role == "assistant" { "output_text" } else { "input_text" },
                            "text": text
                        }));
                    }
                }
                UnifiedContent::Image { media_type, data } => content.push(json!({
                    "type": "input_image",
                    "image_url": format!("data:{media_type};base64,{data}")
                })),
                UnifiedContent::Thinking {
                    text,
                    signature,
                    encrypted_data,
                } => {
                    push_message(&mut output, role, &mut content);
                    let id = signature
                        .as_deref()
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("reasoning_{message_index}_{part_index}"));
                    match encrypted_data {
                        Some(data) => output.push(json!({
                            "type": "reasoning",
                            "id": id,
                            "summary": [],
                            "encrypted_content": data
                        })),
                        None if !text.is_empty() => output.push(json!({
                            "type": "reasoning",
                            "id": id,
                            "summary": [],
                            "content": [{"type": "reasoning_text", "text": text}]
                        })),
                        None => {}
                    }
                }
            }
        }
        push_message(&mut output, role, &mut content);
    }
    output
}

fn push_message(output: &mut Vec<Value>, role: &str, content: &mut Vec<Value>) {
    if !content.is_empty() {
        output.push(json!({"role": role, "content": std::mem::take(content)}));
    }
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

fn collect_logprob_arrays(value: Option<&Value>, output: &mut Vec<f64>) {
    let Some(value) = value else {
        return;
    };
    match value {
        Value::Array(items) => {
            for item in items {
                collect_logprob_arrays(Some(item), output);
            }
        }
        Value::Object(object) => {
            if let Some(items) = object.get("logprobs").and_then(Value::as_array) {
                output.extend(filtered_token_logprobs(items, "logprob"));
            }
            for key in ["content", "output"] {
                if let Some(child) = object.get(key) {
                    collect_logprob_arrays(Some(child), output);
                }
            }
        }
        _ => {}
    }
}
