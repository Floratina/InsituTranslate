use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::ThinkingEffort;
use crate::providers::ProtocolCodec;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CapabilityId(&'static str);

impl CapabilityId {
    pub const REASONING: Self = Self("reasoning");
    pub const WEB: Self = Self("web");
    pub const THINKING_EFFORT: Self = Self("thinking-effort");

    pub const REGISTERED: [Self; 3] = [Self::REASONING, Self::WEB, Self::THINKING_EFFORT];

    pub const fn as_str(self) -> &'static str {
        self.0
    }

    pub fn from_registered(value: &str) -> Option<Self> {
        Self::REGISTERED
            .into_iter()
            .find(|capability| capability.as_str() == value)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CapabilityOverrides {
    values: HashMap<CapabilityId, Value>,
}

impl CapabilityOverrides {
    pub fn insert_json(&mut self, capability: CapabilityId, value: Value) {
        self.values.insert(capability, value);
    }

    pub fn boolean(&self, capability: CapabilityId) -> Result<Option<bool>, String> {
        self.values
            .get(&capability)
            .map(|value| {
                value.as_bool().ok_or_else(|| {
                    format!(
                        "Capability override {} must be a boolean",
                        capability.as_str()
                    )
                })
            })
            .transpose()
    }

    pub fn thinking_efforts(&self) -> Result<Option<Vec<ThinkingEffort>>, String> {
        self.values
            .get(&CapabilityId::THINKING_EFFORT)
            .map(|value| {
                serde_json::from_value(value.clone()).map_err(|error| {
                    format!("Invalid thinking-effort capability override: {error}")
                })
            })
            .transpose()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCapabilities {
    pub reasoning: bool,
    pub web: bool,
    pub thinking_efforts: Vec<ThinkingEffort>,
    pub thinking_required: bool,
    pub default_thinking_effort: Option<ThinkingEffort>,
}

pub fn infer_capabilities(
    codec: &dyn ProtocolCodec,
    base_url: &str,
    model_id: &str,
) -> ModelCapabilities {
    codec.infer_capabilities(base_url, model_id)
}

pub fn resolve_capabilities(
    codec: &dyn ProtocolCodec,
    base_url: &str,
    model_id: &str,
    overrides: &CapabilityOverrides,
) -> Result<ModelCapabilities, String> {
    let inferred = infer_capabilities(codec, base_url, model_id);
    let reasoning_override = overrides.boolean(CapabilityId::REASONING)?;
    if inferred.thinking_required && reasoning_override == Some(false) {
        return Err(format!(
            "Model \"{model_id}\" requires thinking and cannot disable reasoning capability"
        ));
    }
    let reasoning = reasoning_override.unwrap_or(inferred.reasoning);
    let web = overrides
        .boolean(CapabilityId::WEB)?
        .unwrap_or(inferred.web);
    let thinking_efforts = match overrides.thinking_efforts()? {
        Some(efforts) => efforts,
        None if reasoning == inferred.reasoning => inferred.thinking_efforts,
        None => codec.supported_thinking_efforts(base_url, model_id, reasoning),
    };
    if inferred.thinking_required && thinking_efforts.contains(&ThinkingEffort::None) {
        return Err(format!(
            "Model \"{model_id}\" requires thinking and cannot support effort None"
        ));
    }
    Ok(ModelCapabilities {
        reasoning,
        web,
        thinking_efforts,
        thinking_required: inferred.thinking_required,
        default_thinking_effort: inferred.default_thinking_effort,
    })
}
