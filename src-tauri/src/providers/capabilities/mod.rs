mod inference;

use std::collections::BTreeMap;

use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::domain::ThinkingEffort;

pub use inference::{
    estimated_thinking_tokens, infer_capabilities, lower_thinking_config, resolve_thinking,
    supported_thinking_efforts,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityValueKind {
    Boolean,
    ThinkingEfforts,
    OptionalThinkingEffort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityEditor {
    Toggle,
    #[allow(dead_code)]
    Select,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityPresentation {
    Badge,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityOverridePolicy {
    User,
    Stored,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum CapabilityValue {
    Boolean(bool),
    ThinkingEfforts(Vec<ThinkingEffort>),
    OptionalThinkingEffort(Option<ThinkingEffort>),
}

impl CapabilityValue {
    pub fn as_json(&self) -> Result<Value, String> {
        serde_json::to_value(self)
            .map_err(|error| format!("Failed to serialize capability value: {error}"))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CapabilityOption {
    pub value: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct CapabilityDefinition {
    pub value_kind: CapabilityValueKind,
    pub default_value: fn() -> CapabilityValue,
    pub override_policy: CapabilityOverridePolicy,
    pub label: &'static str,
    pub description: &'static str,
    pub icon: Option<&'static str>,
    pub editor: CapabilityEditor,
    pub presentation: CapabilityPresentation,
    pub options: &'static [CapabilityOption],
}

static THINKING_EFFORT_OPTIONS: &[CapabilityOption] = &[
    CapabilityOption {
        value: "none",
        label: "关闭",
    },
    CapabilityOption {
        value: "minimal",
        label: "最小",
    },
    CapabilityOption {
        value: "low",
        label: "低",
    },
    CapabilityOption {
        value: "medium",
        label: "中",
    },
    CapabilityOption {
        value: "high",
        label: "高",
    },
    CapabilityOption {
        value: "xhigh",
        label: "极高",
    },
    CapabilityOption {
        value: "max",
        label: "最大",
    },
];

fn default_false() -> CapabilityValue {
    CapabilityValue::Boolean(false)
}

fn default_thinking_efforts() -> CapabilityValue {
    CapabilityValue::ThinkingEfforts(vec![ThinkingEffort::None])
}

fn default_optional_thinking_effort() -> CapabilityValue {
    CapabilityValue::OptionalThinkingEffort(None)
}

macro_rules! capability_registry {
    ($(
        $constant:ident: $variant:ident => {
            id: $id:literal,
            kind: $kind:ident,
            default: $default:path,
            override_policy: $override_policy:ident,
            label: $label:literal,
            description: $description:literal,
            icon: $icon:expr,
            editor: $editor:ident,
            presentation: $presentation:ident,
            options: $options:expr
        }
    ),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum CapabilityId {
            $($variant),+
        }

        impl CapabilityId {
            $(pub const $constant: Self = Self::$variant;)+

            pub const REGISTERED: &'static [Self] = &[$(Self::$constant),+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $id),+
                }
            }

            pub fn from_registered(value: &str) -> Option<Self> {
                match value {
                    $($id => Some(Self::$variant),)+
                    _ => None,
                }
            }

            pub const fn definition(self) -> CapabilityDefinition {
                match self {
                    $(Self::$variant => CapabilityDefinition {
                        value_kind: CapabilityValueKind::$kind,
                        default_value: $default,
                        override_policy: CapabilityOverridePolicy::$override_policy,
                        label: $label,
                        description: $description,
                        icon: $icon,
                        editor: CapabilityEditor::$editor,
                        presentation: CapabilityPresentation::$presentation,
                        options: $options,
                    }),+
                }
            }
        }

        impl Serialize for CapabilityId {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for CapabilityId {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::from_registered(&value)
                    .ok_or_else(|| D::Error::custom(format!("Unknown capability ID: {value}")))
            }
        }
    };
}

capability_registry! {
    REASONING: Reasoning => {
        id: "reasoning",
        kind: Boolean,
        default: default_false,
        override_policy: User,
        label: "推理",
        description: "模型可使用推理过程处理请求",
        icon: Some("brain"),
        editor: Toggle,
        presentation: Badge,
        options: &[]
    },
    WEB: Web => {
        id: "web",
        kind: Boolean,
        default: default_false,
        override_policy: User,
        label: "联网",
        description: "模型和协议支持原生联网搜索",
        icon: Some("globe-2"),
        editor: Toggle,
        presentation: Badge,
        options: &[]
    },
    THINKING_EFFORT: ThinkingEffort => {
        id: "thinking-effort",
        kind: ThinkingEfforts,
        default: default_thinking_efforts,
        override_policy: Stored,
        label: "推理强度",
        description: "模型支持的推理强度集合",
        icon: None,
        editor: Hidden,
        presentation: Hidden,
        options: THINKING_EFFORT_OPTIONS
    },
    THINKING_REQUIRED: ThinkingRequired => {
        id: "thinking-required",
        kind: Boolean,
        default: default_false,
        override_policy: None,
        label: "必须推理",
        description: "模型不允许关闭推理",
        icon: None,
        editor: Hidden,
        presentation: Hidden,
        options: &[]
    },
    DEFAULT_THINKING_EFFORT: DefaultThinkingEffort => {
        id: "default-thinking-effort",
        kind: OptionalThinkingEffort,
        default: default_optional_thinking_effort,
        override_policy: None,
        label: "默认推理强度",
        description: "模型启用推理时采用的默认强度",
        icon: None,
        editor: Hidden,
        presentation: Hidden,
        options: THINKING_EFFORT_OPTIONS
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCapabilities {
    values: BTreeMap<CapabilityId, CapabilityValue>,
}

impl ModelCapabilities {
    pub fn from_values(values: BTreeMap<CapabilityId, CapabilityValue>) -> Result<Self, String> {
        let capabilities = Self { values };
        capabilities.validate()?;
        Ok(capabilities)
    }

    pub(crate) fn from_inferred(
        reasoning: bool,
        web: bool,
        thinking_efforts: Vec<ThinkingEffort>,
        thinking_required: bool,
        default_thinking_effort: Option<ThinkingEffort>,
    ) -> Result<Self, String> {
        let mut values = default_values();
        values.insert(CapabilityId::REASONING, CapabilityValue::Boolean(reasoning));
        values.insert(CapabilityId::WEB, CapabilityValue::Boolean(web));
        values.insert(
            CapabilityId::THINKING_EFFORT,
            CapabilityValue::ThinkingEfforts(thinking_efforts),
        );
        values.insert(
            CapabilityId::THINKING_REQUIRED,
            CapabilityValue::Boolean(thinking_required),
        );
        values.insert(
            CapabilityId::DEFAULT_THINKING_EFFORT,
            CapabilityValue::OptionalThinkingEffort(default_thinking_effort),
        );
        Self::from_values(values)
    }

    pub fn legacy(reasoning: bool, web: bool) -> Self {
        Self::from_inferred(reasoning, web, vec![ThinkingEffort::None], false, None)
            .expect("legacy capability values satisfy the central registry")
    }

    pub fn get(&self, capability: CapabilityId) -> &CapabilityValue {
        self.values
            .get(&capability)
            .expect("validated capability maps contain every registered capability")
    }

    pub fn reasoning(&self) -> bool {
        self.boolean(CapabilityId::REASONING)
    }

    pub fn web(&self) -> bool {
        self.boolean(CapabilityId::WEB)
    }

    pub fn thinking_efforts(&self) -> &[ThinkingEffort] {
        match self.get(CapabilityId::THINKING_EFFORT) {
            CapabilityValue::ThinkingEfforts(value) => value,
            _ => unreachable!("validated thinking-effort capability has the registered type"),
        }
    }

    pub fn thinking_required(&self) -> bool {
        self.boolean(CapabilityId::THINKING_REQUIRED)
    }

    pub fn default_thinking_effort(&self) -> Option<ThinkingEffort> {
        match self.get(CapabilityId::DEFAULT_THINKING_EFFORT) {
            CapabilityValue::OptionalThinkingEffort(value) => *value,
            _ => unreachable!("validated default-thinking-effort has the registered type"),
        }
    }

    #[cfg(test)]
    pub fn iter(&self) -> impl Iterator<Item = (CapabilityId, &CapabilityValue)> {
        self.values.iter().map(|(id, value)| (*id, value))
    }

    fn boolean(&self, capability: CapabilityId) -> bool {
        match self.get(capability) {
            CapabilityValue::Boolean(value) => *value,
            _ => unreachable!("validated boolean capability has the registered type"),
        }
    }

    fn validate(&self) -> Result<(), String> {
        for capability in CapabilityId::REGISTERED {
            let value = self
                .values
                .get(capability)
                .ok_or_else(|| format!("Missing registered capability: {}", capability.as_str()))?;
            validate_value_type(*capability, value)?;
        }
        if self.values.len() != CapabilityId::REGISTERED.len() {
            return Err("Capability map contains an unregistered capability".into());
        }
        if self.thinking_required() && !self.reasoning() {
            return Err("Required thinking must enable reasoning capability".into());
        }
        if self.thinking_required() && self.thinking_efforts().contains(&ThinkingEffort::None) {
            return Err("Required thinking cannot include effort None".into());
        }
        if let Some(default_effort) = self.default_thinking_effort() {
            if !self.thinking_efforts().contains(&default_effort) {
                return Err(format!(
                    "Default thinking effort {default_effort:?} is not supported"
                ));
            }
        }
        Ok(())
    }
}

impl Serialize for ModelCapabilities {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.values.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ModelCapabilities {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = BTreeMap::<CapabilityId, Value>::deserialize(deserializer)?;
        let mut values = BTreeMap::new();
        for (capability, value) in raw {
            values.insert(
                capability,
                parse_json_value(capability, value).map_err(D::Error::custom)?,
            );
        }
        Self::from_values(values).map_err(D::Error::custom)
    }
}

fn default_values() -> BTreeMap<CapabilityId, CapabilityValue> {
    CapabilityId::REGISTERED
        .iter()
        .map(|id| (*id, (id.definition().default_value)()))
        .collect()
}

fn validate_value_type(capability: CapabilityId, value: &CapabilityValue) -> Result<(), String> {
    let valid = matches!(
        (capability.definition().value_kind, value),
        (CapabilityValueKind::Boolean, CapabilityValue::Boolean(_))
            | (
                CapabilityValueKind::ThinkingEfforts,
                CapabilityValue::ThinkingEfforts(_)
            )
            | (
                CapabilityValueKind::OptionalThinkingEffort,
                CapabilityValue::OptionalThinkingEffort(_)
            )
    );
    if valid {
        Ok(())
    } else {
        Err(format!(
            "Capability {} has the wrong value type",
            capability.as_str()
        ))
    }
}

pub fn parse_json_value(capability: CapabilityId, value: Value) -> Result<CapabilityValue, String> {
    match capability.definition().value_kind {
        CapabilityValueKind::Boolean => value
            .as_bool()
            .map(CapabilityValue::Boolean)
            .ok_or_else(|| format!("Capability {} must be a boolean", capability.as_str())),
        CapabilityValueKind::ThinkingEfforts => serde_json::from_value(value)
            .map(CapabilityValue::ThinkingEfforts)
            .map_err(|error| {
                format!(
                    "Capability {} must be a valid thinking effort list: {error}",
                    capability.as_str()
                )
            }),
        CapabilityValueKind::OptionalThinkingEffort => serde_json::from_value(value)
            .map(CapabilityValue::OptionalThinkingEffort)
            .map_err(|error| {
                format!(
                    "Capability {} must be a valid optional thinking effort: {error}",
                    capability.as_str()
                )
            }),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilityOverrides {
    values: BTreeMap<CapabilityId, CapabilityValue>,
}

impl CapabilityOverrides {
    pub fn insert_json(&mut self, capability: CapabilityId, value: Value) -> Result<(), String> {
        if capability.definition().override_policy == CapabilityOverridePolicy::None {
            return Err(format!(
                "Capability {} cannot be overridden",
                capability.as_str()
            ));
        }
        self.values
            .insert(capability, parse_json_value(capability, value)?);
        Ok(())
    }

    pub fn insert_user_value(
        &mut self,
        capability: CapabilityId,
        value: Value,
    ) -> Result<(), String> {
        if capability.definition().override_policy != CapabilityOverridePolicy::User {
            return Err(format!(
                "Capability {} cannot be changed by the user",
                capability.as_str()
            ));
        }
        self.values
            .insert(capability, parse_json_value(capability, value)?);
        Ok(())
    }

    pub fn remove(&mut self, capability: CapabilityId) {
        self.values.remove(&capability);
    }

    pub fn get(&self, capability: CapabilityId) -> Option<&CapabilityValue> {
        self.values.get(&capability)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityProfile {
    OpenAiChat,
    OpenAiResponses,
    Anthropic,
    Gemini,
    VertexAi,
    Ollama,
    #[cfg_attr(not(test), allow(dead_code))]
    Test,
}

pub fn resolve_capabilities(
    profile: CapabilityProfile,
    base_url: &str,
    model_id: &str,
    overrides: &CapabilityOverrides,
) -> Result<ModelCapabilities, String> {
    let inferred = infer_capabilities(profile, base_url, model_id);
    let reasoning = match overrides.get(CapabilityId::REASONING) {
        Some(CapabilityValue::Boolean(value)) => *value,
        None => inferred.reasoning(),
        _ => unreachable!("capability overrides are type-checked on insertion"),
    };
    if inferred.thinking_required() && !reasoning {
        return Err(format!(
            "Model \"{model_id}\" requires thinking and cannot disable reasoning capability"
        ));
    }
    let web = match overrides.get(CapabilityId::WEB) {
        Some(CapabilityValue::Boolean(value)) => *value,
        None => inferred.web(),
        _ => unreachable!("capability overrides are type-checked on insertion"),
    };
    let thinking_efforts = match overrides.get(CapabilityId::THINKING_EFFORT) {
        Some(CapabilityValue::ThinkingEfforts(value))
            if value
                .iter()
                .all(|effort| inferred.thinking_efforts().contains(effort)) =>
        {
            value.clone()
        }
        Some(CapabilityValue::ThinkingEfforts(_)) => inferred.thinking_efforts().to_vec(),
        None if reasoning == inferred.reasoning() => inferred.thinking_efforts().to_vec(),
        None => supported_thinking_efforts(profile, base_url, model_id, reasoning),
        _ => unreachable!("capability overrides are type-checked on insertion"),
    };
    let default_thinking_effort = if reasoning {
        inferred.default_thinking_effort()
    } else {
        None
    };
    ModelCapabilities::from_inferred(
        reasoning,
        web,
        thinking_efforts,
        inferred.thinking_required(),
        default_thinking_effort,
    )
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityOptionView {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDescriptorView {
    pub id: String,
    pub label: String,
    pub description: String,
    pub value_kind: CapabilityValueKind,
    pub user_editable: bool,
    pub icon: Option<String>,
    pub editor: CapabilityEditor,
    pub presentation: CapabilityPresentation,
    pub options: Vec<CapabilityOptionView>,
    pub default_value: CapabilityValue,
}

pub fn descriptor_views() -> Result<Vec<CapabilityDescriptorView>, String> {
    validate_registry()?;
    Ok(CapabilityId::REGISTERED
        .iter()
        .map(|id| {
            let definition = id.definition();
            CapabilityDescriptorView {
                id: id.as_str().to_string(),
                label: definition.label.to_string(),
                description: definition.description.to_string(),
                value_kind: definition.value_kind,
                user_editable: definition.override_policy == CapabilityOverridePolicy::User,
                icon: definition.icon.map(str::to_string),
                editor: definition.editor,
                presentation: definition.presentation,
                options: definition
                    .options
                    .iter()
                    .map(|option| CapabilityOptionView {
                        value: option.value.to_string(),
                        label: option.label.to_string(),
                    })
                    .collect(),
                default_value: (definition.default_value)(),
            }
        })
        .collect())
}

pub fn validate_registry() -> Result<(), String> {
    let mut ids = std::collections::HashSet::new();
    for capability in CapabilityId::REGISTERED {
        let definition = capability.definition();
        if !ids.insert(capability.as_str()) {
            return Err(format!("Duplicate capability ID: {}", capability.as_str()));
        }
        let default_value = (definition.default_value)();
        validate_value_type(*capability, &default_value)?;
        if definition.editor != CapabilityEditor::Hidden
            && definition.override_policy != CapabilityOverridePolicy::User
        {
            return Err(format!(
                "Capability {} has an editor but is not user-overridable",
                capability.as_str()
            ));
        }
        match definition.editor {
            CapabilityEditor::Toggle if definition.value_kind != CapabilityValueKind::Boolean => {
                return Err(format!(
                    "Toggle capability {} must be boolean",
                    capability.as_str()
                ));
            }
            CapabilityEditor::Select if definition.options.is_empty() => {
                return Err(format!(
                    "Select capability {} must define options",
                    capability.as_str()
                ));
            }
            _ => {}
        }
        let mut options = std::collections::HashSet::new();
        for option in definition.options {
            if !options.insert(option.value) {
                return Err(format!(
                    "Capability {} has duplicate option {}",
                    capability.as_str(),
                    option.value
                ));
            }
            serde_json::from_value::<ThinkingEffort>(Value::String(option.value.to_string()))
                .map_err(|error| {
                    format!(
                        "Capability {} has invalid thinking effort option {}: {error}",
                        capability.as_str(),
                        option.value
                    )
                })?;
        }
        if definition.presentation == CapabilityPresentation::Badge
            && (definition.value_kind != CapabilityValueKind::Boolean || definition.icon.is_none())
        {
            return Err(format!(
                "Badge capability {} must be boolean and define an icon",
                capability.as_str()
            ));
        }
    }
    ModelCapabilities::from_values(default_values())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_defaults_and_ui_metadata_are_consistent() {
        validate_registry().expect("valid capability registry");
        assert_eq!(CapabilityId::REGISTERED.len(), 5);
        assert_eq!(
            CapabilityId::from_registered("thinking-required"),
            Some(CapabilityId::THINKING_REQUIRED)
        );
        assert!(CapabilityId::from_registered("unknown").is_none());
    }

    #[test]
    fn capability_json_is_a_generic_strict_map() {
        let capabilities = ModelCapabilities::legacy(true, false);
        let value = serde_json::to_value(&capabilities).expect("serialize capabilities");
        assert_eq!(value["reasoning"], true);
        assert_eq!(value["thinking-effort"], serde_json::json!(["none"]));
        assert!(
            serde_json::from_value::<ModelCapabilities>(serde_json::json!({
                "reasoning": true,
                "web": false,
                "thinking-effort": ["none"],
                "thinking-required": false,
                "default-thinking-effort": null
            }))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<ModelCapabilities>(serde_json::json!({
                "reasoning": "yes",
                "web": false,
                "thinking-effort": ["none"],
                "thinking-required": false,
                "default-thinking-effort": null
            }))
            .is_err()
        );
    }

    #[test]
    fn external_values_and_override_policy_are_strict() {
        let unknown = serde_json::from_value::<ModelCapabilities>(serde_json::json!({
            "reasoning": false,
            "web": false,
            "thinking-effort": ["none"],
            "thinking-required": false,
            "default-thinking-effort": null,
            "future-capability": true
        }));
        assert!(unknown.is_err());

        let mut overrides = CapabilityOverrides::default();
        assert!(overrides
            .insert_json(CapabilityId::REASONING, serde_json::json!("yes"))
            .is_err());
        assert!(overrides
            .insert_json(CapabilityId::THINKING_REQUIRED, serde_json::json!(true))
            .is_err());
        assert!(overrides
            .insert_user_value(CapabilityId::THINKING_EFFORT, serde_json::json!(["high"]),)
            .is_err());
    }

    #[test]
    fn reasoning_constraints_are_validated_centrally() {
        assert!(ModelCapabilities::from_inferred(
            false,
            false,
            vec![ThinkingEffort::High],
            true,
            Some(ThinkingEffort::High),
        )
        .is_err());
        assert!(ModelCapabilities::from_inferred(
            true,
            false,
            vec![ThinkingEffort::None, ThinkingEffort::High],
            true,
            Some(ThinkingEffort::High),
        )
        .is_err());
        assert!(ModelCapabilities::from_inferred(
            true,
            false,
            vec![ThinkingEffort::Low],
            false,
            Some(ThinkingEffort::High),
        )
        .is_err());
    }
}
