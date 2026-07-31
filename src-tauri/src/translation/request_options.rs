use serde_json::Value;

use crate::domain::{ModelView, ProviderRuntimeConfig, ThinkingConfig, ThinkingEffort};
use crate::providers::descriptor_for;

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
    if !model.capability_web {
        return Err(format!(
            "Web search is enabled, but model \"{}\" does not have web search capability enabled.",
            model.alias_or_request_name()
        ));
    }
    let descriptor = descriptor_for(&runtime.protocol)?;
    if !descriptor
        .codec
        .infer_capabilities(&runtime.base_url, &model.request_name)
        .web
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
        return Ok(None);
    }
    if !model.capability_reasoning {
        return Err(format!(
            "Model \"{}\" does not have reasoning capability enabled. Set thinking effort to None or enable reasoning for this model.",
            model.alias_or_request_name()
        ));
    }
    if !model.supported_thinking_efforts.is_empty()
        && !model.supported_thinking_efforts.contains(&effort)
    {
        return Err(format!(
            "Thinking effort {effort:?} is not supported by model \"{}\".",
            model.alias_or_request_name()
        ));
    }

    let descriptor = descriptor_for(&runtime.protocol)?;
    Ok(Some(descriptor.codec.resolve_thinking(
        &runtime.base_url,
        &model.request_name,
        effort,
    )))
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
    use crate::domain::ProtocolId;
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
            capability_reasoning: reasoning,
            supported_thinking_efforts: Vec::new(),
            capability_web: false,
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
            capability_web: true,
            ..model(request_name, false)
        }
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
        model.supported_thinking_efforts = vec![
            ThinkingEffort::None,
            ThinkingEffort::High,
            ThinkingEffort::Max,
        ];
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
    fn empty_supported_effort_list_keeps_legacy_mapping_compatibility() {
        let options = resolve_translation_request_options(
            &config(ThinkingEffort::Low),
            &runtime("openai-chat", "https://api.deepseek.com"),
            &model("deepseek-v4", true),
            json!({}),
        )
        .expect("legacy unknown effort list");

        assert_eq!(
            options.thinking.as_ref().and_then(|item| item.effort),
            Some(ThinkingEffort::High)
        );
    }

    #[test]
    fn routes_deepseek_effort_to_high_or_max() {
        let options = resolve_translation_request_options(
            &config(ThinkingEffort::Low),
            &runtime("openai-chat", "https://api.deepseek.com"),
            &model("deepseek-v4", true),
            json!({}),
        )
        .expect("low options");
        assert_eq!(
            options.thinking.as_ref().and_then(|item| item.effort),
            Some(ThinkingEffort::High)
        );

        let options = resolve_translation_request_options(
            &config(ThinkingEffort::Xhigh),
            &runtime("openai-chat", "https://api.deepseek.com"),
            &model("deepseek-v4", true),
            json!({}),
        )
        .expect("xhigh options");
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
    fn qwen_budget_models_get_thinking_budget() {
        let options = resolve_translation_request_options(
            &config(ThinkingEffort::Medium),
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
            Some(16_000)
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
}
