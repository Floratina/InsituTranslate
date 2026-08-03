use serde_json::Value;

use crate::domain::{ThinkingConfig, ThinkingEffort, ThinkingMode};

#[derive(Debug, Clone, Copy)]
pub struct CompletionBudgetAlias {
    pointer: &'static str,
    segments: &'static [&'static str],
}

impl CompletionBudgetAlias {
    pub const fn new(pointer: &'static str, segments: &'static [&'static str]) -> Self {
        Self { pointer, segments }
    }
}

pub const ALL_COMPLETION_BUDGET_ALIASES: &[CompletionBudgetAlias] = &[
    CompletionBudgetAlias::new("/max_tokens", &["max_tokens"]),
    CompletionBudgetAlias::new("/max_completion_tokens", &["max_completion_tokens"]),
    CompletionBudgetAlias::new("/max_output_tokens", &["max_output_tokens"]),
    CompletionBudgetAlias::new(
        "/generationConfig/maxOutputTokens",
        &["generationConfig", "maxOutputTokens"],
    ),
    CompletionBudgetAlias::new("/options/num_predict", &["options", "num_predict"]),
];
pub const OPENAI_CHAT_ALIASES: &[CompletionBudgetAlias] = ALL_COMPLETION_BUDGET_ALIASES;
pub const OPENAI_RESPONSES_ALIASES: &[CompletionBudgetAlias] = ALL_COMPLETION_BUDGET_ALIASES;
pub const ANTHROPIC_ALIASES: &[CompletionBudgetAlias] = ALL_COMPLETION_BUDGET_ALIASES;
pub const GEMINI_ALIASES: &[CompletionBudgetAlias] = ALL_COMPLETION_BUDGET_ALIASES;
pub const OLLAMA_ALIASES: &[CompletionBudgetAlias] = ALL_COMPLETION_BUDGET_ALIASES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionLimitSource {
    Structured,
    LegacyAlias,
    Automatic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionLimitScope {
    TotalOutput,
    VisibleOutput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionBudget {
    pub visible_output_tokens: u32,
    pub thinking_tokens: u32,
    pub total_output_tokens: u32,
    pub wire_max_output_tokens: Option<u32>,
    pub limit_source: CompletionLimitSource,
    pub custom_parameters: Value,
}

#[derive(Debug, Clone)]
pub struct NormalizedCompletionBudget {
    structured_max_output_tokens: Option<u32>,
    alias_max_output_tokens: Option<u32>,
    single_alias_pointer: Option<&'static str>,
    custom_parameters: Value,
}

impl NormalizedCompletionBudget {
    pub fn resolved_max_output_tokens(&self, automatic: Option<u32>) -> Option<u32> {
        self.structured_max_output_tokens
            .or(self.alias_max_output_tokens)
            .or(automatic)
    }

    pub fn custom_parameters(&self) -> &Value {
        &self.custom_parameters
    }

    pub fn explicit_max_output_tokens(&self) -> Option<u32> {
        self.structured_max_output_tokens
            .or(self.alias_max_output_tokens)
    }

    pub fn source(&self) -> Option<CompletionLimitSource> {
        if self.structured_max_output_tokens.is_some() {
            Some(CompletionLimitSource::Structured)
        } else if self.alias_max_output_tokens.is_some() {
            Some(CompletionLimitSource::LegacyAlias)
        } else {
            None
        }
    }

    pub fn legacy_alias_pointer(&self) -> Option<&'static str> {
        if self.structured_max_output_tokens.is_some() {
            None
        } else {
            self.single_alias_pointer
        }
    }

    pub fn into_custom_parameters(self) -> Value {
        self.custom_parameters
    }
}

pub fn thinking_token_reserve(thinking: Option<&ThinkingConfig>) -> u32 {
    let Some(thinking) = thinking else {
        return 0;
    };
    if thinking.mode == ThinkingMode::Disabled || thinking.effort == Some(ThinkingEffort::None) {
        return 0;
    }
    if let Some(tokens) = thinking.budget_tokens {
        return tokens;
    }
    match thinking.effort {
        None | Some(ThinkingEffort::None) => 0,
        Some(ThinkingEffort::Minimal | ThinkingEffort::Low) => 1_024,
        Some(ThinkingEffort::Medium) => 16_000,
        Some(ThinkingEffort::High | ThinkingEffort::Xhigh | ThinkingEffort::Max) => 32_000,
    }
}

pub fn completion_budget(
    normalized: NormalizedCompletionBudget,
    visible_output_tokens: u32,
    thinking_tokens: u32,
    automatic_wire_max: Option<u32>,
    limit_scope: CompletionLimitScope,
    protocol: &str,
    model: &str,
) -> Result<CompletionBudget, String> {
    let explicit = normalized.explicit_max_output_tokens();
    let wire_max_output_tokens = explicit.or(automatic_wire_max);
    let source = normalized
        .source()
        .unwrap_or(CompletionLimitSource::Automatic);
    let required_total = visible_output_tokens
        .checked_add(thinking_tokens)
        .ok_or_else(|| {
            format!("{protocol} completion budget overflows u32 for model \"{model}\".")
        })?;

    if let Some(limit) = explicit {
        let required = match limit_scope {
            CompletionLimitScope::TotalOutput => required_total,
            CompletionLimitScope::VisibleOutput => visible_output_tokens,
        };
        if limit < required {
            return Err(format!(
                "{protocol} output limit for model \"{model}\" is {limit}, but the planned visible output ({visible_output_tokens}) and thinking reserve ({thinking_tokens}) require at least {required}."
            ));
        }
    }

    let total_output_tokens = match (wire_max_output_tokens, limit_scope) {
        (Some(limit), CompletionLimitScope::VisibleOutput) => {
            limit.checked_add(thinking_tokens).ok_or_else(|| {
                format!("{protocol} completion budget overflows u32 for model \"{model}\".")
            })?
        }
        (Some(limit), CompletionLimitScope::TotalOutput) => limit,
        (None, _) => required_total,
    };

    Ok(CompletionBudget {
        visible_output_tokens,
        thinking_tokens,
        total_output_tokens,
        wire_max_output_tokens,
        limit_source: source,
        custom_parameters: normalized.into_custom_parameters(),
    })
}

pub fn normalize_completion_budget(
    structured_max_output_tokens: Option<u32>,
    custom_parameters: &Value,
    aliases: &[CompletionBudgetAlias],
) -> Result<NormalizedCompletionBudget, String> {
    if structured_max_output_tokens == Some(0) {
        return Err("Structured max_output_tokens must be a positive integer.".into());
    }
    if !custom_parameters.is_null() && !custom_parameters.is_object() {
        return Err("Custom request body parameters must be a JSON object".into());
    }

    let mut alias_max_output_tokens = None;
    let mut first_alias = None;
    let mut alias_count = 0_u8;
    let mut single_alias_pointer = None;
    for alias in aliases {
        let Some(value) = custom_parameters.pointer(alias.pointer) else {
            continue;
        };
        alias_count = alias_count.saturating_add(1);
        single_alias_pointer = if alias_count == 1 {
            Some(alias.pointer)
        } else {
            None
        };
        let tokens = parse_alias_value(alias.pointer, value)?;
        if let Some(previous) = alias_max_output_tokens {
            if previous != tokens {
                return Err(format!(
                    "Completion budget aliases {} and {} must have the same value; received {previous} and {tokens}.",
                    first_alias.unwrap_or("<unknown>"),
                    alias.pointer
                ));
            }
        } else {
            alias_max_output_tokens = Some(tokens);
            first_alias = Some(alias.pointer);
        }
    }

    let mut sanitized = custom_parameters.clone();
    for alias in aliases {
        remove_path(&mut sanitized, alias.segments);
    }

    Ok(NormalizedCompletionBudget {
        structured_max_output_tokens,
        alias_max_output_tokens,
        single_alias_pointer,
        custom_parameters: sanitized,
    })
}

fn parse_alias_value(pointer: &str, value: &Value) -> Result<u32, String> {
    let raw = value
        .as_u64()
        .ok_or_else(|| format!("Completion budget alias {pointer} must be a positive integer."))?;
    let tokens = u32::try_from(raw).map_err(|_| {
        format!("Completion budget alias {pointer} exceeds the supported integer range.")
    })?;
    if tokens == 0 {
        return Err(format!(
            "Completion budget alias {pointer} must be a positive integer."
        ));
    }
    Ok(tokens)
}

fn remove_path(value: &mut Value, segments: &[&str]) {
    let Some((segment, remainder)) = segments.split_first() else {
        return;
    };
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if remainder.is_empty() {
        object.remove(*segment);
    } else if let Some(child) = object.get_mut(*segment) {
        remove_path(child, remainder);
        if child.as_object().is_some_and(|object| object.is_empty()) {
            object.remove(*segment);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{UnifiedChatRequest, UnifiedContent, UnifiedMessage};
    use crate::providers::codec::ProtocolCodec;
    use crate::providers::protocols::{
        anthropic, gemini, ollama, openai_chat, openai_responses, vertex_ai,
    };
    use serde_json::json;

    fn request(custom_parameters: Value) -> UnifiedChatRequest {
        UnifiedChatRequest {
            model: "compat-model".into(),
            messages: vec![UnifiedMessage {
                role: "user".into(),
                content: vec![UnifiedContent::Text {
                    text: "Translate this".into(),
                }],
            }],
            web_search: false,
            thinking: None,
            max_output_tokens: None,
            temperature: None,
            top_p: None,
            stream: false,
            logprobs: false,
            custom_parameters,
        }
    }

    #[test]
    fn structured_value_wins_and_all_root_aliases_are_removed() {
        let budget = normalize_completion_budget(
            Some(4096),
            &json!({
                "max_tokens": 8192,
                "max_completion_tokens": 8192,
                "max_output_tokens": 8192,
                "generationConfig": {"maxOutputTokens": 8192, "candidateCount": 1},
                "options": {"num_predict": 8192, "num_ctx": 32768},
                "response_format": {"type": "json_object"}
            }),
            OPENAI_CHAT_ALIASES,
        )
        .expect("valid budget");

        assert_eq!(budget.resolved_max_output_tokens(Some(16_000)), Some(4096));
        assert_eq!(
            budget.custom_parameters(),
            &json!({
                "generationConfig": {"candidateCount": 1},
                "options": {"num_ctx": 32768},
                "response_format": {"type": "json_object"}
            })
        );
    }

    #[test]
    fn aliases_win_over_automatic_and_nested_unknown_keys_are_preserved() {
        let budget = normalize_completion_budget(
            None,
            &json!({
                "generationConfig": {"maxOutputTokens": 8192, "candidateCount": 1},
                "safetySettings": []
            }),
            GEMINI_ALIASES,
        )
        .expect("valid nested budget");

        assert_eq!(budget.resolved_max_output_tokens(Some(16_000)), Some(8192));
        assert_eq!(
            budget.custom_parameters(),
            &json!({
                "generationConfig": {"candidateCount": 1},
                "safetySettings": []
            })
        );
    }

    #[test]
    fn aliases_must_be_positive_u32_values_and_agree() {
        for value in [
            json!(0),
            json!(-1),
            json!(1.5),
            json!("8192"),
            json!(u64::from(u32::MAX) + 1),
        ] {
            assert!(normalize_completion_budget(
                None,
                &json!({"max_tokens": value}),
                OPENAI_CHAT_ALIASES,
            )
            .is_err());
        }

        let error = normalize_completion_budget(
            None,
            &json!({
                "max_tokens": 4096,
                "generationConfig": {"maxOutputTokens": 8192}
            }),
            OPENAI_CHAT_ALIASES,
        )
        .expect_err("conflicting aliases");
        assert!(error.contains("must have the same value"));
    }

    #[test]
    fn rejects_zero_structured_budget_and_non_object_custom_parameters() {
        let zero = normalize_completion_budget(Some(0), &json!({}), OPENAI_CHAT_ALIASES)
            .expect_err("zero structured budget");
        assert!(zero.contains("positive integer"));

        let array = normalize_completion_budget(None, &json!([]), OPENAI_CHAT_ALIASES)
            .expect_err("non-object custom parameters");
        assert!(array.contains("JSON object"));
    }

    #[test]
    fn completion_limit_scope_controls_whether_thinking_is_inside_the_wire_cap() {
        let total_scope = completion_budget(
            normalize_completion_budget(Some(12_000), &json!({}), OPENAI_CHAT_ALIASES)
                .expect("total-scope normalized budget"),
            4_000,
            8_000,
            None,
            CompletionLimitScope::TotalOutput,
            "openai-chat",
            "reasoning-model",
        )
        .expect("total-scope budget");
        assert_eq!(total_scope.wire_max_output_tokens, Some(12_000));
        assert_eq!(total_scope.total_output_tokens, 12_000);

        let visible_scope = completion_budget(
            normalize_completion_budget(Some(4_000), &json!({}), GEMINI_ALIASES)
                .expect("visible-scope normalized budget"),
            4_000,
            8_000,
            None,
            CompletionLimitScope::VisibleOutput,
            "gemini",
            "thinking-model",
        )
        .expect("visible-scope budget");
        assert_eq!(visible_scope.wire_max_output_tokens, Some(4_000));
        assert_eq!(visible_scope.total_output_tokens, 12_000);

        let error = completion_budget(
            normalize_completion_budget(Some(11_999), &json!({}), OPENAI_CHAT_ALIASES)
                .expect("insufficient normalized budget"),
            4_000,
            8_000,
            None,
            CompletionLimitScope::TotalOutput,
            "openai-chat",
            "reasoning-model",
        )
        .expect_err("total-scope cap must include thinking reserve");
        assert!(error.contains("require at least 12000"));
    }

    #[test]
    fn a_single_legacy_alias_keeps_its_wire_hint_unless_structured_value_wins() {
        let legacy =
            normalize_completion_budget(None, &json!({"max_tokens": 8192}), OPENAI_CHAT_ALIASES)
                .expect("single legacy alias");
        assert_eq!(legacy.legacy_alias_pointer(), Some("/max_tokens"));

        let ambiguous = normalize_completion_budget(
            None,
            &json!({"max_tokens": 8192, "max_completion_tokens": 8192}),
            OPENAI_CHAT_ALIASES,
        )
        .expect("equivalent aliases");
        assert_eq!(ambiguous.legacy_alias_pointer(), None);

        let structured = normalize_completion_budget(
            Some(4096),
            &json!({"max_tokens": 8192}),
            OPENAI_CHAT_ALIASES,
        )
        .expect("structured precedence");
        assert_eq!(structured.legacy_alias_pointer(), None);
    }

    #[test]
    fn codecs_promote_legacy_aliases_to_one_protocol_wire_field() {
        let mut openai_chat_request = request(json!({
            "max_tokens": 8192,
            "max_completion_tokens": 8192,
            "seed": 7
        }));
        openai_chat_request.max_output_tokens = Some(4096);
        let openai_chat =
            openai_chat::build_body("https://compatible.example/v1", &openai_chat_request)
                .expect("OpenAI Chat body");
        assert_eq!(openai_chat["max_tokens"], 4096);
        assert!(openai_chat.get("max_completion_tokens").is_none());
        assert_eq!(openai_chat["seed"], 7);

        let responses = openai_responses::build_body(
            "https://api.openai.com/v1",
            &request(json!({"max_output_tokens": 7000, "store": false})),
        )
        .expect("OpenAI Responses body");
        assert_eq!(responses["max_output_tokens"], 7000);
        assert_eq!(responses["store"], false);

        let gemini = gemini::build_body(
            "https://generativelanguage.googleapis.com",
            &request(json!({
                "generationConfig": {"maxOutputTokens": 6000, "candidateCount": 1}
            })),
        )
        .expect("Gemini body");
        assert_eq!(
            gemini.pointer("/generationConfig/maxOutputTokens"),
            Some(&json!(6000))
        );
        assert_eq!(
            gemini.pointer("/generationConfig/candidateCount"),
            Some(&json!(1))
        );

        let ollama = ollama::build_body(&request(json!({
            "options": {"num_predict": 5000, "num_ctx": 32768},
            "keep_alive": "5m"
        })))
        .expect("Ollama body");
        assert_eq!(ollama.pointer("/options/num_predict"), Some(&json!(5000)));
        assert_eq!(ollama.pointer("/options/num_ctx"), Some(&json!(32768)));
        assert_eq!(ollama["keep_alive"], "5m");

        let anthropic = anthropic::build_body(&request(json!({
            "max_tokens": 8192,
            "service_tier": "auto"
        })))
        .expect("Anthropic body");
        assert_eq!(anthropic["max_tokens"], 8192);
        assert_eq!(anthropic["service_tier"], "auto");
    }

    #[test]
    fn non_anthropic_codecs_omit_wire_cap_without_an_explicit_budget() {
        let request = request(json!({"seed": 7}));
        let openai_chat = openai_chat::build_body("https://compatible.example/v1", &request)
            .expect("OpenAI Chat body");
        assert!(openai_chat.get("max_tokens").is_none());
        assert!(openai_chat.get("max_completion_tokens").is_none());

        let responses = openai_responses::build_body("https://api.openai.com/v1", &request)
            .expect("OpenAI Responses body");
        assert!(responses.get("max_output_tokens").is_none());

        let gemini = gemini::build_body("https://generativelanguage.googleapis.com", &request)
            .expect("Gemini body");
        assert!(gemini
            .pointer("/generationConfig/maxOutputTokens")
            .is_none());

        let ollama = ollama::build_body(&request).expect("Ollama body");
        assert!(ollama.pointer("/options/num_predict").is_none());
    }

    #[test]
    fn vertex_preflight_uses_the_gemini_nested_alias() {
        let planned = vertex_ai::CODEC
            .plan_max_output_tokens(
                "https://aiplatform.googleapis.com",
                "gemini-2.5-pro",
                None,
                None,
                &json!({"generationConfig": {"maxOutputTokens": 8192}}),
                16_000,
            )
            .expect("Vertex completion budget");
        assert_eq!(planned, Some(8192));
    }
}
