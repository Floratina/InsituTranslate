#[cfg(test)]
#[allow(unused_imports)]
pub use crate::providers::runtime::{
    finish_reason_is_truncation, ProviderAdapter, ProviderChatError, ProviderChatErrorKind,
    ProviderChatMeta, RateLimitTelemetry, RuntimeAdapter,
};

#[cfg(test)]
use crate::domain::UnifiedChatRequest;
#[cfg(test)]
use serde_json::Value;

#[cfg(test)]
pub fn build_openai_chat_body(base_url: &str, request: &UnifiedChatRequest) -> Value {
    crate::providers::protocols::openai_chat::build_body(base_url, request)
        .expect("valid OpenAI Chat request")
}

#[cfg(test)]
pub fn build_openai_responses_body(base_url: &str, request: &UnifiedChatRequest) -> Value {
    crate::providers::protocols::openai_responses::build_body(base_url, request)
        .expect("valid OpenAI Responses request")
}

#[cfg(test)]
pub fn build_anthropic_body(request: &UnifiedChatRequest) -> Value {
    crate::providers::protocols::anthropic::build_body(request).expect("valid Anthropic request")
}

#[cfg(test)]
pub fn build_gemini_body(request: &UnifiedChatRequest) -> Value {
    crate::providers::protocols::gemini::build_body("", request).expect("valid Gemini request")
}

#[cfg(test)]
pub fn build_ollama_body(request: &UnifiedChatRequest) -> Value {
    crate::providers::protocols::ollama::build_body(request).expect("valid Ollama request")
}

#[cfg(test)]
#[allow(dead_code)]
pub fn ensure_anthropic_alternating_roles(messages: Vec<Value>) -> Result<Vec<Value>, String> {
    crate::providers::protocols::anthropic::ensure_alternating_roles(messages)
}
