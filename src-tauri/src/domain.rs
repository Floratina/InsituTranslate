use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderProtocol {
    OpenaiChat,
    OpenaiResponses,
    Anthropic,
    Gemini,
    VertexAi,
    Ollama,
}

impl ProviderProtocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiChat => "openai-chat",
            Self::OpenaiResponses => "openai-responses",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::VertexAi => "vertex-ai",
            Self::Ollama => "ollama",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "openai-chat" => Ok(Self::OpenaiChat),
            "openai-responses" => Ok(Self::OpenaiResponses),
            "anthropic" => Ok(Self::Anthropic),
            "gemini" => Ok(Self::Gemini),
            "vertex-ai" => Ok(Self::VertexAi),
            "ollama" => Ok(Self::Ollama),
            _ => Err(format!("Unsupported provider protocol: {value}")),
        }
    }
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
    pub protocol: ProviderProtocol,
    pub base_url: String,
    pub use_raw_base_url: bool,
    pub config: Value,
    pub avatar: Option<String>,
    pub is_builtin: bool,
    pub enabled: bool,
    pub credential_mask: Option<String>,
    pub custom_header_keys: Vec<String>,
    pub purpose: ProviderPurpose,
    pub models: Vec<ModelView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelView {
    pub id: String,
    pub provider_id: String,
    pub request_name: String,
    pub alias: String,
    pub source: String,
    pub capability_reasoning: bool,
    pub supported_thinking_efforts: Vec<ThinkingEffort>,
    pub capability_web: bool,
    pub test_status: String,
    pub latency_ms: Option<i64>,
    pub tested_at: Option<String>,
    pub test_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProviderInput {
    pub name: String,
    pub protocol: ProviderProtocol,
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
    pub protocol: ProviderProtocol,
    pub avatar: Option<String>,
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
    pub capability_reasoning: bool,
    pub capability_web: bool,
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
    pub stream: bool,
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
    pub protocol: ProviderProtocol,
    pub base_url: String,
    pub use_raw_base_url: bool,
    pub config: Value,
    pub auth_type: String,
    pub auth_header: String,
    pub credential: Option<String>,
    pub custom_headers: Vec<(String, String)>,
}
