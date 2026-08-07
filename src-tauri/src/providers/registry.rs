use std::collections::HashSet;
use std::sync::OnceLock;

use reqwest::header::HeaderName;
use serde::Serialize;
use serde_json::Value;

use crate::domain::ProtocolId;
use crate::providers::capabilities::CapabilityProfile;
use crate::providers::config_schema::{parse_default, validate_fields};
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
    pub capability_profile: CapabilityProfile,
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
static TEST_CONFIG_FIELDS: &[ConfigField] = &[
    ConfigField {
        pointer: "/mode",
        label: "Mode",
        kind: ConfigFieldKind::Select,
        required: true,
        default_json: Some("\"fast\""),
        options: TEST_OPTIONS,
        help_text: Some("Test-only schema field"),
    },
    ConfigField {
        pointer: "/label",
        label: "Label",
        kind: ConfigFieldKind::Text,
        required: false,
        default_json: Some("\"seventh\""),
        options: &[],
        help_text: None,
    },
    ConfigField {
        pointer: "/limits/retries",
        label: "Retries",
        kind: ConfigFieldKind::Number,
        required: false,
        default_json: Some("3"),
        options: &[],
        help_text: None,
    },
    ConfigField {
        pointer: "/features/cache",
        label: "Cache",
        kind: ConfigFieldKind::Boolean,
        required: false,
        default_json: Some("true"),
        options: &[],
        help_text: None,
    },
];

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
        capability_profile: CapabilityProfile::OpenAiChat,
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
        capability_profile: CapabilityProfile::OpenAiResponses,
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
        capability_profile: CapabilityProfile::Anthropic,
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
        capability_profile: CapabilityProfile::Gemini,
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
        capability_profile: CapabilityProfile::VertexAi,
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
        capability_profile: CapabilityProfile::Ollama,
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
        capability_profile: CapabilityProfile::Test,
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

static REGISTRY_VALIDATION: OnceLock<Result<(), String>> = OnceLock::new();

pub fn validate_registry() -> Result<(), String> {
    crate::providers::capabilities::validate_registry()?;
    REGISTRY_VALIDATION
        .get_or_init(|| validate_descriptors(DESCRIPTORS))
        .clone()
}

fn validate_descriptors(descriptors: &[ProtocolDescriptor]) -> Result<(), String> {
    let mut ids = HashSet::new();
    for descriptor in descriptors {
        if descriptor.id.trim().is_empty() {
            return Err("Provider protocol ID cannot be empty".into());
        }
        if descriptor.id == ProtocolId::UNKNOWN_VALUE {
            return Err("Provider protocol ID 'unknown' is reserved".into());
        }
        if !ids.insert(descriptor.id) {
            return Err(format!("Duplicate provider protocol ID: {}", descriptor.id));
        }
        if descriptor.codec.id() != descriptor.id {
            return Err(format!(
                "Provider protocol {} uses codec ID {}",
                descriptor.id,
                descriptor.codec.id()
            ));
        }
        let url = url::Url::parse(descriptor.default_base_url).map_err(|error| {
            format!(
                "Provider protocol {} has invalid default Base URL: {error}",
                descriptor.id
            )
        })?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(format!(
                "Provider protocol {} default Base URL must use HTTP or HTTPS",
                descriptor.id
            ));
        }
        if let AuthStrategy::StaticHeader { header, .. } = descriptor.auth.strategy {
            HeaderName::from_bytes(header.as_bytes()).map_err(|error| {
                format!(
                    "Provider protocol {} has invalid authentication header: {error}",
                    descriptor.id
                )
            })?;
        }
        validate_fields(descriptor.config_fields)
            .map_err(|error| format!("Provider protocol {}: {error}", descriptor.id))?;
    }
    Ok(())
}

pub fn descriptor_for(id: &ProtocolId) -> Result<&'static ProtocolDescriptor, String> {
    resolve_input(id)
}

pub fn descriptor_by_id(id: &str) -> Result<Option<&'static ProtocolDescriptor>, String> {
    validate_registry()?;
    Ok(DESCRIPTORS.iter().find(|descriptor| descriptor.id == id))
}

pub fn resolve_persisted(raw_id: impl Into<String>) -> Result<ProtocolResolution, String> {
    let raw_id = raw_id.into();
    Ok(match descriptor_by_id(&raw_id)? {
        Some(descriptor) => ProtocolResolution::Known(descriptor),
        None => ProtocolResolution::Unknown {
            id: ProtocolId::unknown(),
            raw_id,
        },
    })
}

pub fn resolve_input(id: &ProtocolId) -> Result<&'static ProtocolDescriptor, String> {
    if id.is_unknown() {
        return Err("Unknown or unavailable provider protocol: unknown".into());
    }
    descriptor_by_id(id.as_str())?
        .ok_or_else(|| format!("Unknown or unavailable provider protocol: {}", id.as_str()))
}

