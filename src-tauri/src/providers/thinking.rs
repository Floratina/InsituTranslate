use crate::domain::{ThinkingConfig, ThinkingEffort, ThinkingMode};

pub fn base_config(effort: ThinkingEffort) -> ThinkingConfig {
    ThinkingConfig {
        mode: ThinkingMode::Enabled,
        budget_tokens: None,
        effort: Some(effort),
        summary: None,
    }
}

pub fn openai_effort(effort: ThinkingEffort) -> ThinkingEffort {
    match effort {
        ThinkingEffort::Max => ThinkingEffort::Xhigh,
        other => other,
    }
}

pub fn volc_effort(effort: ThinkingEffort) -> ThinkingEffort {
    match effort {
        ThinkingEffort::Xhigh | ThinkingEffort::Max => ThinkingEffort::High,
        other => other,
    }
}

pub fn deepseek_effort(effort: ThinkingEffort) -> ThinkingEffort {
    match effort {
        ThinkingEffort::Xhigh | ThinkingEffort::Max => ThinkingEffort::Max,
        _ => ThinkingEffort::High,
    }
}

pub fn gemini_level_effort(effort: ThinkingEffort) -> ThinkingEffort {
    match effort {
        ThinkingEffort::Xhigh | ThinkingEffort::Max => ThinkingEffort::High,
        other => other,
    }
}

pub fn ollama_effort(effort: ThinkingEffort) -> ThinkingEffort {
    match effort {
        ThinkingEffort::Minimal => ThinkingEffort::Low,
        ThinkingEffort::Xhigh | ThinkingEffort::Max => ThinkingEffort::High,
        other => other,
    }
}

pub fn budget_tokens(effort: ThinkingEffort) -> u32 {
    match effort {
        ThinkingEffort::None => 0,
        ThinkingEffort::Minimal | ThinkingEffort::Low => 1024,
        ThinkingEffort::Medium => 16_000,
        ThinkingEffort::High | ThinkingEffort::Xhigh | ThinkingEffort::Max => 32_000,
    }
}
