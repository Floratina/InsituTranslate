use serde_json::Value;

use crate::domain::{ModelView, ProviderRuntimeConfig, ThinkingConfig, ThinkingEffort};
use crate::providers::capabilities::{infer_capabilities, resolve_thinking};
use crate::providers::descriptor_for;
#[cfg(test)]
use crate::providers::CompletionBudget;

use super::TranslationConfigView;

#[derive(Debug, Clone)]
pub(super) struct ModelRequestSettings {
    pub thinking_effort: ThinkingEffort,
    pub use_web_search: bool,
    pub use_custom_parameters: bool,
}

#[derive(Debug, Clone)]
pub(super) struct ModelRequestOptions {
    pub custom_parameters: Value,
    pub web_search: bool,
    pub thinking: Option<ThinkingConfig>,
}

pub(super) type TranslationRequestOptions = ModelRequestOptions;

pub(super) fn visible_output_token_reserve(source_tokens: u64) -> u32 {
    source_tokens.saturating_mul(2).clamp(4_096, 16_000) as u32
}

#[cfg(test)]
pub(super) fn validate_and_plan_model_request(
    runtime: &ProviderRuntimeConfig,
    model: &ModelView,
    options: &ModelRequestOptions,
    temperature: Option<f64>,
    top_p: Option<f64>,
    visible_output_tokens: u32,
) -> Result<CompletionBudget, String> {
    let descriptor = descriptor_for(&runtime.protocol)?;
    descriptor.codec.validate_chat_options(
        &runtime.base_url,
        &model.request_name,
        options.thinking.as_ref(),
        temperature,
        top_p,
        &options.custom_parameters,
    )?;
    descriptor.codec.plan_completion_budget(
        &runtime.base_url,
        &model.request_name,
        options.thinking.as_ref(),
        None,
        &options.custom_parameters,
        visible_output_tokens,
    )
}

pub(super) fn resolve_model_request_options(
    settings: &ModelRequestSettings,
    runtime: &ProviderRuntimeConfig,
    model: &ModelView,
    custom_parameters: Value,
) -> Result<ModelRequestOptions, String> {
    let custom_parameters = if settings.use_custom_parameters {
        validate_custom_parameters(custom_parameters)?
    } else {
        Value::Object(serde_json::Map::new())
    };
    let thinking = resolve_model_thinking(settings.thinking_effort, runtime, model)?;
    let web_search = resolve_model_web_search(settings.use_web_search, runtime, model)?;
    Ok(ModelRequestOptions {
        custom_parameters,
        web_search,
        thinking,
    })
}

pub(super) fn resolve_translation_request_options(
    config: &TranslationConfigView,
    runtime: &ProviderRuntimeConfig,
    model: &ModelView,
    custom_parameters: Value,
) -> Result<TranslationRequestOptions, String> {
    resolve_model_request_options(
        &ModelRequestSettings {
            thinking_effort: config.thinking_effort,
            use_web_search: config.use_web_search,
            use_custom_parameters: config.use_custom_parameters,
        },
        runtime,
        model,
        custom_parameters,
    )
}

fn validate_custom_parameters(custom_parameters: Value) -> Result<Value, String> {
    match custom_parameters {
        Value::Null => Ok(Value::Object(serde_json::Map::new())),
        Value::Object(object) => Ok(Value::Object(object)),
        _ => Err("Assistant custom parameters must be a JSON object".into()),
    }
}

fn resolve_model_web_search(
    enabled: bool,
    runtime: &ProviderRuntimeConfig,
    model: &ModelView,
) -> Result<bool, String> {
    if !enabled {
        return Ok(false);
    }
    if !model.capabilities.web() {
        return Err(format!(
            "Web search is enabled, but model \"{}\" does not have web search capability enabled.",
            model.alias_or_request_name()
        ));
    }
    let descriptor = descriptor_for(&runtime.protocol)?;
    if !infer_capabilities(
        descriptor.capability_profile,
        &runtime.base_url,
        &model.request_name,
    )
    .web()
    {
        return Err(format!(
            "Web search is not supported for provider protocol {} and model \"{}\".",
            runtime.protocol.as_str(),
            model.alias_or_request_name()
        ));
    }
    Ok(true)
}