pub fn descriptor_views() -> Result<Vec<ProtocolDescriptorView>, String> {
    validate_registry()?;
    DESCRIPTORS
        .iter()
        .map(|descriptor| -> Result<ProtocolDescriptorView, String> {
            Ok(ProtocolDescriptorView {
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
                    .map(|field| {
                        Ok(ConfigFieldView {
                            pointer: field.pointer.to_string(),
                            label: field.label.to_string(),
                            kind: field.kind,
                            required: field.required,
                            default_value: parse_default(field)?,
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
                    })
                    .collect::<Result<Vec<_>, String>>()?,
                help_text: descriptor.help_text.map(str::to_string),
            })
        })
        .collect::<Result<Vec<_>, String>>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::protocols::test_protocol;

    static INVALID_POINTER_FIELDS: &[ConfigField] = &[ConfigField {
        pointer: "/invalid~2pointer",
        label: "Invalid",
        kind: ConfigFieldKind::Text,
        required: false,
        default_json: None,
        options: &[],
        help_text: None,
    }];

    static INVALID_DEFAULT_FIELDS: &[ConfigField] = &[ConfigField {
        pointer: "/count",
        label: "Count",
        kind: ConfigFieldKind::Number,
        required: true,
        default_json: Some("\"not-a-number\""),
        options: &[],
        help_text: None,
    }];

    static MALFORMED_DEFAULT_FIELDS: &[ConfigField] = &[ConfigField {
        pointer: "/count",
        label: "Count",
        kind: ConfigFieldKind::Number,
        required: true,
        default_json: Some("{"),
        options: &[],
        help_text: None,
    }];

    static DUPLICATE_SELECT_OPTIONS: &[ConfigOption] = &[
        ConfigOption {
            value: "same",
            label: "First",
        },
        ConfigOption {
            value: "same",
            label: "Second",
        },
    ];

    static INVALID_SELECT_FIELDS: &[ConfigField] = &[ConfigField {
        pointer: "/mode",
        label: "Mode",
        kind: ConfigFieldKind::Select,
        required: true,
        default_json: Some("\"missing\""),
        options: TEST_OPTIONS,
        help_text: None,
    }];

    static DUPLICATE_SELECT_FIELDS: &[ConfigField] = &[ConfigField {
        pointer: "/mode",
        label: "Mode",
        kind: ConfigFieldKind::Select,
        required: true,
        default_json: Some("\"same\""),
        options: DUPLICATE_SELECT_OPTIONS,
        help_text: None,
    }];

    fn descriptor(id: &'static str, fields: &'static [ConfigField]) -> ProtocolDescriptor {
        ProtocolDescriptor {
            id,
            display_name: "Test",
            default_base_url: "https://example.test",
            wire_family: "test",
            config_kind: "generic",
            supports_model_listing: true,
            capability_profile: CapabilityProfile::Test,
            auth: AuthDescriptor {
                strategy: AuthStrategy::None,
                label: "None",
                help_text: None,
            },
            config_fields: fields,
            help_text: None,
            codec: &test_protocol::CODEC,
        }
    }

    #[test]
    fn validates_the_registered_descriptor_set() {
        validate_descriptors(DESCRIPTORS).expect("valid production and test descriptors");
        let views = descriptor_views().expect("descriptor views");
        let seventh = views
            .iter()
            .find(|view| view.id == "test-seventh")
            .expect("seventh protocol view");
        assert_eq!(
            seventh.config_fields[0].default_value,
            Some(Value::from("fast"))
        );
    }

    #[test]
    fn rejects_duplicate_reserved_and_codec_mismatched_ids() {
        assert!(validate_descriptors(&[
            descriptor("test-seventh", &[]),
            descriptor("test-seventh", &[]),
        ])
        .expect_err("duplicate ID")
        .contains("Duplicate"));
        assert!(validate_descriptors(&[descriptor("unknown", &[])])
            .expect_err("reserved ID")
            .contains("reserved"));
        assert!(validate_descriptors(&[descriptor("mismatched", &[])])
            .expect_err("codec mismatch")
            .contains("codec ID"));
    }

    #[test]
    fn rejects_invalid_schema_pointers_and_defaults() {
        assert!(
            validate_descriptors(&[descriptor("test-seventh", INVALID_POINTER_FIELDS,)])
                .expect_err("invalid pointer")
                .contains("invalid escape")
        );
        assert!(
            validate_descriptors(&[descriptor("test-seventh", INVALID_DEFAULT_FIELDS,)])
                .expect_err("invalid default")
                .contains("must be a number")
        );
        assert!(
            validate_descriptors(&[descriptor("test-seventh", MALFORMED_DEFAULT_FIELDS,)])
                .expect_err("malformed default JSON")
                .contains("invalid default JSON")
        );
        assert!(
            validate_descriptors(&[descriptor("test-seventh", INVALID_SELECT_FIELDS,)])
                .expect_err("select default outside options")
                .contains("registered option")
        );
        assert!(
            validate_descriptors(&[descriptor("test-seventh", DUPLICATE_SELECT_FIELDS,)])
                .expect_err("duplicate select option")
                .contains("duplicate option")
        );
    }

    #[test]
    fn rejects_invalid_urls_and_authentication_headers() {
        let mut invalid_url = descriptor("test-seventh", &[]);
        invalid_url.default_base_url = "file:///tmp/provider";
        assert!(validate_descriptors(&[invalid_url])
            .expect_err("non-HTTP URL")
            .contains("HTTP or HTTPS"));

        let mut invalid_header = descriptor("test-seventh", &[]);
        invalid_header.auth = AuthDescriptor {
            strategy: AuthStrategy::StaticHeader {
                header: "bad header",
                scheme: None,
            },
            label: "Bad",
            help_text: None,
        };
        assert!(validate_descriptors(&[invalid_header])
            .expect_err("invalid authentication header")
            .contains("authentication header"));
    }
}
