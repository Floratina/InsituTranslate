use serde_json::{json, Value};

use crate::domain::{
    ProviderRuntimeConfig, RemoteModel, ThinkingConfig, ThinkingEffort, ThinkingMode,
    UnifiedChatRequest, UnifiedChatResponse, UnifiedContent, UnifiedMessage, UnifiedUsage,
};
use crate::providers::budget::{
    normalize_completion_budget, CompletionBudgetAlias, ANTHROPIC_ALIASES,
};
use crate::providers::codec::{
    append_endpoint_suffix, EncodedRequest, EndpointPreview, HeaderDirective, HeaderMode,
    HttpMethod, ProtocolCodec,
};
use crate::providers::shared::{
    checked_usage_sum, merge_custom_parameters, normalize_usage, optional_usage_u64,
    push_encrypted_thinking, push_thinking_text, remove_object_keys, set_optional_field,
    unified_response, usage_object,
};

use super::anthropic_profile::{
    AnthropicDisablePolicy, AnthropicModelProfile, AnthropicSamplingPolicy,
    AnthropicThinkingDialect,
};

const DEFAULT_VISIBLE_OUTPUT_TOKENS: u32 = 4096;

pub struct AnthropicCodec;

pub static CODEC: AnthropicCodec = AnthropicCodec;

impl ProtocolCodec for AnthropicCodec {
    fn id(&self) -> &'static str {
        "anthropic"
    }

    fn completion_budget_aliases(&self) -> &'static [CompletionBudgetAlias] {
        ANTHROPIC_ALIASES
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

    fn validate_chat_options(
        &self,
        _base_url: &str,
        model_id: &str,
        thinking: Option<&ThinkingConfig>,
        temperature: Option<f64>,
        top_p: Option<f64>,
        custom_parameters: &Value,
    ) -> Result<(), String> {
        validate_request_options(model_id, thinking, temperature, top_p, custom_parameters)
    }

    fn plan_max_output_tokens(
        &self,
        _base_url: &str,
        model_id: &str,
        thinking: Option<&ThinkingConfig>,
        structured_max_output_tokens: Option<u32>,
        custom_parameters: &Value,
        visible_output_tokens: u32,
    ) -> Result<Option<u32>, String> {
        plan_request_max_tokens(
            model_id,
            thinking,
            structured_max_output_tokens,
            custom_parameters,
            visible_output_tokens,
        )
        .map(Some)
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
                Some("thinking") => {
                    let thinking_text = part
                        .get("thinking")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let signature = part
                        .get("signature")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    if thinking_text.is_empty() && signature.is_some() {
                        thinking.push(UnifiedContent::Thinking {
                            text: String::new(),
                            signature,
                            encrypted_data: None,
                        });
                    } else {
                        push_thinking_text(&mut reasoning, &mut thinking, thinking_text, signature);
                    }
                }
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
    let usage = usage_from_anthropic(raw.get("usage"))?;
    Ok(unified_response(
        raw,
        text,
        reasoning,
        thinking,
        usage,
        Vec::new(),
    ))
}

fn usage_from_anthropic(value: Option<&Value>) -> Result<Option<UnifiedUsage>, String> {
    let Some(value) = usage_object(value, "Anthropic")? else {
        return Ok(None);
    };
    let uncached_input = optional_usage_u64(value, "/input_tokens", "Anthropic")?;
    let cache_read = optional_usage_u64(value, "/cache_read_input_tokens", "Anthropic")?;
    let cache_creation = optional_usage_u64(value, "/cache_creation_input_tokens", "Anthropic")?;
    let input_tokens =
        if uncached_input.is_some() || cache_read.is_some() || cache_creation.is_some() {
            Some(checked_usage_sum(
                "Anthropic",
                "input",
                &[
                    uncached_input.unwrap_or(0),
                    cache_read.unwrap_or(0),
                    cache_creation.unwrap_or(0),
                ],
            )?)
        } else {
            None
        };
    let output_tokens = optional_usage_u64(value, "/output_tokens", "Anthropic")?;
    let thinking_tokens =
        optional_usage_u64(value, "/output_tokens_details/thinking_tokens", "Anthropic")?;
    normalize_usage(
        "Anthropic",
        input_tokens,
        output_tokens,
        cache_read,
        thinking_tokens,
        None,
        true,
    )
    .map(Some)
}

