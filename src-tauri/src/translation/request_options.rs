use serde_json::{Map, Value};

use crate::domain::{
    ModelView, ProviderProtocol, ProviderRuntimeConfig, ThinkingConfig, ThinkingEffort,
    ThinkingMode, UnifiedTool, UnifiedToolChoice,
};
use crate::features::{is_feature_supported, native_web_search_supported, FeatureId};

use super::TranslationConfigView;

const INSITU_TOOLS_KEY: &str = "insituTools";

#[derive(Debug, Clone)]
pub(super) struct TranslationRequestOptions {
    pub custom_parameters: Value,
    pub tools: Vec<UnifiedTool>,
    pub tool_choice: UnifiedToolChoice,
    pub web_search: bool,
    pub thinking: Option<ThinkingConfig>,
}

pub(super) fn resolve_translation_request_options(
    config: &TranslationConfigView,
    runtime: &ProviderRuntimeConfig,
    model: &ModelView,
    custom_parameters: Value,
) -> Result<TranslationRequestOptions, String> {
    let (custom_parameters, tools, tool_choice) = if config.use_tools {
        if !model.capability_tools {
            return Err(
                "Tool calling is enabled, but the selected model does not have tool calling capability enabled."
                    .into(),
            );
        }
        extract_insitu_tools(custom_parameters)?
    } else {
        (
            remove_insitu_tools(custom_parameters)?,
            Vec::new(),
            UnifiedToolChoice::Auto,
        )
    };
    let thinking = resolve_translation_thinking(config.thinking_effort, runtime, model)?;
    let web_search = resolve_translation_web_search(config.use_web_search, runtime, model)?;
    Ok(TranslationRequestOptions {
        custom_parameters,
        tools,
        tool_choice,
        web_search,
        thinking,
    })
}

fn resolve_translation_web_search(
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
    if !native_web_search_supported(runtime.protocol, &runtime.base_url, &model.request_name) {
        return Err(format!(
            "Web search is not supported for provider protocol {} and model \"{}\".",
            runtime.protocol.as_str(),
            model.alias_or_request_name()
        ));
    }
    Ok(true)
}

fn resolve_translation_thinking(
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

    let base_url = runtime.base_url.as_str();
    let model_id = model.request_name.as_str();
    let mut thinking = ThinkingConfig {
        mode: ThinkingMode::Enabled,
        budget_tokens: None,
        effort: Some(effort),
        summary: None,
    };

    match runtime.protocol {
        ProviderProtocol::OpenaiResponses => {
            thinking.effort = Some(openai_reasoning_effort(effort));
        }
        ProviderProtocol::OpenaiChat => {
            if is_feature_supported(FeatureId::OpenAiDeepSeekReasoningEffort, base_url, model_id) {
                thinking.effort = Some(deepseek_reasoning_effort(effort));
            } else if is_feature_supported(FeatureId::OpenAiEnableThinking, base_url, model_id) {
                thinking.effort = Some(openai_reasoning_effort(effort));
                if is_feature_supported(FeatureId::OpenAiThinkingBudget, base_url, model_id) {
                    thinking.budget_tokens = Some(budget_tokens_for_effort(effort));
                }
            } else if is_feature_supported(FeatureId::OpenAiReasoningEffort, base_url, model_id) {
                thinking.effort = Some(volc_reasoning_effort(effort));
            } else {
                thinking.effort = Some(openai_reasoning_effort(effort));
            }
        }
        ProviderProtocol::Anthropic => {
            thinking.budget_tokens = Some(budget_tokens_for_effort(effort));
        }
        ProviderProtocol::Gemini | ProviderProtocol::VertexAi => {
            if is_feature_supported(FeatureId::GeminiThinkingLevel, base_url, model_id) {
                thinking.effort = Some(gemini_thinking_level_effort(effort));
            } else {
                thinking.budget_tokens = Some(budget_tokens_for_effort(effort));
            }
        }
        ProviderProtocol::Ollama => {
            thinking.effort = Some(ollama_thinking_effort(effort));
        }
    }

    Ok(Some(thinking))
}

fn remove_insitu_tools(custom_parameters: Value) -> Result<Value, String> {
    let mut custom = match custom_parameters {
        Value::Null => Map::new(),
        Value::Object(object) => object,
        _ => return Err("Assistant custom parameters must be a JSON object".into()),
    };
    custom.remove(INSITU_TOOLS_KEY);
    Ok(Value::Object(custom))
}

