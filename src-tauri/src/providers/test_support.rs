use reqwest::Client;
use serde_json::{json, Value};

use crate::domain::{
    ProtocolId, ProviderRuntimeConfig, ThinkingConfig, ThinkingEffort, ThinkingMode,
    ThinkingSummary, UnifiedChatRequest, UnifiedContent, UnifiedMessage,
};
use crate::providers::protocols::{anthropic, gemini, ollama, openai_chat, openai_responses};
use crate::providers::RuntimeAdapter;

pub const SYSTEM_TEXT: &str = "Always translate formally.";
pub const USER_TEXT: &str = "Hello.";

pub fn request() -> UnifiedChatRequest {
    UnifiedChatRequest {
        model: "deepseek-v4".into(),
        messages: vec![UnifiedMessage {
            role: "user".into(),
            content: vec![UnifiedContent::Text {
                text: "hello".into(),
            }],
        }],
        web_search: false,
        thinking: Some(ThinkingConfig {
            mode: ThinkingMode::Enabled,
            budget_tokens: Some(2048),
            effort: Some(ThinkingEffort::Max),
            summary: Some(ThinkingSummary::Concise),
        }),
        max_output_tokens: Some(4096),
        temperature: None,
        top_p: None,
        stream: true,
        logprobs: false,
        custom_parameters: json!({}),
    }
}

pub fn prompt_request() -> UnifiedChatRequest {
    UnifiedChatRequest {
        model: "stable-model".into(),
        messages: vec![
            UnifiedMessage {
                role: "system".into(),
                content: vec![UnifiedContent::Text {
                    text: SYSTEM_TEXT.into(),
                }],
            },
            UnifiedMessage {
                role: "user".into(),
                content: vec![UnifiedContent::Text {
                    text: USER_TEXT.into(),
                }],
            },
        ],
        web_search: false,
        thinking: None,
        max_output_tokens: None,
        temperature: Some(0.0),
        top_p: None,
        stream: false,
        logprobs: false,
        custom_parameters: json!({}),
    }
}

pub fn adapter(protocol: &str, base_url: &str) -> RuntimeAdapter {
    RuntimeAdapter::new(
        Client::new(),
        ProviderRuntimeConfig {
            protocol: ProtocolId::registered(protocol),
            base_url: base_url.into(),
            use_raw_base_url: false,
            config: if protocol == "vertex-ai" {
                json!({
                    "vertexAi": {
                        "projectId": "project-1",
                        "location": "global",
                        "clientEmail": "svc@project-1.iam.gserviceaccount.com"
                    }
                })
            } else {
                json!({})
            },
            credential: (protocol == "vertex-ai").then(|| "private-key".into()),
            custom_headers: Vec::new(),
        },
    )
}

pub fn protected_custom_parameters() -> Value {
    json!({
        "model": "custom-model",
        "messages": [{"role": "user", "content": "custom messages"}],
        "message": {"role": "user", "content": "custom message"},
        "input": "custom input",
        "instructions": "custom instructions",
        "contents": [{"role": "user", "parts": [{"text": "custom contents"}]}],
        "system": "custom system",
        "systemInstruction": {"parts": [{"text": "custom system instruction"}]},
        "system_instruction": "custom system instruction",
        "system_prompt": "custom system prompt",
        "systemPrompt": "custom system prompt",
        "prompt": "custom prompt",
        "tools": [{"type": "function", "function": {"name": "custom_tool"}}],
        "insituTools": {"tools": [{"name": "lookup"}]},
        "tool_choice": "required",
        "toolChoice": {"functionCallingConfig": {"mode": "ANY"}},
        "toolConfig": {"functionCallingConfig": {"mode": "ANY"}},
        "tool_config": {"functionCallingConfig": {"mode": "ANY"}},
        "stream": true,
        "stream_options": {"include_usage": false},
        "streamOptions": {"includeUsage": false}
    })
}

pub fn build_openai_chat_body(base_url: &str, request: &UnifiedChatRequest) -> Value {
    openai_chat::build_body(base_url, request).expect("valid OpenAI Chat request")
}

pub fn build_openai_responses_body(base_url: &str, request: &UnifiedChatRequest) -> Value {
    openai_responses::build_body(base_url, request).expect("valid OpenAI Responses request")
}

pub fn build_anthropic_body(request: &UnifiedChatRequest) -> Value {
    anthropic::build_body(request).expect("valid Anthropic request")
}

pub fn build_gemini_body(request: &UnifiedChatRequest) -> Value {
    gemini::build_body("", request).expect("valid Gemini request")
}

pub fn build_ollama_body(request: &UnifiedChatRequest) -> Value {
    ollama::build_body(request).expect("valid Ollama request")
}