pub(crate) fn build_body(request: &UnifiedChatRequest) -> Result<Value, String> {
    let completion_budget = normalize_completion_budget(
        request.max_output_tokens,
        &request.custom_parameters,
        ANTHROPIC_ALIASES,
    )?;
    let explicit_max_tokens = completion_budget.resolved_max_output_tokens(None);
    let mut system = Vec::new();
    let mut messages = Vec::new();
    for message in &request.messages {
        if message.content.is_empty() {
            return Err(format!(
                "Anthropic {} messages must contain at least one content block.",
                message.role
            ));
        }
        if message.role == "system" {
            system.extend(anthropic_content(message));
        } else {
            messages.push(json!({
                "role": message.role,
                "content": anthropic_content(message)
            }));
        }
    }
    let messages = ensure_alternating_roles(messages)?;
    let mut body = json!({
        "model": request.model,
        "messages": messages
    });
    if !system.is_empty() {
        body["system"] = Value::Array(system);
    }
    if request.web_search {
        body["tools"] = json!([{"type": "web_search_20250305", "name": "web_search"}]);
    }

    let mut body = merge_custom_parameters(body, completion_budget.custom_parameters())?;
    remove_object_keys(
        &mut body,
        &[
            "temperature",
            "top_p",
            "thinking",
            "reasoning",
            "web_search_options",
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
    if request.web_search {
        body["tools"] = json!([{"type": "web_search_20250305", "name": "web_search"}]);
    }

    remove_output_config_effort(&mut body);
    let profile = AnthropicModelProfile::for_model(&request.model);
    let thinking = plan_thinking(request.thinking.as_ref(), profile, &request.model)?;
    let sampling = sampling_projection(
        &request.custom_parameters,
        request.temperature,
        request.top_p,
    )?;
    validate_sampling(&sampling, thinking.is_enabled(), profile, &request.model)?;
    let max_tokens = plan_max_tokens(explicit_max_tokens, thinking, profile, &request.model)?;
    body["max_tokens"] = Value::from(max_tokens);
    apply_thinking(&mut body, thinking);
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
        if !matches!(role, "user" | "assistant") {
            return Err(format!(
                "Anthropic messages only support user and assistant roles; received \"{role}\"."
            ));
        }
        if !message
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|content| !content.is_empty())
        {
            return Err(format!(
                "Anthropic {role} messages must contain at least one content block."
            ));
        }
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

#[derive(Debug, Clone, Copy)]
enum AnthropicThinkingPlan {
    Omitted,
    ImplicitAdaptive,
    Disabled,
    Manual {
        budget_tokens: u32,
        effort: Option<ThinkingEffort>,
    },
    Adaptive {
        effort: Option<ThinkingEffort>,
    },
}

impl AnthropicThinkingPlan {
    fn is_enabled(self) -> bool {
        matches!(
            self,
            Self::ImplicitAdaptive | Self::Manual { .. } | Self::Adaptive { .. }
        )
    }

    fn manual_budget(self) -> Option<u32> {
        match self {
            Self::Manual { budget_tokens, .. } => Some(budget_tokens),
            _ => None,
        }
    }
}

fn plan_thinking(
    thinking: Option<&ThinkingConfig>,
    profile: AnthropicModelProfile,
    model_id: &str,
) -> Result<AnthropicThinkingPlan, String> {
    let Some(thinking) = thinking else {
        if profile.default_thinking_effort.is_some() {
            return Ok(AnthropicThinkingPlan::ImplicitAdaptive);
        }
        return Ok(AnthropicThinkingPlan::Omitted);
    };
    if thinking.mode == ThinkingMode::Disabled || thinking.effort == Some(ThinkingEffort::None) {
        match profile.disable_policy {
            AnthropicDisablePolicy::AlwaysOn => {
                return Err(format!(
                    "Anthropic model \"{model_id}\" keeps thinking always on and rejects disabled thinking."
                ));
            }
            AnthropicDisablePolicy::OpusFive
                if matches!(
                    thinking.effort,
                    Some(ThinkingEffort::Xhigh | ThinkingEffort::Max)
                ) =>
            {
                return Err(format!(
                    "Anthropic model \"{model_id}\" cannot disable thinking with effort {:?}.",
                    thinking.effort.unwrap()
                ));
            }
            AnthropicDisablePolicy::Allowed | AnthropicDisablePolicy::OpusFive => {}
        }
        return Ok(AnthropicThinkingPlan::Disabled);
    }

    if profile.thinking_dialect == AnthropicThinkingDialect::Unknown {
        return Err(format!(
            "Anthropic thinking is enabled for unrecognized model \"{model_id}\". Select a recognized Claude model or disable thinking."
        ));
    }
    if let Some(effort) = thinking.effort {
        if !profile.supports_effort(effort) {
            return Err(format!(
                "Anthropic model \"{model_id}\" does not support thinking effort {effort:?}."
            ));
        }
    }

    match profile.thinking_dialect {
        AnthropicThinkingDialect::Manual | AnthropicThinkingDialect::ManualWithEffort => {
            if thinking.mode != ThinkingMode::Enabled {
                return Err(format!(
                    "Anthropic model \"{model_id}\" uses manual thinking and requires ThinkingMode::Enabled."
                ));
            }
            let budget_tokens = thinking.budget_tokens.ok_or_else(|| {
                format!(
                    "Anthropic manual thinking for model \"{model_id}\" is missing its centrally resolved fixed budget."
                )
            })?;
            if budget_tokens < 1024 {
                return Err(format!(
                    "Anthropic thinking budget for model \"{model_id}\" must be at least 1024 tokens; received {budget_tokens}."
                ));
            }
            Ok(AnthropicThinkingPlan::Manual {
                budget_tokens,
                effort: (profile.thinking_dialect == AnthropicThinkingDialect::ManualWithEffort)
                    .then_some(thinking.effort)
                    .flatten(),
            })
        }
        AnthropicThinkingDialect::Adaptive => {
            if thinking.mode != ThinkingMode::Auto {
                return Err(format!(
                    "Anthropic model \"{model_id}\" uses adaptive thinking and requires ThinkingMode::Auto."
                ));
            }
            if let Some(budget_tokens) = thinking.budget_tokens {
                return Err(format!(
                    "Anthropic model \"{model_id}\" uses adaptive thinking and does not accept budget_tokens ({budget_tokens})."
                ));
            }
            Ok(AnthropicThinkingPlan::Adaptive {
                effort: thinking.effort,
            })
        }
        AnthropicThinkingDialect::Unknown => unreachable!("unknown models return above"),
    }
}

fn plan_max_tokens(
    explicit_max_tokens: Option<u32>,
    thinking: AnthropicThinkingPlan,
    profile: AnthropicModelProfile,
    model_id: &str,
) -> Result<u32, String> {
    plan_max_tokens_from_parts(
        explicit_max_tokens,
        None,
        thinking,
        profile,
        model_id,
        DEFAULT_VISIBLE_OUTPUT_TOKENS,
    )
}

fn plan_request_max_tokens(
    model_id: &str,
    thinking: Option<&ThinkingConfig>,
    structured_max_tokens: Option<u32>,
    custom_parameters: &Value,
    visible_output_tokens: u32,
) -> Result<u32, String> {
    let profile = AnthropicModelProfile::for_model(model_id);
    let thinking = plan_thinking(thinking, profile, model_id)?;
    let completion_budget =
        normalize_completion_budget(structured_max_tokens, custom_parameters, ANTHROPIC_ALIASES)?;
    plan_max_tokens_from_parts(
        completion_budget.resolved_max_output_tokens(None),
        None,
        thinking,
        profile,
        model_id,
        visible_output_tokens.clamp(DEFAULT_VISIBLE_OUTPUT_TOKENS, 16_000),
    )
}

fn plan_max_tokens_from_parts(
    structured_max_tokens: Option<u32>,
    custom_max_tokens: Option<u32>,
    thinking: AnthropicThinkingPlan,
    profile: AnthropicModelProfile,
    model_id: &str,
    visible_output_tokens: u32,
) -> Result<u32, String> {
    if thinking.is_enabled()
        && profile.max_output_tokens.is_none()
        && structured_max_tokens.is_none()
        && custom_max_tokens.is_none()
    {
        return Err(format!(
            "Anthropic model \"{model_id}\" has no current verified output limit; set an explicit max_tokens value before enabling thinking."
        ));
    }
    let required = required_max_tokens(thinking, visible_output_tokens, model_id)?;
    let uses_unknown_automatic_default = structured_max_tokens.is_none()
        && custom_max_tokens.is_none()
        && profile.thinking_dialect == AnthropicThinkingDialect::Unknown;
    let max_tokens = if let Some(max_tokens) = structured_max_tokens {
        max_tokens
    } else if let Some(max_tokens) = custom_max_tokens {
        max_tokens
    } else if uses_unknown_automatic_default {
        DEFAULT_VISIBLE_OUTPUT_TOKENS
    } else {
        required
    };

    validate_max_tokens(
        max_tokens,
        thinking,
        profile,
        model_id,
        (!uses_unknown_automatic_default).then_some(required),
    )?;
    Ok(max_tokens)
}

fn required_max_tokens(
    thinking: AnthropicThinkingPlan,
    visible_output_tokens: u32,
    model_id: &str,
) -> Result<u32, String> {
    let add = |tokens: u32| {
        visible_output_tokens
            .checked_add(tokens)
            .ok_or_else(|| format!("Anthropic output budget overflows for model \"{model_id}\"."))
    };
    match thinking {
        AnthropicThinkingPlan::Manual { budget_tokens, .. } => add(budget_tokens),
        AnthropicThinkingPlan::ImplicitAdaptive | AnthropicThinkingPlan::Adaptive { .. } => {
            add(visible_output_tokens)
        }
        AnthropicThinkingPlan::Omitted | AnthropicThinkingPlan::Disabled => {
            Ok(visible_output_tokens)
        }
    }
}

fn validate_max_tokens(
    max_tokens: u32,
    thinking: AnthropicThinkingPlan,
    profile: AnthropicModelProfile,
    model_id: &str,
    minimum: Option<u32>,
) -> Result<(), String> {
    if max_tokens == 0 {
        return Err(format!(
            "Anthropic max_tokens for model \"{model_id}\" must be greater than zero."
        ));
    }
    if let Some(limit) = profile.max_output_tokens {
        if max_tokens > limit {
            return Err(format!(
                "Anthropic max_tokens for model \"{model_id}\" is {max_tokens}, which exceeds the known {limit}-token output limit."
            ));
        }
    }
    if let Some(budget_tokens) = thinking.manual_budget() {
        if budget_tokens >= max_tokens {
            return Err(format!(
                "Anthropic thinking budget ({budget_tokens}) must be smaller than max_tokens ({max_tokens}) for model \"{model_id}\"."
            ));
        }
    }
    if let Some(minimum) = minimum {
        if max_tokens < minimum {
            return Err(format!(
                "Anthropic max_tokens for model \"{model_id}\" is {max_tokens}, but the selected output and thinking settings require at least {minimum}."
            ));
        }
    }
    Ok(())
}

fn validate_request_options(
    model_id: &str,
    thinking: Option<&ThinkingConfig>,
    temperature: Option<f64>,
    top_p: Option<f64>,
    custom_parameters: &Value,
) -> Result<(), String> {
    let profile = AnthropicModelProfile::for_model(model_id);
    let thinking = plan_thinking(thinking, profile, model_id)?;
    let sampling = sampling_projection(custom_parameters, temperature, top_p)?;
    validate_sampling(&sampling, thinking.is_enabled(), profile, model_id)
}

fn sampling_projection(
    custom_parameters: &Value,
    temperature: Option<f64>,
    top_p: Option<f64>,
) -> Result<Value, String> {
    let custom = match custom_parameters {
        Value::Null => None,
        Value::Object(parameters) => Some(parameters),
        _ => return Err("Custom request body parameters must be a JSON object".into()),
    };
    let mut sampling = json!({});
    if let Some(top_k) = custom.and_then(|parameters| parameters.get("top_k")) {
        sampling["top_k"] = top_k.clone();
    }
    set_optional_field(&mut sampling, "temperature", temperature.map(Value::from));
    set_optional_field(&mut sampling, "top_p", top_p.map(Value::from));
    Ok(sampling)
}

fn apply_thinking(body: &mut Value, thinking: AnthropicThinkingPlan) {
    match thinking {
        AnthropicThinkingPlan::Omitted | AnthropicThinkingPlan::ImplicitAdaptive => {}
        AnthropicThinkingPlan::Disabled => {
            body["thinking"] = json!({"type": "disabled"});
        }
        AnthropicThinkingPlan::Manual {
            budget_tokens,
            effort,
        } => {
            body["thinking"] = json!({
                "type": "enabled",
                "budget_tokens": budget_tokens
            });
            set_output_config_effort(body, effort);
        }
        AnthropicThinkingPlan::Adaptive { effort } => {
            body["thinking"] = json!({"type": "adaptive"});
            set_output_config_effort(body, effort);
        }
    }
}

fn set_output_config_effort(body: &mut Value, effort: Option<ThinkingEffort>) {
    let Some(effort) = effort else {
        return;
    };
    if !body.get("output_config").is_some_and(Value::is_object) {
        body["output_config"] = json!({});
    }
    body["output_config"]["effort"] = Value::String(effort_wire_name(effort).into());
}

fn remove_output_config_effort(body: &mut Value) {
    let remove_output_config = body
        .get_mut("output_config")
        .and_then(Value::as_object_mut)
        .is_some_and(|output_config| {
            output_config.remove("effort");
            output_config.is_empty()
        });
    if remove_output_config {
        body.as_object_mut()
            .expect("Anthropic request body is an object")
            .remove("output_config");
    }
}

fn effort_wire_name(effort: ThinkingEffort) -> &'static str {
    match effort {
        ThinkingEffort::Low => "low",
        ThinkingEffort::Medium => "medium",
        ThinkingEffort::High => "high",
        ThinkingEffort::Xhigh => "xhigh",
        ThinkingEffort::Max => "max",
        ThinkingEffort::None | ThinkingEffort::Minimal => {
            unreachable!("disabled and unsupported efforts are handled before serialization")
        }
    }
}

fn validate_unit_interval(name: &str, value: Option<f64>) -> Result<(), String> {
    if let Some(value) = value {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(format!(
                "Anthropic {name} must be a finite number from 0 through 1; received {value}."
            ));
        }
    }
    Ok(())
}