fn extract_insitu_tools(
    custom_parameters: Value,
) -> Result<(Value, Vec<UnifiedTool>, UnifiedToolChoice), String> {
    let mut custom = match custom_parameters {
        Value::Null => Map::new(),
        Value::Object(object) => object,
        _ => return Err("Assistant custom parameters must be a JSON object".into()),
    };
    let Some(insitu_tools) = custom.remove(INSITU_TOOLS_KEY) else {
        return Ok((Value::Object(custom), Vec::new(), UnifiedToolChoice::Auto));
    };
    if insitu_tools.is_null() {
        return Ok((Value::Object(custom), Vec::new(), UnifiedToolChoice::Auto));
    }
    let object = insitu_tools
        .as_object()
        .ok_or_else(|| "customParameters.insituTools must be a JSON object".to_string())?;
    let choice = parse_tool_choice(object.get("choice"))?;
    let tools = parse_tools(object.get("tools"))?;

    Ok((Value::Object(custom), tools, choice))
}

fn parse_tool_choice(value: Option<&Value>) -> Result<UnifiedToolChoice, String> {
    let Some(value) = value else {
        return Ok(UnifiedToolChoice::Auto);
    };
    match value.as_str() {
        Some("auto") => Ok(UnifiedToolChoice::Auto),
        Some("required") => Ok(UnifiedToolChoice::Required),
        Some("none") => Ok(UnifiedToolChoice::None),
        _ => Err("customParameters.insituTools.choice must be auto, required, or none".into()),
    }
}

fn parse_tools(value: Option<&Value>) -> Result<Vec<UnifiedTool>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let tools = value
        .as_array()
        .ok_or_else(|| "customParameters.insituTools.tools must be a JSON array".to_string())?;
    let mut output = Vec::with_capacity(tools.len());
    for (index, tool) in tools.iter().enumerate() {
        let object = tool.as_object().ok_or_else(|| {
            format!("customParameters.insituTools.tools[{index}] must be a JSON object")
        })?;
        let name = required_string(object, "name", index)?;
        let description = required_string(object, "description", index)?;
        let input_schema = object
            .get("inputSchema")
            .filter(|value| value.is_object())
            .cloned()
            .ok_or_else(|| {
                format!(
                    "customParameters.insituTools.tools[{index}].inputSchema must be a JSON object"
                )
            })?;
        output.push(UnifiedTool {
            name,
            description,
            input_schema,
        });
    }
    Ok(output)
}

fn required_string(object: &Map<String, Value>, key: &str, index: usize) -> Result<String, String> {
    let value = object
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            format!("customParameters.insituTools.tools[{index}].{key} must be a non-empty string")
        })?;
    Ok(value.to_string())
}

fn openai_reasoning_effort(effort: ThinkingEffort) -> ThinkingEffort {
    match effort {
        ThinkingEffort::Max => ThinkingEffort::Xhigh,
        other => other,
    }
}

fn volc_reasoning_effort(effort: ThinkingEffort) -> ThinkingEffort {
    match effort {
        ThinkingEffort::Xhigh | ThinkingEffort::Max => ThinkingEffort::High,
        other => other,
    }
}

fn deepseek_reasoning_effort(effort: ThinkingEffort) -> ThinkingEffort {
    match effort {
        ThinkingEffort::Xhigh | ThinkingEffort::Max => ThinkingEffort::Max,
        _ => ThinkingEffort::High,
    }
}

fn gemini_thinking_level_effort(effort: ThinkingEffort) -> ThinkingEffort {
    match effort {
        ThinkingEffort::Xhigh | ThinkingEffort::Max => ThinkingEffort::High,
        other => other,
    }
}

fn ollama_thinking_effort(effort: ThinkingEffort) -> ThinkingEffort {
    match effort {
        ThinkingEffort::Minimal => ThinkingEffort::Low,
        ThinkingEffort::Xhigh | ThinkingEffort::Max => ThinkingEffort::High,
        other => other,
    }
}

