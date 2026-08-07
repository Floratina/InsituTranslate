use crate::domain::ThinkingEffort;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnthropicThinkingDialect {
    Manual,
    ManualWithEffort,
    Adaptive,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnthropicSamplingPolicy {
    Legacy,
    Haiku45,
    NoNonDefault,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AnthropicDisablePolicy {
    Allowed,
    AlwaysOn,
    OpusFive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnthropicEffortSet {
    Budget,
    BudgetWithoutHigh,
    LowThroughHigh,
    LowThroughMax,
    Full,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AnthropicModelProfile {
    pub(super) thinking_dialect: AnthropicThinkingDialect,
    pub(super) sampling_policy: AnthropicSamplingPolicy,
    pub(super) max_output_tokens: Option<u32>,
    pub(super) thinking_required: bool,
    pub(super) default_thinking_effort: Option<ThinkingEffort>,
    pub(super) disable_policy: AnthropicDisablePolicy,
    effort_set: AnthropicEffortSet,
}

impl AnthropicModelProfile {
    pub(crate) fn for_model(model_id: &str) -> Self {
        let name = normalized_model_name(model_id);

        if matches!(
            name.as_str(),
            "claude-3-7-sonnet-20250219"
                | "claude-3-7-sonnet-latest"
                | "claude-3-7-sonnet@20250219"
                | "claude-3-7-sonnet-20250219-v1:0"
        ) {
            return Self::manual(AnthropicEffortSet::Budget, Some(64_000));
        }
        if matches!(
            name.as_str(),
            "claude-sonnet-4-20250514"
                | "claude-sonnet-4-0"
                | "claude-sonnet-4@20250514"
                | "claude-sonnet-4-20250514-v1:0"
        ) {
            return Self::manual(AnthropicEffortSet::Budget, Some(64_000));
        }
        if matches!(
            name.as_str(),
            "claude-opus-4-20250514"
                | "claude-opus-4-0"
                | "claude-opus-4@20250514"
                | "claude-opus-4-20250514-v1:0"
        ) {
            return Self::manual(AnthropicEffortSet::BudgetWithoutHigh, Some(32_000));
        }
        if matches!(
            name.as_str(),
            "claude-opus-4-1"
                | "claude-opus-4-1-20250805"
                | "claude-opus-4-1@20250805"
                | "claude-opus-4-1-20250805-v1:0"
        ) {
            return Self::manual(AnthropicEffortSet::BudgetWithoutHigh, Some(32_000));
        }
        if matches!(
            name.as_str(),
            "claude-sonnet-4-5"
                | "claude-sonnet-4-5-20250929"
                | "claude-sonnet-4-5@20250929"
                | "claude-sonnet-4-5-20250929-v1:0"
        ) {
            return Self::manual(AnthropicEffortSet::Budget, Some(64_000));
        }
        if matches!(
            name.as_str(),
            "claude-haiku-4-5"
                | "claude-haiku-4-5-20251001"
                | "claude-haiku-4-5@20251001"
                | "claude-haiku-4-5-20251001-v1:0"
        ) {
            return Self {
                sampling_policy: AnthropicSamplingPolicy::Haiku45,
                ..Self::manual(AnthropicEffortSet::Budget, Some(64_000))
            };
        }
        if matches!(
            name.as_str(),
            "claude-opus-4-5"
                | "claude-opus-4-5-20251101"
                | "claude-opus-4-5@20251101"
                | "claude-opus-4-5-20251101-v1:0"
        ) {
            return Self {
                thinking_dialect: AnthropicThinkingDialect::ManualWithEffort,
                sampling_policy: AnthropicSamplingPolicy::Legacy,
                max_output_tokens: Some(64_000),
                thinking_required: false,
                default_thinking_effort: None,
                disable_policy: AnthropicDisablePolicy::Allowed,
                effort_set: AnthropicEffortSet::LowThroughHigh,
            };
        }
        if matches!(
            name.as_str(),
            "claude-sonnet-4-6" | "claude-opus-4-6" | "claude-opus-4-6-v1"
        ) {
            return Self::adaptive(
                AnthropicEffortSet::LowThroughMax,
                AnthropicSamplingPolicy::Legacy,
                None,
                AnthropicDisablePolicy::Allowed,
            );
        }
        if matches!(name.as_str(), "claude-opus-4-7" | "claude-opus-4-8") {
            return Self::adaptive(
                AnthropicEffortSet::Full,
                AnthropicSamplingPolicy::NoNonDefault,
                None,
                AnthropicDisablePolicy::Allowed,
            );
        }
        if matches!(name.as_str(), "claude-fable-5" | "claude-mythos-5") {
            return Self::adaptive(
                AnthropicEffortSet::Full,
                AnthropicSamplingPolicy::NoNonDefault,
                Some(ThinkingEffort::High),
                AnthropicDisablePolicy::AlwaysOn,
            );
        }
        if name == "claude-opus-5" {
            return Self::adaptive(
                AnthropicEffortSet::Full,
                AnthropicSamplingPolicy::NoNonDefault,
                Some(ThinkingEffort::High),
                AnthropicDisablePolicy::OpusFive,
            );
        }
        if name == "claude-sonnet-5" {
            return Self::adaptive(
                AnthropicEffortSet::Full,
                AnthropicSamplingPolicy::NoNonDefault,
                Some(ThinkingEffort::High),
                AnthropicDisablePolicy::Allowed,
            );
        }

        Self {
            thinking_dialect: AnthropicThinkingDialect::Unknown,
            sampling_policy: AnthropicSamplingPolicy::Unknown,
            max_output_tokens: None,
            thinking_required: false,
            default_thinking_effort: None,
            disable_policy: AnthropicDisablePolicy::Allowed,
            effort_set: AnthropicEffortSet::None,
        }
    }

    pub(crate) fn is_known_reasoning_model(self) -> bool {
        self.thinking_dialect != AnthropicThinkingDialect::Unknown
    }

    pub(crate) fn supported_efforts(self, reasoning: bool) -> Vec<ThinkingEffort> {
        if !reasoning {
            return vec![ThinkingEffort::None];
        }
        let mut efforts = match self.effort_set {
            AnthropicEffortSet::Budget | AnthropicEffortSet::LowThroughHigh => vec![
                ThinkingEffort::None,
                ThinkingEffort::Low,
                ThinkingEffort::Medium,
                ThinkingEffort::High,
            ],
            AnthropicEffortSet::BudgetWithoutHigh => vec![
                ThinkingEffort::None,
                ThinkingEffort::Low,
                ThinkingEffort::Medium,
            ],
            AnthropicEffortSet::LowThroughMax => vec![
                ThinkingEffort::None,
                ThinkingEffort::Low,
                ThinkingEffort::Medium,
                ThinkingEffort::High,
                ThinkingEffort::Max,
            ],
            AnthropicEffortSet::Full => vec![
                ThinkingEffort::None,
                ThinkingEffort::Low,
                ThinkingEffort::Medium,
                ThinkingEffort::High,
                ThinkingEffort::Xhigh,
                ThinkingEffort::Max,
            ],
            AnthropicEffortSet::None => vec![ThinkingEffort::None],
        };
        if self.thinking_required {
            efforts.retain(|effort| *effort != ThinkingEffort::None);
        }
        efforts
    }

    pub(super) fn supports_effort(self, effort: ThinkingEffort) -> bool {
        self.supported_efforts(true).contains(&effort)
    }

    pub(crate) fn thinking_required(self) -> bool {
        self.thinking_required
    }

    pub(crate) fn uses_manual_thinking(self) -> bool {
        matches!(
            self.thinking_dialect,
            AnthropicThinkingDialect::Manual | AnthropicThinkingDialect::ManualWithEffort
        )
    }

    pub(crate) fn uses_adaptive_thinking(self) -> bool {
        self.thinking_dialect == AnthropicThinkingDialect::Adaptive
    }

    pub(crate) fn default_thinking_effort(self) -> Option<ThinkingEffort> {
        self.default_thinking_effort
    }

    fn manual(effort_set: AnthropicEffortSet, max_output_tokens: Option<u32>) -> Self {
        Self {
            thinking_dialect: AnthropicThinkingDialect::Manual,
            sampling_policy: AnthropicSamplingPolicy::Legacy,
            max_output_tokens,
            thinking_required: false,
            default_thinking_effort: None,
            disable_policy: AnthropicDisablePolicy::Allowed,
            effort_set,
        }
    }

    fn adaptive(
        effort_set: AnthropicEffortSet,
        sampling_policy: AnthropicSamplingPolicy,
        default_thinking_effort: Option<ThinkingEffort>,
        disable_policy: AnthropicDisablePolicy,
    ) -> Self {
        Self {
            thinking_dialect: AnthropicThinkingDialect::Adaptive,
            sampling_policy,
            max_output_tokens: Some(128_000),
            thinking_required: disable_policy == AnthropicDisablePolicy::AlwaysOn,
            default_thinking_effort,
            disable_policy,
            effort_set,
        }
    }
}

fn normalized_model_name(model_id: &str) -> String {
    let mut name = model_id
        .trim()
        .to_ascii_lowercase()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();

    for prefix in [
        "anthropic.",
        "us.anthropic.",
        "eu.anthropic.",
        "apac.anthropic.",
        "global.anthropic.",
    ] {
        if let Some(model_name) = name.strip_prefix(prefix) {
            name = model_name.to_string();
            break;
        }
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_family_matching_does_not_guess_future_models() {
        assert_eq!(
            AnthropicModelProfile::for_model("claude-opus-4-20250514").thinking_dialect,
            AnthropicThinkingDialect::Manual
        );
        assert_eq!(
            AnthropicModelProfile::for_model("claude-opus-4-7").thinking_dialect,
            AnthropicThinkingDialect::Adaptive
        );
        assert_eq!(
            AnthropicModelProfile::for_model("claude-opus-4-9").thinking_dialect,
            AnthropicThinkingDialect::Unknown
        );
        assert_eq!(
            AnthropicModelProfile::for_model("claude-sonnet-5-beta").thinking_dialect,
            AnthropicThinkingDialect::Unknown
        );
        for near_miss in [
            "claude-3-7-sonnet",
            "claude-3-7-sonnet-99999999",
            "claude-sonnet-4",
            "claude-sonnet-4-latest",
            "claude-opus-4",
            "claude-opus-4-latest",
            "claude-opus-4-1-latest",
            "claude-opus-4-5-99999999",
            "claude-opus-4-6-latest",
            "claude-opus-4-7-20260701",
            "claude-sonnet-5-latest",
            "claude-sonnet-5-20260701",
            "claude-sonnet-5@20260701",
            "claude-sonnet-5-v1:0",
        ] {
            assert_eq!(
                AnthropicModelProfile::for_model(near_miss).thinking_dialect,
                AnthropicThinkingDialect::Unknown,
                "{near_miss} must not be inferred as an official model ID"
            );
        }
    }

    #[test]
    fn provider_and_cloud_model_ids_are_normalized() {
        assert_eq!(
            AnthropicModelProfile::for_model("anthropic/claude-sonnet-4-5-20250929")
                .thinking_dialect,
            AnthropicThinkingDialect::Manual
        );
        assert_eq!(
            AnthropicModelProfile::for_model("claude-sonnet-4-5@20250929").thinking_dialect,
            AnthropicThinkingDialect::Manual
        );
        assert_eq!(
            AnthropicModelProfile::for_model("us.anthropic.claude-haiku-4-5-20251001-v1:0")
                .sampling_policy,
            AnthropicSamplingPolicy::Haiku45
        );
        assert_eq!(
            AnthropicModelProfile::for_model("anthropic.claude-opus-4-6-v1").thinking_dialect,
            AnthropicThinkingDialect::Adaptive
        );
    }

    #[test]
    fn official_legacy_ids_aliases_and_output_limits_are_exact() {
        for model in [
            "claude-3-7-sonnet-20250219",
            "claude-3-7-sonnet-latest",
            "claude-sonnet-4-20250514",
            "claude-sonnet-4-0",
        ] {
            assert_eq!(
                AnthropicModelProfile::for_model(model).max_output_tokens,
                Some(64_000),
                "{model}"
            );
        }
        for model in [
            "claude-opus-4-20250514",
            "claude-opus-4-0",
            "claude-opus-4-1-20250805",
            "claude-opus-4-1",
        ] {
            assert_eq!(
                AnthropicModelProfile::for_model(model).max_output_tokens,
                Some(32_000),
                "{model}"
            );
            assert!(!AnthropicModelProfile::for_model(model).supports_effort(ThinkingEffort::High));
        }
    }

    #[test]
    fn effort_sets_follow_model_generation() {
        let opus_41 = AnthropicModelProfile::for_model("claude-opus-4-1-20250805");
        assert_eq!(
            opus_41.supported_efforts(true),
            vec![
                ThinkingEffort::None,
                ThinkingEffort::Low,
                ThinkingEffort::Medium,
            ]
        );

        let sonnet_46 = AnthropicModelProfile::for_model("claude-sonnet-4-6");
        assert!(sonnet_46.supports_effort(ThinkingEffort::Max));
        assert!(!sonnet_46.supports_effort(ThinkingEffort::Xhigh));

        let opus_47 = AnthropicModelProfile::for_model("claude-opus-4-7");
        assert!(opus_47.supports_effort(ThinkingEffort::Xhigh));
        assert!(opus_47.supports_effort(ThinkingEffort::Max));
        assert!(!opus_47.supports_effort(ThinkingEffort::Minimal));
    }

    #[test]
    fn claude_five_profiles_are_exact_and_preserve_disable_policy() {
        for model in [
            "claude-fable-5",
            "claude-mythos-5",
            "claude-opus-5",
            "claude-sonnet-5",
        ] {
            let profile = AnthropicModelProfile::for_model(model);
            assert_eq!(profile.thinking_dialect, AnthropicThinkingDialect::Adaptive);
            assert_eq!(
                profile.sampling_policy,
                AnthropicSamplingPolicy::NoNonDefault
            );
            assert_eq!(profile.max_output_tokens, Some(128_000));
            assert_eq!(profile.default_thinking_effort, Some(ThinkingEffort::High));
            assert!(profile.supports_effort(ThinkingEffort::Xhigh));
            assert!(profile.supports_effort(ThinkingEffort::Max));
            assert!(!profile.supports_effort(ThinkingEffort::Minimal));
        }

        assert_eq!(
            AnthropicModelProfile::for_model("claude-fable-5").disable_policy,
            AnthropicDisablePolicy::AlwaysOn
        );
        assert_eq!(
            AnthropicModelProfile::for_model("claude-mythos-5").disable_policy,
            AnthropicDisablePolicy::AlwaysOn
        );
        assert_eq!(
            AnthropicModelProfile::for_model("claude-opus-5").disable_policy,
            AnthropicDisablePolicy::OpusFive
        );
        assert_eq!(
            AnthropicModelProfile::for_model("claude-sonnet-5").disable_policy,
            AnthropicDisablePolicy::Allowed
        );
    }
}