fn validate_sampling(
    body: &Value,
    thinking_enabled: bool,
    profile: AnthropicModelProfile,
    model_id: &str,
) -> Result<(), String> {
    let temperature = optional_number(body, "temperature")?;
    let top_p = optional_number(body, "top_p")?;
    let top_k = match body.get("top_k") {
        Some(value) => Some(value.as_u64().ok_or_else(|| {
            format!("Anthropic top_k for model \"{model_id}\" must be a non-negative integer.")
        })?),
        None => None,
    };
    validate_unit_interval("temperature", temperature)?;
    validate_unit_interval("top_p", top_p)?;

    if profile.sampling_policy == AnthropicSamplingPolicy::Unknown
        && (temperature.is_some() || top_p.is_some() || top_k.is_some())
    {
        return Err(format!(
            "Anthropic sampling compatibility cannot be verified for unrecognized model \"{model_id}\"; omit temperature, top_p, and top_k."
        ));
    }

    if profile.sampling_policy == AnthropicSamplingPolicy::NoNonDefault {
        if temperature.is_some_and(|value| value != 1.0)
            || top_p.is_some_and(|value| value != 1.0)
            || top_k.is_some()
        {
            return Err(format!(
                "Anthropic model \"{model_id}\" does not accept non-default temperature, top_p, or top_k values; omit those sampling parameters."
            ));
        }
        return Ok(());
    }

    if profile.sampling_policy == AnthropicSamplingPolicy::Haiku45
        && temperature.is_some()
        && top_p.is_some()
    {
        return Err(format!(
            "Anthropic model \"{model_id}\" cannot use temperature and top_p together."
        ));
    }

    if thinking_enabled {
        if temperature.is_some() {
            return Err(format!(
                "Anthropic thinking for model \"{model_id}\" is incompatible with temperature; omit temperature."
            ));
        }
        if top_k.is_some() {
            return Err(format!(
                "Anthropic thinking for model \"{model_id}\" is incompatible with top_k; omit top_k."
            ));
        }
        if top_p.is_some_and(|value| !(0.95..=1.0).contains(&value)) {
            return Err(format!(
                "Anthropic thinking for model \"{model_id}\" requires top_p from 0.95 through 1."
            ));
        }
    }
    Ok(())
}