fn resolve_model_thinking(
    effort: ThinkingEffort,
    runtime: &ProviderRuntimeConfig,
    model: &ModelView,
) -> Result<Option<ThinkingConfig>, String> {
    if effort == ThinkingEffort::None {
        if model.capabilities.thinking_required() {
            return Err(format!(
                "Model \"{}\" requires thinking. Select the default {:?} effort or another supported effort.",
                model.alias_or_request_name(),
                model.capabilities.default_thinking_effort().unwrap_or(ThinkingEffort::High)
            ));
        }
        if model.capabilities.default_thinking_effort().is_none() {
            return Ok(None);
        }
        let descriptor = descriptor_for(&runtime.protocol)?;
        return resolve_thinking(
            descriptor.capability_profile,
            &runtime.base_url,
            &model.request_name,
            effort,
        )
        .map(Some);
    }
    if !model.capabilities.reasoning() {
        return Err(format!(
            "Model \"{}\" does not have reasoning capability enabled. Set thinking effort to None or enable reasoning for this model.",
            model.alias_or_request_name()
        ));
    }
    if !model.capabilities.thinking_efforts().is_empty()
        && !model.capabilities.thinking_efforts().contains(&effort)
    {
        return Err(format!(
            "Thinking effort {effort:?} is not supported by model \"{}\".",
            model.alias_or_request_name()
        ));
    }

    let descriptor = descriptor_for(&runtime.protocol)?;
    Ok(Some(resolve_thinking(
        descriptor.capability_profile,
        &runtime.base_url,
        &model.request_name,
        effort,
    )?))
}

trait ModelLabel {
    fn alias_or_request_name(&self) -> &str;
}

