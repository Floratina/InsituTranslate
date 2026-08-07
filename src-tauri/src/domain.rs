use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::providers::capabilities::ModelCapabilities;

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct ProtocolId(String);

impl ProtocolId {
    pub const UNKNOWN_VALUE: &'static str = "unknown";

    pub fn registered(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn unknown() -> Self {
        Self(Self::UNKNOWN_VALUE.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_unknown(&self) -> bool {
        self.0 == Self::UNKNOWN_VALUE
    }
}

impl<'de> Deserialize<'de> for ProtocolId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        if crate::providers::registry::descriptor_by_id(&raw)
            .map_err(serde::de::Error::custom)?
            .is_some()
        {
            Ok(Self::registered(raw))
        } else {
            Ok(Self::unknown())
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProtocolStatus {
    Available,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderPurpose {
    Translation,
    Glossary,
    Proofreading,
    DocumentParsing,
}

impl ProviderPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Translation => "translation",
            Self::Glossary => "glossary",
            Self::Proofreading => "proofreading",
            Self::DocumentParsing => "document-parsing",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "translation" => Ok(Self::Translation),
            "glossary" => Ok(Self::Glossary),
            "proofreading" => Ok(Self::Proofreading),
            "document-parsing" => Ok(Self::DocumentParsing),
            _ => Err(format!("Unsupported provider purpose: {value}")),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AssistantIconKind {
    Emoji,
    Lucide,
}

impl AssistantIconKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Emoji => "emoji",
            Self::Lucide => "lucide",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "emoji" => Ok(Self::Emoji),
            "lucide" => Ok(Self::Lucide),
            _ => Err(format!("Unsupported assistant icon kind: {value}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantView {
    pub id: String,
    pub name: String,
    pub icon_kind: AssistantIconKind,
    pub icon_value: String,
    pub purpose: ProviderPurpose,
    pub system_prompt: String,
    pub temperature_enabled: bool,
    pub temperature: f64,
    pub top_p_enabled: bool,
    pub top_p: f64,
    pub custom_parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAssistantInput {
    pub purpose: ProviderPurpose,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAssistantSettingsInput {
    pub id: String,
    pub name: String,
    pub icon_kind: AssistantIconKind,
    pub icon_value: String,
    pub temperature_enabled: bool,
    pub temperature: f64,
    pub top_p_enabled: bool,
    pub top_p: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAssistantPromptInput {
    pub id: String,
    pub system_prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAssistantCustomParametersInput {
    pub id: String,
    pub custom_parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderAssistantsInput {
    pub purpose: ProviderPurpose,
    pub assistant_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyAssistantInput {
    pub assistant_id: String,
    pub purpose: ProviderPurpose,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub protocol: ProtocolId,
    pub protocol_status: ProtocolStatus,
    pub protocol_raw_id: Option<String>,
    pub base_url: String,
    pub use_raw_base_url: bool,
    pub config: Value,
    pub config_issues: Vec<ProviderConfigIssue>,
    pub avatar: Option<String>,
    pub is_builtin: bool,
    pub enabled: bool,
    pub credential_mask: Option<String>,
    pub custom_header_keys: Vec<String>,
    pub purpose: ProviderPurpose,
    pub models: Vec<ModelView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfigIssue {
    pub pointer: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelView {
    pub id: String,
    pub provider_id: String,
    pub request_name: String,
    pub alias: String,
    pub source: String,
    pub capabilities: ModelCapabilities,
    pub test_status: String,
    pub latency_ms: Option<i64>,
    pub tested_at: Option<String>,
    pub test_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProviderInput {
    pub name: String,
    pub protocol: ProtocolId,
    pub purpose: ProviderPurpose,
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProviderConfigInput {
    pub id: String,
    pub base_url: String,
    pub use_raw_base_url: bool,
    #[serde(default)]
    pub config: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewProtocolEndpointsInput {
    pub protocol: ProtocolId,
    pub base_url: String,
    pub use_raw_base_url: bool,
    #[serde(default)]
    pub config: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateVertexAiConfigInput {
    pub provider_id: String,
    pub project_id: String,
    pub location: String,
    pub client_email: String,
    #[serde(default)]
    pub private_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportVertexAiServiceAccountInput {
    pub provider_id: String,
    pub service_account_json: String,
    #[serde(default)]
    pub location: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProviderMetadataInput {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairProviderProtocolInput {
    pub id: String,
    pub protocol: ProtocolId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetProviderEnabledInput {
    pub id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderProvidersInput {
    pub purpose: ProviderPurpose,
    pub provider_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyProviderInput {
    pub provider_id: String,
    pub purpose: ProviderPurpose,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddModelInput {
    pub provider_id: String,
    pub request_name: String,
    pub alias: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateModelInput {
    pub id: String,
    pub alias: String,
    pub capabilities: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteModel {
    pub request_name: String,
    pub alias: String,
    pub added: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectivityResult {
    pub success: bool,
    pub latency_ms: i64,
    pub tested_at: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingConfig {
    pub mode: ThinkingMode,
    pub budget_tokens: Option<u32>,
    pub effort: Option<ThinkingEffort>,
    pub summary: Option<ThinkingSummary>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ThinkingMode {
    Enabled,
    Disabled,
    Auto,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ThinkingEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Default for ThinkingEffort {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ThinkingSummary {
    None,
    Auto,
    Concise,
    Detailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum UnifiedContent {
    Text {
        text: String,
    },
    CacheableText {
        text: String,
    },
    Image {
        media_type: String,
        data: String,
    },
    Thinking {
        text: String,
        signature: Option<String>,
        encrypted_data: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedMessage {
    pub role: String,
    pub content: Vec<UnifiedContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedChatRequest {
    pub model: String,
    pub messages: Vec<UnifiedMessage>,
    #[serde(default)]
    pub web_search: bool,
    pub thinking: Option<ThinkingConfig>,
    pub max_output_tokens: Option<u32>,
    pub temperature: Option<f64>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub logprobs: bool,
    #[serde(default = "default_custom_parameters")]
    pub custom_parameters: Value,
}

fn default_custom_parameters() -> Value {
    Value::Object(serde_json::Map::new())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub thinking_tokens: u64,
    pub total_tokens: u64,
    #[serde(skip)]
    pub(crate) provenance: UnifiedUsageProvenance,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub(crate) struct UnifiedUsageProvenance {
    pub input_tokens_reported: bool,
    pub output_tokens_reported: bool,
    #[allow(dead_code)]
    pub cached_tokens_reported: bool,
    pub thinking_tokens_reported: bool,
    pub output_includes_unreported_thinking: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogprobStats {
    pub token_count: u64,
    pub average_probability: f64,
    pub standard_deviation: f64,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedChatResponse {
    pub text: String,
    pub reasoning: String,
    pub thinking: Vec<UnifiedContent>,
    pub usage: Option<UnifiedUsage>,
    pub logprob_stats: Option<LogprobStats>,
    pub raw: Value,
}

#[derive(Debug, Clone)]
pub struct ProviderRuntimeConfig {
    pub protocol: ProtocolId,
    pub base_url: String,
    pub use_raw_base_url: bool,
    pub config: Value,
    pub credential: Option<String>,
    pub custom_headers: Vec<(String, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_id_deserialization_degrades_unknown_strings() {
        let known: ProtocolId = serde_json::from_str("\"openai-chat\"").expect("known id");
        assert_eq!(known.as_str(), "openai-chat");

        let unknown: ProtocolId =
            serde_json::from_str("\"retired-chat-v0\"").expect("unknown string");
        assert!(unknown.is_unknown());
        let blank: ProtocolId = serde_json::from_str("\"   \"").expect("blank string");
        assert!(blank.is_unknown());
        assert!(serde_json::from_str::<ProtocolId>("42").is_err());
    }
}