fn optional_number(body: &Value, key: &str) -> Result<Option<f64>, String> {
    body.get(key)
        .map(|value| {
            value.as_f64().ok_or_else(|| {
                format!("Anthropic {key} must be a JSON number when it is provided.")
            })
        })
        .transpose()
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[test]
    fn anthropic_usage_reconstructs_cached_input_and_visible_output() {
        let response = decode_chat(json!({
            "content": [{"type": "text", "text": "done"}],
            "usage": {
                "input_tokens": 50,
                "cache_read_input_tokens": 100_000,
                "cache_creation_input_tokens": 200,
                "output_tokens": 503,
                "output_tokens_details": {"thinking_tokens": 203}
            }
        }))
        .expect("valid response");
        let usage = response.usage.expect("usage");

        assert_eq!(usage.input_tokens, 100_250);
        assert_eq!(usage.cached_tokens, 100_000);
        assert_eq!(usage.output_tokens, 300);
        assert_eq!(usage.thinking_tokens, 203);
        assert_eq!(usage.total_tokens, 100_753);
    }

    #[test]
    fn anthropic_usage_rejects_reasoning_larger_than_output() {
        let error = decode_chat(json!({
            "content": [],
            "usage": {
                "input_tokens": 1,
                "output_tokens": 2,
                "output_tokens_details": {"thinking_tokens": 3}
            }
        }))
        .expect_err("invalid reasoning count must fail");

        assert!(error.contains("thinking token count 3 exceeds output token count 2"));
    }
}
