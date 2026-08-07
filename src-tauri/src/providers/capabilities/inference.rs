use crate::domain::{ThinkingConfig, ThinkingEffort, ThinkingMode};
use crate::features::{is_feature_supported, FeatureId};
use crate::providers::protocols::anthropic_profile::AnthropicModelProfile;

use super::{CapabilityProfile, ModelCapabilities};

#[derive(Debug, Clone, Copy)]
enum ThinkingEffortFamily {
    FixedBudget,
    Toggle,
    Level,
    OpenAi,
    DeepSeek,
    OllamaEffort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingBudgetPolicy {
    Disabled,
    Fixed(u32),
    Adaptive,
}

pub fn infer_capabilities(
    profile: CapabilityProfile,
    base_url: &str,
    model_id: &str,
) -> ModelCapabilities {
    let (reasoning, web, thinking_required, default_thinking_effort) = match profile {
        CapabilityProfile::OpenAiChat => (
            model_name_starts(
                model_id,
                &["gpt-5", "o1", "o3", "o4", "codex-mini", "gpt-oss"],
            ) || is_feature_supported(FeatureId::OpenAiReasoningObject, base_url, model_id)
                || is_feature_supported(FeatureId::OpenAiThinkingObject, base_url, model_id)
                || is_feature_supported(FeatureId::OpenAiReasoningEffort, base_url, model_id)
                || is_feature_supported(
                    FeatureId::OpenAiDeepSeekReasoningEffort,
                    base_url,
                    model_id,
                )
                || is_feature_supported(FeatureId::OpenAiEnableThinking, base_url, model_id),
            is_openai_chat_search_model(model_id),
            false,
            None,
        ),
        CapabilityProfile::OpenAiResponses => (
            model_name_starts(
                model_id,
                &["gpt-5", "o1", "o3", "o4", "codex-mini", "gpt-oss"],
            ),
            true,
            false,
            None,
        ),
        CapabilityProfile::Anthropic => {
            let model_profile = AnthropicModelProfile::for_model(model_id);
            (
                model_profile.is_known_reasoning_model(),
                is_feature_supported(FeatureId::AnthropicWebSearch, base_url, model_id),
                model_profile.thinking_required(),
                model_profile.default_thinking_effort(),
            )
        }
        CapabilityProfile::Gemini | CapabilityProfile::VertexAi => (
            model_name_starts(model_id, &["gemini-2.5", "gemini-3", "gemma-4"]),
            gemini_google_search_model(model_id),
            false,
            None,
        ),
        CapabilityProfile::Ollama => (
            model_contains(model_id, &["deepseek-r1", "qwen3", "gpt-oss", "magistral"]),
            false,
            false,
            None,
        ),
        CapabilityProfile::Test => (model_id.ends_with("-r"), false, false, None),
    };
    let efforts = supported_thinking_efforts(profile, base_url, model_id, reasoning);
    ModelCapabilities::from_inferred(
        reasoning,
        web,
        efforts,
        thinking_required,
        default_thinking_effort,
    )
    .expect("protocol capability profiles satisfy the central registry")
}

pub fn supported_thinking_efforts(
    profile: CapabilityProfile,
    base_url: &str,
    model_id: &str,
    reasoning: bool,
) -> Vec<ThinkingEffort> {
    match profile {
        CapabilityProfile::OpenAiChat => {
            let family = if is_feature_supported(
                FeatureId::OpenAiDeepSeekReasoningEffort,
                base_url,
                model_id,
            ) {
                ThinkingEffortFamily::DeepSeek
            } else if is_feature_supported(FeatureId::OpenAiEnableThinking, base_url, model_id) {
                ThinkingEffortFamily::Toggle
            } else if is_feature_supported(FeatureId::OpenAiReasoningEffort, base_url, model_id) {
                ThinkingEffortFamily::Level
            } else {
                ThinkingEffortFamily::OpenAi
            };
            thinking_efforts(reasoning, family)
        }
        CapabilityProfile::OpenAiResponses => {
            thinking_efforts(reasoning, ThinkingEffortFamily::OpenAi)
        }
        CapabilityProfile::Anthropic => {
            AnthropicModelProfile::for_model(model_id).supported_efforts(reasoning)
        }
        CapabilityProfile::Gemini | CapabilityProfile::VertexAi => {
            let family = if is_feature_supported(FeatureId::GeminiThinkingLevel, base_url, model_id)
            {
                ThinkingEffortFamily::Level
            } else {
                ThinkingEffortFamily::FixedBudget
            };
            thinking_efforts(reasoning, family)
        }
        CapabilityProfile::Ollama => {
            let family = if model_contains(model_id, &["gpt-oss"]) {
                ThinkingEffortFamily::OllamaEffort
            } else {
                ThinkingEffortFamily::Toggle
            };
            thinking_efforts(reasoning, family)
        }
        CapabilityProfile::Test => {
            if reasoning {
                vec![ThinkingEffort::None, ThinkingEffort::High]
            } else {
                vec![ThinkingEffort::None]
            }
        }
    }
}

fn thinking_efforts(reasoning: bool, family: ThinkingEffortFamily) -> Vec<ThinkingEffort> {
    if !reasoning {
        return vec![ThinkingEffort::None];
    }
    match family {
        ThinkingEffortFamily::FixedBudget => vec![
            ThinkingEffort::None,
            ThinkingEffort::Minimal,
            ThinkingEffort::Low,
            ThinkingEffort::Medium,
            ThinkingEffort::High,
        ],
        ThinkingEffortFamily::Toggle => vec![ThinkingEffort::None, ThinkingEffort::High],
        ThinkingEffortFamily::Level => vec![
            ThinkingEffort::None,
            ThinkingEffort::Minimal,
            ThinkingEffort::Low,
            ThinkingEffort::Medium,
            ThinkingEffort::High,
        ],
        ThinkingEffortFamily::OpenAi => vec![
            ThinkingEffort::None,
            ThinkingEffort::Minimal,
            ThinkingEffort::Low,
            ThinkingEffort::Medium,
            ThinkingEffort::High,
            ThinkingEffort::Xhigh,
        ],
        ThinkingEffortFamily::DeepSeek => vec![
            ThinkingEffort::None,
            ThinkingEffort::High,
            ThinkingEffort::Max,
        ],
        ThinkingEffortFamily::OllamaEffort => vec![
            ThinkingEffort::None,
            ThinkingEffort::Low,
            ThinkingEffort::Medium,
            ThinkingEffort::High,
        ],
    }
}

pub fn thinking_budget_policy(
    profile: CapabilityProfile,
    _base_url: &str,
    model_id: &str,
    effort: ThinkingEffort,
) -> Result<ThinkingBudgetPolicy, String> {
    if effort == ThinkingEffort::None {
        return Ok(ThinkingBudgetPolicy::Disabled);
    }
    match profile {
        CapabilityProfile::Anthropic => {
            let model_profile = AnthropicModelProfile::for_model(model_id);
            if model_profile.uses_manual_thinking() {
                anthropic_fixed_budget(effort).map(ThinkingBudgetPolicy::Fixed)
            } else if model_profile.uses_adaptive_thinking() {
                Ok(ThinkingBudgetPolicy::Adaptive)
            } else {
                Err(format!(
                    "Anthropic thinking is enabled for unrecognized model \"{model_id}\"."
                ))
            }
        }
        CapabilityProfile::Gemini | CapabilityProfile::VertexAi
            if model_name_starts(model_id, &["gemini-2.5"]) =>
        {
            gemini_25_fixed_budget(effort).map(ThinkingBudgetPolicy::Fixed)
        }
        CapabilityProfile::OpenAiChat
        | CapabilityProfile::OpenAiResponses
        | CapabilityProfile::Gemini
        | CapabilityProfile::VertexAi
        | CapabilityProfile::Ollama
        | CapabilityProfile::Test => Ok(ThinkingBudgetPolicy::Adaptive),
    }
}

pub fn estimated_thinking_tokens(
    thinking: Option<&ThinkingConfig>,
    visible_output_tokens: u32,
) -> u32 {
    let Some(thinking) = thinking else {
        return 0;
    };
    if thinking.mode == ThinkingMode::Disabled || thinking.effort == Some(ThinkingEffort::None) {
        0
    } else {
        thinking.budget_tokens.unwrap_or(visible_output_tokens)
    }
}

pub fn resolve_thinking(
    profile: CapabilityProfile,
    base_url: &str,
    model_id: &str,
    effort: ThinkingEffort,
) -> Result<ThinkingConfig, String> {
    let capabilities = infer_capabilities(profile, base_url, model_id);
    if !capabilities.thinking_efforts().contains(&effort) {
        return Err(format!(
            "Model \"{model_id}\" does not support thinking effort {effort:?}."
        ));
    }
    if effort == ThinkingEffort::None && capabilities.thinking_required() {
        return Err(format!(
            "Model \"{model_id}\" requires thinking and cannot use effort None."
        ));
    }

    let policy = thinking_budget_policy(profile, base_url, model_id, effort)?;
    let mut config = ThinkingConfig {
        mode: if policy == ThinkingBudgetPolicy::Disabled {
            ThinkingMode::Disabled
        } else {
            ThinkingMode::Enabled
        },
        budget_tokens: match policy {
            ThinkingBudgetPolicy::Fixed(tokens) => Some(tokens),
            ThinkingBudgetPolicy::Disabled | ThinkingBudgetPolicy::Adaptive => None,
        },
        effort: Some(effort),
        summary: None,
    };

    match profile {
        CapabilityProfile::OpenAiChat => {
            config.effort = Some(
                if is_feature_supported(
                    FeatureId::OpenAiDeepSeekReasoningEffort,
                    base_url,
                    model_id,
                ) {
                    deepseek_effort(effort)
                } else if is_feature_supported(FeatureId::OpenAiReasoningEffort, base_url, model_id)
                {
                    bounded_high_effort(effort)
                } else {
                    openai_effort(effort)
                },
            );
        }
        CapabilityProfile::OpenAiResponses => config.effort = Some(openai_effort(effort)),
        CapabilityProfile::Anthropic => {
            if policy == ThinkingBudgetPolicy::Adaptive {
                config.mode = ThinkingMode::Auto;
            }
        }
        CapabilityProfile::Gemini | CapabilityProfile::VertexAi => {
            if is_feature_supported(FeatureId::GeminiThinkingLevel, base_url, model_id) {
                config.effort = Some(bounded_high_effort(effort));
            }
        }
        CapabilityProfile::Ollama => config.effort = Some(ollama_effort(effort)),
        CapabilityProfile::Test => {}
    }
    Ok(config)
}

pub fn lower_thinking_config(
    profile: CapabilityProfile,
    base_url: &str,
    model_id: &str,
    current: Option<&ThinkingConfig>,
) -> Result<Option<ThinkingConfig>, String> {
    let Some(current_effort) = current.and_then(|thinking| thinking.effort) else {
        return Ok(None);
    };
    let capabilities = infer_capabilities(profile, base_url, model_id);
    let efforts = capabilities.thinking_efforts();
    let Some(position) = efforts.iter().position(|effort| *effort == current_effort) else {
        return Ok(None);
    };
    let Some(lower) = position.checked_sub(1).and_then(|index| efforts.get(index)) else {
        return Ok(None);
    };
    resolve_thinking(profile, base_url, model_id, *lower).map(Some)
}

fn anthropic_fixed_budget(effort: ThinkingEffort) -> Result<u32, String> {
    match effort {
        ThinkingEffort::Low => Ok(1_024),
        ThinkingEffort::Medium => Ok(16_000),
        ThinkingEffort::High => Ok(32_000),
        unsupported => Err(format!(
            "Anthropic manual thinking has no registered fixed budget for effort {unsupported:?}."
        )),
    }
}

fn gemini_25_fixed_budget(effort: ThinkingEffort) -> Result<u32, String> {
    match effort {
        ThinkingEffort::Minimal | ThinkingEffort::Low => Ok(1_024),
        ThinkingEffort::Medium => Ok(8_192),
        ThinkingEffort::High | ThinkingEffort::Xhigh | ThinkingEffort::Max => Ok(24_576),
        ThinkingEffort::None => Ok(0),
    }
}

fn openai_effort(effort: ThinkingEffort) -> ThinkingEffort {
    match effort {
        ThinkingEffort::Max => ThinkingEffort::Xhigh,
        other => other,
    }
}

fn bounded_high_effort(effort: ThinkingEffort) -> ThinkingEffort {
    match effort {
        ThinkingEffort::Xhigh | ThinkingEffort::Max => ThinkingEffort::High,
        other => other,
    }
}

fn deepseek_effort(effort: ThinkingEffort) -> ThinkingEffort {
    match effort {
        ThinkingEffort::Xhigh | ThinkingEffort::Max => ThinkingEffort::Max,
        _ => ThinkingEffort::High,
    }
}

fn ollama_effort(effort: ThinkingEffort) -> ThinkingEffort {
    match effort {
        ThinkingEffort::Minimal => ThinkingEffort::Low,
        ThinkingEffort::Xhigh | ThinkingEffort::Max => ThinkingEffort::High,
        other => other,
    }
}

fn model_name(model_id: &str) -> String {
    let model = model_id.trim().to_lowercase();
    model
        .rsplit("/models/")
        .next()
        .unwrap_or(&model)
        .trim_start_matches("models/")
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn model_name_starts(model_id: &str, patterns: &[&str]) -> bool {
    let name = model_name(model_id);
    patterns.iter().any(|pattern| name.starts_with(pattern))
}

fn model_contains(model_id: &str, patterns: &[&str]) -> bool {
    let model = model_id.to_lowercase();
    patterns.iter().any(|pattern| model.contains(pattern))
}

fn is_openai_chat_search_model(model_id: &str) -> bool {
    let name = model_name(model_id);
    matches!(
        name.as_str(),
        "gpt-5-search-api" | "gpt-4o-search-preview" | "gpt-4o-mini-search-preview"
    )
}

fn gemini_google_search_model(model_id: &str) -> bool {
    model_name_starts(model_id, &["gemini-2.0", "gemini-2.5", "gemini-3"])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_protocol_profile_produces_a_complete_capability_map() {
        for profile in [
            CapabilityProfile::OpenAiChat,
            CapabilityProfile::OpenAiResponses,
            CapabilityProfile::Anthropic,
            CapabilityProfile::Gemini,
            CapabilityProfile::VertexAi,
            CapabilityProfile::Ollama,
            CapabilityProfile::Test,
        ] {
            let capabilities = infer_capabilities(profile, "https://example.test", "test-model");
            assert_eq!(
                capabilities.iter().count(),
                super::super::CapabilityId::REGISTERED.len()
            );
        }
    }

    #[test]
    fn fixed_budget_profiles_are_registered_once() {
        for (effort, expected) in [
            (ThinkingEffort::Low, 1_024),
            (ThinkingEffort::Medium, 16_000),
            (ThinkingEffort::High, 32_000),
        ] {
            assert_eq!(
                thinking_budget_policy(
                    CapabilityProfile::Anthropic,
                    "https://api.anthropic.com",
                    "claude-sonnet-4-5",
                    effort,
                ),
                Ok(ThinkingBudgetPolicy::Fixed(expected))
            );
        }
        for (effort, expected) in [
            (ThinkingEffort::Low, 1_024),
            (ThinkingEffort::Medium, 8_192),
            (ThinkingEffort::High, 24_576),
        ] {
            assert_eq!(
                thinking_budget_policy(
                    CapabilityProfile::Gemini,
                    "https://generativelanguage.googleapis.com",
                    "gemini-2.5-pro",
                    effort,
                ),
                Ok(ThinkingBudgetPolicy::Fixed(expected))
            );
        }
    }

    #[test]
    fn dynamic_profiles_reserve_visible_output_without_inventing_a_wire_budget() {
        let dynamic = resolve_thinking(
            CapabilityProfile::OpenAiChat,
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "qwen3-32b",
            ThinkingEffort::High,
        )
        .expect("provider-managed thinking");
        assert_eq!(dynamic.budget_tokens, None);
        assert_eq!(estimated_thinking_tokens(Some(&dynamic), 7_500), 7_500);
        assert_eq!(
            supported_thinking_efforts(
                CapabilityProfile::OpenAiChat,
                "https://dashscope.aliyuncs.com/compatible-mode/v1",
                "qwen3-32b",
                true,
            ),
            vec![ThinkingEffort::None, ThinkingEffort::High]
        );
    }

    #[test]
    fn lowering_respects_required_thinking_floor() {
        let current = resolve_thinking(
            CapabilityProfile::Anthropic,
            "https://api.anthropic.com",
            "claude-fable-5",
            ThinkingEffort::Medium,
        )
        .expect("required thinking");
        let low = lower_thinking_config(
            CapabilityProfile::Anthropic,
            "https://api.anthropic.com",
            "claude-fable-5",
            Some(&current),
        )
        .expect("lower effort")
        .expect("low effort");
        assert_eq!(low.effort, Some(ThinkingEffort::Low));
        assert!(lower_thinking_config(
            CapabilityProfile::Anthropic,
            "https://api.anthropic.com",
            "claude-fable-5",
            Some(&low),
        )
        .expect("required floor")
        .is_none());
    }
}