impl ModelLabel for ModelView {
    fn alias_or_request_name(&self) -> &str {
        if self.alias.trim().is_empty() {
            &self.request_name
        } else {
            &self.alias
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ProtocolId, ThinkingMode, UnifiedChatRequest, UnifiedContent, UnifiedMessage,
    };
    use crate::providers::capabilities::{infer_capabilities, ModelCapabilities};
    use crate::providers::protocols::anthropic;
    use serde_json::json;

    fn runtime(protocol: &str, base_url: &str) -> ProviderRuntimeConfig {
        ProviderRuntimeConfig {
            protocol: ProtocolId::registered(protocol),
            base_url: base_url.into(),
            use_raw_base_url: false,
            config: json!({}),
            credential: None,
            custom_headers: Vec::new(),
        }
    }

    fn model(request_name: &str, reasoning: bool) -> ModelView {
        ModelView {
            id: "model-1".into(),
            provider_id: "provider-1".into(),
            request_name: request_name.into(),
            alias: String::new(),
            source: "custom".into(),
            capabilities: ModelCapabilities::from_inferred(
                reasoning,
                false,
                if reasoning {
                    Vec::new()
                } else {
                    vec![ThinkingEffort::None]
                },
                false,
                None,
            )
            .expect("valid test capabilities"),
            test_status: "untested".into(),
            latency_ms: None,
            tested_at: None,
            test_error: None,
        }
    }

    fn config(effort: ThinkingEffort) -> TranslationConfigView {
        TranslationConfigView {
            thinking_effort: effort,
            ..TranslationConfigView::default()
        }
    }

    fn config_with_custom_parameters() -> TranslationConfigView {
        TranslationConfigView {
            use_custom_parameters: true,
            ..config(ThinkingEffort::None)
        }
    }

    fn config_with_web_search() -> TranslationConfigView {
        TranslationConfigView {
            use_web_search: true,
            ..config(ThinkingEffort::None)
        }
    }

    fn web_model(request_name: &str) -> ModelView {
        ModelView {
            capabilities: ModelCapabilities::legacy(false, true),
            ..model(request_name, false)
        }
    }

    fn anthropic_model(request_name: &str) -> ModelView {
        let mut value = model(request_name, true);
        let descriptor =
            descriptor_for(&ProtocolId::registered("anthropic")).expect("Anthropic descriptor");
        value.capabilities = infer_capabilities(
            descriptor.capability_profile,
            "https://api.anthropic.com",
            request_name,
        );
        value
    }

    #[test]
    fn none_thinking_sends_no_thinking_config() {
        let options = resolve_translation_request_options(
            &config(ThinkingEffort::None),
            &runtime("openai-responses", "https://api.openai.com"),
            &model("gpt-5", false),
            json!({}),
        )
        .expect("options");

        assert!(options.thinking.is_none());
    }

    #[test]
    fn default_on_and_always_on_models_resolve_none_differently() {
        let opus = resolve_translation_request_options(
            &config(ThinkingEffort::None),
            &runtime("anthropic", "https://api.anthropic.com"),
            &anthropic_model("claude-opus-5"),
            json!({}),
        )
        .expect("Opus 5 can explicitly disable default thinking");
        assert_eq!(
            opus.thinking.as_ref().map(|thinking| thinking.mode),
            Some(ThinkingMode::Disabled)
        );

        let error = resolve_translation_request_options(
            &config(ThinkingEffort::None),
            &runtime("anthropic", "https://api.anthropic.com"),
            &anthropic_model("claude-fable-5"),
            json!({}),
        )
        .expect_err("Fable 5 cannot disable thinking");
        assert!(error.contains("requires thinking"));
    }

    #[test]
    fn anthropic_preflight_matches_codec_validation_and_plans_dynamic_budget() {
        let runtime = runtime("anthropic", "https://api.anthropic.com");
        let adaptive_model = anthropic_model("claude-sonnet-4-6");
        let options = resolve_translation_request_options(
            &config(ThinkingEffort::High),
            &runtime,
            &adaptive_model,
            json!({}),
        )
        .expect("adaptive options");
        let preflight_error = validate_and_plan_model_request(
            &runtime,
            &adaptive_model,
            &options,
            Some(0.2),
            None,
            visible_output_token_reserve(800),
        )
        .expect_err("4.6 thinking sampling preflight");
        let request = UnifiedChatRequest {
            model: adaptive_model.request_name.clone(),
            messages: vec![UnifiedMessage {
                role: "user".into(),
                content: vec![UnifiedContent::Text {
                    text: "Translate this".into(),
                }],
            }],
            web_search: false,
            thinking: options.thinking.clone(),
            max_output_tokens: None,
            temperature: Some(0.2),
            top_p: None,
            logprobs: false,
            custom_parameters: json!({}),
        };
        let codec_error = anthropic::build_body(&request).expect_err("same codec validation");
        assert_eq!(preflight_error, codec_error);

        let manual_model = anthropic_model("claude-sonnet-4-5");
        let manual_options = resolve_translation_request_options(
            &config(ThinkingEffort::High),
            &runtime,
            &manual_model,
            json!({}),
        )
        .expect("manual options");
        let planned = validate_and_plan_model_request(
            &runtime,
            &manual_model,
            &manual_options,
            None,
            None,
            visible_output_token_reserve(8_000),
        )
        .expect("planned max tokens");
        assert_eq!(planned.wire_max_output_tokens, Some(48_000));

        let unknown = model("private-claude-model", false);
        let unknown_options = resolve_translation_request_options(
            &config(ThinkingEffort::None),
            &runtime,
            &unknown,
            json!({}),
        )
        .expect("unknown options without thinking");
        let planned = validate_and_plan_model_request(
            &runtime,
            &unknown,
            &unknown_options,
            None,
            None,
            visible_output_token_reserve(8_000),
        )
        .expect("conservative unknown plan");
        assert_eq!(planned.wire_max_output_tokens, Some(4_096));
    }

    #[test]
    fn reasoning_requires_model_capability() {
        let error = resolve_translation_request_options(
            &config(ThinkingEffort::Low),
            &runtime("openai-responses", "https://api.openai.com"),
            &model("gpt-5", false),
            json!({}),
        )
        .expect_err("reasoning capability error");

        assert!(error.contains("reasoning capability"));
    }

    #[test]
    fn rejects_effort_outside_non_empty_supported_effort_list() {
        let mut model = model("deepseek-v4", true);
        model.capabilities = ModelCapabilities::from_inferred(
            true,
            false,
            vec![
                ThinkingEffort::None,
                ThinkingEffort::High,
                ThinkingEffort::Max,
            ],
            false,
            None,
        )
        .expect("valid test capabilities");
        let error = resolve_translation_request_options(
            &config(ThinkingEffort::Low),
            &runtime("openai-chat", "https://api.deepseek.com"),
            &model,
            json!({}),
        )
        .expect_err("unsupported effort");

        assert!(error.contains("not supported"));
    }

    #[test]
    fn central_profile_rejects_invalid_effort_even_for_legacy_empty_lists() {
        let error = resolve_translation_request_options(
            &config(ThinkingEffort::Low),
            &runtime("openai-chat", "https://api.deepseek.com"),
            &model("deepseek-v4", true),
            json!({}),
        )
        .expect_err("central profile owns the effort set");

        assert!(error.contains("does not support thinking effort Low"));
    }

    #[test]
    fn routes_deepseek_effort_to_high_or_max() {
        let options = resolve_translation_request_options(
            &config(ThinkingEffort::High),
            &runtime("openai-chat", "https://api.deepseek.com"),
            &model("deepseek-v4", true),
            json!({}),
        )
        .expect("high options");
        assert_eq!(
            options.thinking.as_ref().and_then(|item| item.effort),
            Some(ThinkingEffort::High)
        );

        let options = resolve_translation_request_options(
            &config(ThinkingEffort::Max),
            &runtime("openai-chat", "https://api.deepseek.com"),
            &model("deepseek-v4", true),
            json!({}),
        )
        .expect("max options");
        assert_eq!(
            options.thinking.as_ref().and_then(|item| item.effort),
            Some(ThinkingEffort::Max)
        );
    }

    #[test]
    fn routes_glm_effort_to_deepseek_style_levels() {
        let options = resolve_translation_request_options(
            &config(ThinkingEffort::High),
            &runtime("openai-chat", "https://open.bigmodel.cn/api/paas/v4"),
            &model("glm-5.2", true),
            json!({}),
        )
        .expect("glm options");

        assert_eq!(
            options.thinking.as_ref().and_then(|item| item.effort),
            Some(ThinkingEffort::High)
        );

        let options = resolve_translation_request_options(
            &config(ThinkingEffort::Max),
            &runtime("openai-chat", "https://open.bigmodel.cn/api/paas/v4"),
            &model("glm-5.2", true),
            json!({}),
        )
        .expect("glm max options");

        assert_eq!(
            options.thinking.as_ref().and_then(|item| item.effort),
            Some(ThinkingEffort::Max)
        );
    }

    #[test]
    fn qwen_provider_managed_thinking_does_not_invent_a_numeric_budget() {
        let options = resolve_translation_request_options(
            &config(ThinkingEffort::High),
            &runtime(
                "openai-chat",
                "https://dashscope.aliyuncs.com/compatible-mode/v1",
            ),
            &model("qwen3-235b-a22b", true),
            json!({}),
        )
        .expect("qwen options");

        assert_eq!(
            options
                .thinking
                .as_ref()
                .and_then(|item| item.budget_tokens),
            None
        );
    }

    #[test]
    fn custom_parameters_are_disabled_by_default() {
        let options = resolve_translation_request_options(
            &config(ThinkingEffort::None),
            &runtime("gemini", "https://generativelanguage.googleapis.com"),
            &model("gemini-2.5-pro", false),
            json!({
                "temperature": 0.2,
                "insituTools": {
                    "choice": "required",
                    "tools": [{
                        "name": "lookup_term",
                        "description": "Look up a term",
                        "inputSchema": {"type": "object", "properties": {}}
                    }]
                }
            }),
        )
        .expect("options");

        assert_eq!(options.custom_parameters, json!({}));
    }

    #[test]
    fn custom_parameters_enable_without_parsing_insitu_tools() {
        let parameters = json!({
            "temperature": 0.2,
            "insituTools": {
                "choice": "definitely-not-tool-protocol-anymore",
                "tools": [{
                    "name": "lookup_term",
                    "description": "Look up a term",
                    "inputSchema": {"type": "object"}
                }]
            }
        });
        let options = resolve_translation_request_options(
            &config_with_custom_parameters(),
            &runtime("anthropic", "https://api.anthropic.com"),
            &model("claude-sonnet-4", false),
            parameters.clone(),
        )
        .expect("custom parameters");

        assert_eq!(options.custom_parameters, parameters);
    }

    #[test]
    fn custom_parameters_require_json_object_when_enabled() {
        let error = resolve_translation_request_options(
            &config_with_custom_parameters(),
            &runtime("anthropic", "https://api.anthropic.com"),
            &model("claude-sonnet-4", false),
            json!(["not-object"]),
        )
        .expect_err("custom parameter shape error");

        assert!(error.contains("custom parameters must be a JSON object"));
    }

    #[test]
    fn custom_parameters_disabled_ignores_invalid_shape() {
        let options = resolve_translation_request_options(
            &config(ThinkingEffort::None),
            &runtime("anthropic", "https://api.anthropic.com"),
            &model("claude-sonnet-4", false),
            json!({
                "temperature": 0.1,
                "insituTools": {"choice": "definitely-not-valid"}
            }),
        )
        .expect("custom parameters ignored");

        assert_eq!(options.custom_parameters, json!({}));
    }

    #[test]
    fn web_search_requires_model_capability() {
        let error = resolve_translation_request_options(
            &config_with_web_search(),
            &runtime("openai-responses", "https://api.openai.com"),
            &model("gpt-5", false),
            json!({}),
        )
        .expect_err("web capability error");

        assert!(error.contains("web search capability"));
    }

    #[test]
    fn web_search_requires_native_provider_support() {
        let error = resolve_translation_request_options(
            &config_with_web_search(),
            &runtime("openai-chat", "https://api.deepseek.com"),
            &web_model("deepseek-chat"),
            json!({}),
        )
        .expect_err("native web support error");

        assert!(error.contains("Web search is not supported"));
    }

    #[test]
    fn web_search_sets_request_option_for_supported_models() {
        let options = resolve_translation_request_options(
            &config_with_web_search(),
            &runtime("gemini", "https://generativelanguage.googleapis.com"),
            &web_model("gemini-2.5-pro"),
            json!({}),
        )
        .expect("web search options");

        assert!(options.web_search);
    }

    #[test]
    fn deepseek_legacy_max_tokens_is_promoted_during_preflight() {
        let runtime = runtime("openai-chat", "https://api.deepseek.com");
        let model = model("deepseek-chat", false);
        let options = resolve_translation_request_options(
            &config_with_custom_parameters(),
            &runtime,
            &model,
            json!({"max_tokens": 8192, "response_format": {"type": "json_object"}}),
        )
        .expect("DeepSeek custom parameters");

        let planned = validate_and_plan_model_request(
            &runtime,
            &model,
            &options,
            None,
            None,
            visible_output_token_reserve(2_000),
        )
        .expect("DeepSeek completion budget");
        assert_eq!(planned.wire_max_output_tokens, Some(8192));
    }
}
