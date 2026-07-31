use serde::Serialize;
use serde_json::Value;

use crate::domain::ProtocolId;
#[cfg(test)]
use crate::providers::protocols::test_protocol;
use crate::providers::protocols::{
    anthropic, gemini, ollama, openai_chat, openai_responses, vertex_ai,
};
use crate::providers::ProtocolCodec;

pub struct ProtocolDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
    pub default_base_url: &'static str,
    pub wire_family: &'static str,
    pub config_kind: &'static str,
    pub supports_model_listing: bool,
    pub auth: AuthDescriptor,
    pub config_fields: &'static [ConfigField],
    pub help_text: Option<&'static str>,
    pub codec: &'static dyn ProtocolCodec,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthStrategy {
    None,
    StaticHeader {
        header: &'static str,
        scheme: Option<&'static str>,
    },
    VertexServiceAccount,
}

impl AuthStrategy {
    pub fn legacy_auth_type(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::StaticHeader {
                scheme: Some("Bearer"),
                ..
            } => "bearer",
            Self::StaticHeader { .. } => "api-key",
            Self::VertexServiceAccount => "service-account",
        }
    }

    pub const fn header(self) -> &'static str {
        match self {
            Self::None | Self::VertexServiceAccount => "Authorization",
            Self::StaticHeader { header, .. } => header,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AuthDescriptor {
    pub strategy: AuthStrategy,
    pub label: &'static str,
    pub help_text: Option<&'static str>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigFieldKind {
    Text,
    Number,
    Boolean,
    Select,
}

#[derive(Debug, Clone, Copy)]
pub struct ConfigOption {
    pub value: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct ConfigField {
    pub pointer: &'static str,
    pub label: &'static str,
    pub kind: ConfigFieldKind,
    pub required: bool,
    pub default_json: Option<&'static str>,
    pub options: &'static [ConfigOption],
    pub help_text: Option<&'static str>,
}

#[cfg(test)]
static TEST_OPTIONS: &[ConfigOption] = &[
    ConfigOption {
        value: "fast",
        label: "Fast",
    },
    ConfigOption {
        value: "quality",
        label: "Quality",
    },
];

#[cfg(test)]
static TEST_CONFIG_FIELDS: &[ConfigField] = &[ConfigField {
    pointer: "/mode",
    label: "Mode",
    kind: ConfigFieldKind::Select,
    required: true,
    default_json: Some("\"fast\""),
    options: TEST_OPTIONS,
    help_text: Some("Test-only schema field"),
}];

pub enum ProtocolResolution {
    Known(&'static ProtocolDescriptor),
    Unknown { id: ProtocolId, raw_id: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolDescriptorView {
    pub id: String,
    pub display_name: String,
    pub default_base_url: String,
    pub wire_family: String,
    pub config_kind: String,
    pub supports_model_listing: bool,
    pub auth: AuthDescriptorView,
    pub config_fields: Vec<ConfigFieldView>,
    pub help_text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthDescriptorView {
    pub kind: String,
    pub label: String,
    pub header: String,
    pub help_text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigOptionView {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFieldView {
    pub pointer: String,
    pub label: String,
    pub kind: ConfigFieldKind,
    pub required: bool,
    pub default_value: Option<Value>,
    pub options: Vec<ConfigOptionView>,
    pub help_text: Option<String>,
}

pub static DESCRIPTORS: &[ProtocolDescriptor] = &[
    ProtocolDescriptor {
        id: "openai-chat",
        display_name: "OpenAI Chat Completions",
        default_base_url: "https://api.openai.com",
        wire_family: "openai-chat",
        config_kind: "generic",
        supports_model_listing: true,
        auth: AuthDescriptor {
            strategy: AuthStrategy::StaticHeader {
                header: "Authorization",
                scheme: Some("Bearer"),
            },
            label: "API Key",
            help_text: None,
        },
        config_fields: &[],
        help_text: None,
        codec: &openai_chat::CODEC,
    },
    ProtocolDescriptor {
        id: "openai-responses",
        display_name: "OpenAI Responses",
        default_base_url: "https://api.openai.com",
        wire_family: "openai-responses",
        config_kind: "generic",
        supports_model_listing: true,
        auth: AuthDescriptor {
            strategy: AuthStrategy::StaticHeader {
                header: "Authorization",
                scheme: Some("Bearer"),
            },
            label: "API Key",
            help_text: None,
        },
        config_fields: &[],
        help_text: None,
        codec: &openai_responses::CODEC,
    },
    ProtocolDescriptor {
        id: "anthropic",
        display_name: "Anthropic Messages",
        default_base_url: "https://api.anthropic.com",
        wire_family: "anthropic-messages",
        config_kind: "generic",
        supports_model_listing: true,
        auth: AuthDescriptor {
            strategy: AuthStrategy::StaticHeader {
                header: "x-api-key",
                scheme: None,
            },
            label: "API Key",
            help_text: None,
        },
        config_fields: &[],
        help_text: None,
        codec: &anthropic::CODEC,
    },
    ProtocolDescriptor {
        id: "gemini",
        display_name: "Gemini API",
        default_base_url: "https://generativelanguage.googleapis.com",
        wire_family: "google-generative-language",
        config_kind: "generic",
        supports_model_listing: true,
        auth: AuthDescriptor {
            strategy: AuthStrategy::StaticHeader {
                header: "x-goog-api-key",
                scheme: None,
            },
            label: "API Key / Auth Key",
            help_text: Some(
                "Google 将在 2026 年 9 月起拒绝旧式标准 API Key，请优先使用 Auth Key。",
            ),
        },
        config_fields: &[],
        help_text: None,
        codec: &gemini::CODEC,
    },
    ProtocolDescriptor {
        id: "vertex-ai",
        display_name: "Agent Platform (Vertex AI)",
        default_base_url: crate::vertex_ai::DEFAULT_BASE_URL,
        wire_family: "google-generative-language",
        config_kind: "vertex-ai",
        supports_model_listing: true,
        auth: AuthDescriptor {
            strategy: AuthStrategy::VertexServiceAccount,
            label: "Service Account",
            help_text: None,
        },
        config_fields: &[],
        help_text: None,
        codec: &vertex_ai::CODEC,
    },
    ProtocolDescriptor {
        id: "ollama",
        display_name: "Ollama Chat",
        default_base_url: "http://localhost:11434/api",
        wire_family: "ollama-chat",
        config_kind: "generic",
        supports_model_listing: true,
        auth: AuthDescriptor {
            strategy: AuthStrategy::None,
            label: "无需凭证",
            help_text: None,
        },
        config_fields: &[],
        help_text: None,
        codec: &ollama::CODEC,
    },
    #[cfg(test)]
    ProtocolDescriptor {
        id: "test-seventh",
        display_name: "Test Seventh Protocol",
        default_base_url: "https://test.invalid",
        wire_family: "test-wire",
        config_kind: "generic",
        supports_model_listing: true,
        auth: AuthDescriptor {
            strategy: AuthStrategy::None,
            label: "No credential",
            help_text: None,
        },
        config_fields: TEST_CONFIG_FIELDS,
        help_text: Some("Registered only while running Rust tests"),
        codec: &test_protocol::CODEC,
    },
];

pub fn descriptor_for(id: &ProtocolId) -> Result<&'static ProtocolDescriptor, String> {
    resolve_input(id)
}

pub fn descriptor_by_id(id: &str) -> Option<&'static ProtocolDescriptor> {
    DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.id == id && descriptor.codec.id() == id)
}

pub fn resolve_persisted(raw_id: impl Into<String>) -> ProtocolResolution {
    let raw_id = raw_id.into();
    match descriptor_by_id(&raw_id) {
        Some(descriptor) => ProtocolResolution::Known(descriptor),
        None => ProtocolResolution::Unknown {
            id: ProtocolId::unknown(),
            raw_id,
        },
    }
}

pub fn resolve_input(id: &ProtocolId) -> Result<&'static ProtocolDescriptor, String> {
    if id.is_unknown() {
        return Err("Unknown or unavailable provider protocol: unknown".into());
    }
    descriptor_by_id(id.as_str())
        .ok_or_else(|| format!("Unknown or unavailable provider protocol: {}", id.as_str()))
}

pub fn descriptor_views() -> Vec<ProtocolDescriptorView> {
    DESCRIPTORS
        .iter()
        .map(|descriptor| ProtocolDescriptorView {
            id: descriptor.id.to_string(),
            display_name: descriptor.display_name.to_string(),
            default_base_url: descriptor.default_base_url.to_string(),
            wire_family: descriptor.wire_family.to_string(),
            config_kind: descriptor.config_kind.to_string(),
            supports_model_listing: descriptor.supports_model_listing,
            auth: AuthDescriptorView {
                kind: descriptor.auth.strategy.legacy_auth_type().to_string(),
                label: descriptor.auth.label.to_string(),
                header: descriptor.auth.strategy.header().to_string(),
                help_text: descriptor.auth.help_text.map(str::to_string),
            },
            config_fields: descriptor
                .config_fields
                .iter()
                .map(|field| ConfigFieldView {
                    pointer: field.pointer.to_string(),
                    label: field.label.to_string(),
                    kind: field.kind,
                    required: field.required,
                    default_value: field.default_json.map(|value| {
                        serde_json::from_str(value).expect("registered config field default JSON")
                    }),
                    options: field
                        .options
                        .iter()
                        .map(|option| ConfigOptionView {
                            value: option.value.to_string(),
                            label: option.label.to_string(),
                        })
                        .collect(),
                    help_text: field.help_text.map(str::to_string),
                })
                .collect(),
            help_text: descriptor.help_text.map(str::to_string),
        })
        .collect()
}
