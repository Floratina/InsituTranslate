use serde_json::Value;

use crate::domain::{
    ProviderRuntimeConfig, RemoteModel, ThinkingConfig, UnifiedChatRequest, UnifiedChatResponse,
};
use crate::providers::budget::{
    completion_budget, normalize_completion_budget, CompletionBudget, CompletionBudgetAlias,
    ALL_COMPLETION_BUDGET_ALIASES,
};
use crate::providers::capabilities::estimated_thinking_tokens;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderMode {
    Replace,
    IfAbsent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderDirective {
    pub name: String,
    pub value: String,
    pub mode: HeaderMode,
}

#[derive(Debug, Clone)]
pub struct EncodedRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<HeaderDirective>,
    pub body: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointPreview {
    pub chat: String,
    pub models: Option<String>,
}

pub trait ProtocolCodec: Send + Sync {
    fn id(&self) -> &'static str;
    fn encode_model_list(&self, config: &ProviderRuntimeConfig) -> Result<EncodedRequest, String>;
    fn decode_model_list(&self, raw: &Value) -> Result<Vec<RemoteModel>, String>;
    fn encode_chat(
        &self,
        config: &ProviderRuntimeConfig,
        request: &UnifiedChatRequest,
    ) -> Result<EncodedRequest, String>;
    fn decode_chat(&self, raw: Value) -> Result<UnifiedChatResponse, String>;
    fn finish_reason(&self, raw: &Value) -> Option<String>;
    fn completion_budget_aliases(&self) -> &'static [CompletionBudgetAlias] {
        ALL_COMPLETION_BUDGET_ALIASES
    }
    fn validate_chat_options(
        &self,
        _base_url: &str,
        _model_id: &str,
        _thinking: Option<&ThinkingConfig>,
        _temperature: Option<f64>,
        _top_p: Option<f64>,
        _custom_parameters: &Value,
    ) -> Result<(), String> {
        Ok(())
    }
    fn plan_max_output_tokens(
        &self,
        _base_url: &str,
        _model_id: &str,
        _thinking: Option<&ThinkingConfig>,
        structured_max_output_tokens: Option<u32>,
        _custom_parameters: &Value,
        _visible_output_tokens: u32,
    ) -> Result<Option<u32>, String> {
        normalize_completion_budget(
            structured_max_output_tokens,
            _custom_parameters,
            self.completion_budget_aliases(),
        )
        .map(|budget| budget.resolved_max_output_tokens(None))
    }
    fn plan_completion_budget(
        &self,
        base_url: &str,
        model_id: &str,
        thinking: Option<&ThinkingConfig>,
        structured_max_output_tokens: Option<u32>,
        custom_parameters: &Value,
        visible_output_tokens: u32,
    ) -> Result<CompletionBudget, String> {
        let normalized = normalize_completion_budget(
            structured_max_output_tokens,
            custom_parameters,
            self.completion_budget_aliases(),
        )?;
        let planned_wire_max = self.plan_max_output_tokens(
            base_url,
            model_id,
            thinking,
            normalized.explicit_max_output_tokens(),
            normalized.custom_parameters(),
            visible_output_tokens,
        )?;
        completion_budget(
            normalized,
            visible_output_tokens,
            estimated_thinking_tokens(thinking, visible_output_tokens),
            planned_wire_max,
            self.id(),
            model_id,
        )
    }
    fn preview_endpoints(&self, config: &ProviderRuntimeConfig) -> Result<EndpointPreview, String>;

    fn decode_error(&self, status: u16, body: &str) -> String {
        format!("HTTP {status}: {}", truncate(body, 500))
    }
}

pub fn append_endpoint_suffix(base_url: &str, suffix: &str) -> String {
    let base = endpoint_base_url(base_url).trim_end_matches('/');
    let suffix = suffix.trim_start_matches('/');
    if suffix.is_empty() {
        base.to_string()
    } else {
        format!("{base}/{suffix}")
    }
}

pub fn endpoint_base_url(base_url: &str) -> &str {
    base_url.split(['?', '#']).next().unwrap_or(base_url)
}

pub fn openai_endpoint(config: &ProviderRuntimeConfig, suffix: &str) -> String {
    let base = endpoint_base_url(&config.base_url).trim_end_matches('/');
    if config.use_raw_base_url || is_versioned_base_url(base) {
        append_endpoint_suffix(base, suffix)
    } else {
        append_endpoint_suffix(&format!("{base}/v1"), suffix)
    }
}

fn is_versioned_base_url(base_url: &str) -> bool {
    base_url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .map(|segment| {
            segment == "v1"
                || segment == "v1beta"
                || segment.strip_prefix('v').is_some_and(|version| {
                    version.chars().all(|character| character.is_ascii_digit())
                })
        })
        .unwrap_or(false)
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        value.chars().take(max).collect::<String>() + "…"
    }
}