fn budget_tokens_for_effort(effort: ThinkingEffort) -> u32 {
    match effort {
        ThinkingEffort::None => 0,
        ThinkingEffort::Minimal | ThinkingEffort::Low => 1024,
        ThinkingEffort::Medium => 16_000,
        ThinkingEffort::High | ThinkingEffort::Xhigh | ThinkingEffort::Max => 32_000,
    }
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
    use serde_json::json;

    fn runtime(protocol: ProviderProtocol, base_url: &str) -> ProviderRuntimeConfig {
        ProviderRuntimeConfig {
            protocol,
            base_url: base_url.into(),
            use_raw_base_url: false,
            config: json!({}),
            auth_type: "none".into(),
            auth_header: "Authorization".into(),
            credential: None,
            custom_headers: Vec::new(),
        }
    }

    fn model(request_name: &str, reasoning: bool, tools: bool) -> ModelView {
        ModelView {
            id: "model-1".into(),
            provider_id: "provider-1".into(),
            request_name: request_name.into(),
            alias: String::new(),
            source: "custom".into(),
            capability_reasoning: reasoning,
            capability_web: false,
            capability_tools: tools,
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

    fn config_with_tools() -> TranslationConfigView {
        TranslationConfigView {
            use_tools: true,
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
            ..model(request_name, false, false)
        }
    }

    #[test]
    fn none_thinking_sends_no_thinking_config() {
        let options = resolve_translation_request_options(
            &config(ThinkingEffort::None),
            &runtime(ProviderProtocol::OpenaiResponses, "https://api.openai.com"),
            &model("gpt-5", false, false),
            json!({}),
        )
        .expect("options");

        assert!(options.thinking.is_none());
    }

    #[test]
    fn reasoning_requires_model_capability() {
        let error = resolve_translation_request_options(
            &config(ThinkingEffort::Low),
            &runtime(ProviderProtocol::OpenaiResponses, "https://api.openai.com"),
            &model("gpt-5", false, false),
            json!({}),
        )
        .expect_err("reasoning capability error");

        assert!(error.contains("reasoning capability"));
    }

    #[test]
    fn routes_deepseek_effort_to_high_or_max() {
        let options = resolve_translation_request_options(
            &config(ThinkingEffort::Low),
            &runtime(ProviderProtocol::OpenaiChat, "https://api.deepseek.com"),
            &model("deepseek-v4", true, false),
            json!({}),
        )
        .expect("low options");
        assert_eq!(
            options.thinking.as_ref().and_then(|item| item.effort),
            Some(ThinkingEffort::High)
        );

        let options = resolve_translation_request_options(
            &config(ThinkingEffort::Xhigh),
            &runtime(ProviderProtocol::OpenaiChat, "https://api.deepseek.com"),
            &model("deepseek-v4", true, false),
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
            &runtime(
                ProviderProtocol::OpenaiChat,
                "https://open.bigmodel.cn/api/paas/v4",
            ),
            &model("glm-5.2", true, false),
            json!({}),
        )
        .expect("glm options");

        assert_eq!(
            options.thinking.as_ref().and_then(|item| item.effort),
            Some(ThinkingEffort::High)
        );

        let options = resolve_translation_request_options(
            &config(ThinkingEffort::Max),
            &runtime(
                ProviderProtocol::OpenaiChat,
                "https://open.bigmodel.cn/api/paas/v4",
            ),
            &model("glm-5.2", true, false),
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
                ProviderProtocol::OpenaiChat,
                "https://dashscope.aliyuncs.com/compatible-mode/v1",
            ),
            &model("qwen3-235b-a22b", true, false),
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
    fn parses_and_removes_insitu_tools() {
        let options = resolve_translation_request_options(
            &config_with_tools(),
            &runtime(
                ProviderProtocol::Gemini,
                "https://generativelanguage.googleapis.com",
            ),
            &model("gemini-2.5-pro", false, true),
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
        .expect("tool options");

        assert_eq!(options.custom_parameters, json!({"temperature": 0.2}));
        assert_eq!(options.tools.len(), 1);
        assert_eq!(options.tool_choice, UnifiedToolChoice::Required);
    }

    #[test]
    fn tools_require_model_capability() {
        let error = resolve_translation_request_options(
            &config_with_tools(),
            &runtime(ProviderProtocol::Anthropic, "https://api.anthropic.com"),
            &model("claude-sonnet-4", false, false),
            json!({
                "insituTools": {
                    "tools": [{
                        "name": "lookup_term",
                        "description": "Look up a term",
                        "inputSchema": {"type": "object"}
                    }]
                }
            }),
        )
        .expect_err("tool capability error");

        assert!(error.contains("tool calling capability"));
    }

    #[test]
    fn disabled_tools_remove_invalid_insitu_tools_without_parsing() {
        let options = resolve_translation_request_options(
            &config(ThinkingEffort::None),
            &runtime(ProviderProtocol::Anthropic, "https://api.anthropic.com"),
            &model("claude-sonnet-4", false, false),
            json!({
                "temperature": 0.1,
                "insituTools": {"choice": "definitely-not-valid"}
            }),
        )
        .expect("tool config ignored");

        assert_eq!(options.custom_parameters, json!({"temperature": 0.1}));
        assert!(options.tools.is_empty());
        assert_eq!(options.tool_choice, UnifiedToolChoice::Auto);
    }

    #[test]
    fn web_search_requires_model_capability() {
        let error = resolve_translation_request_options(
            &config_with_web_search(),
            &runtime(ProviderProtocol::OpenaiResponses, "https://api.openai.com"),
            &model("gpt-5", false, false),
            json!({}),
        )
        .expect_err("web capability error");

        assert!(error.contains("web search capability"));
    }

    #[test]
    fn web_search_requires_native_provider_support() {
        let error = resolve_translation_request_options(
            &config_with_web_search(),
            &runtime(ProviderProtocol::OpenaiChat, "https://api.deepseek.com"),
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
            &runtime(
                ProviderProtocol::Gemini,
                "https://generativelanguage.googleapis.com",
            ),
            &web_model("gemini-2.5-pro"),
            json!({}),
        )
        .expect("web search options");

        assert!(options.web_search);
    }
}
