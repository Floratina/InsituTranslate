use reqwest::Client;
use serde_json::{json, Value};

use crate::domain::{
    ProtocolId, ProviderRuntimeConfig, ThinkingConfig, ThinkingEffort, ThinkingMode,
    UnifiedContent, UnifiedMessage,
};
use crate::providers::registry::descriptor_by_id;
use crate::providers::runtime::{ProviderChatError, ProviderChatErrorKind};
use crate::providers::test_support::{
    adapter, build_anthropic_body, build_gemini_body, build_openai_chat_body,
    build_openai_responses_body, prompt_request, protected_custom_parameters, request, SYSTEM_TEXT,
    USER_TEXT,
};
use crate::providers::{ProviderAdapter, RateLimitTelemetry, RuntimeAdapter};

fn protocol_base_url(protocol: &str) -> &'static str {
    match protocol {
        "openai-chat" | "openai-responses" => "https://api.openai.com",
        "anthropic" => "https://api.anthropic.com",
        "gemini" => "https://generativelanguage.googleapis.com",
        "vertex-ai" => "https://aiplatform.googleapis.com",
        "ollama" => "http://localhost:11434/api",
        _ => unreachable!("test protocol"),
    }
}

fn decode(protocol: &str, raw: Value) -> crate::domain::UnifiedChatResponse {
    descriptor_by_id(protocol)
        .expect("valid registry")
        .expect("registered protocol")
        .codec
        .decode_chat(raw)
        .expect("valid response")
}

fn assert_alias_fields_absent(body: &Value) {
    for key in [
        "message",
        "instructions",
        "system_instruction",
        "system_prompt",
        "systemPrompt",
        "prompt",
        "insituTools",
        "toolChoice",
        "tool_config",
        "streamOptions",
    ] {
        assert!(body.get(key).is_none(), "{key} should be ignored");
    }
}

#[test]
fn custom_parameters_cannot_override_protocol_prompt_fields() {
    for protocol in [
        "openai-chat",
        "openai-responses",
        "anthropic",
        "gemini",
        "vertex-ai",
        "ollama",
    ] {
        let mut request = prompt_request();
        request.custom_parameters = protected_custom_parameters();
        let (_, body) = adapter(protocol, protocol_base_url(protocol))
            .build_chat_request(&request)
            .expect("request");
        match protocol {
            "openai-chat" => {
                assert_eq!(body["model"], "stable-model");
                assert_eq!(body["stream"], false);
                assert_eq!(
                    body.pointer("/messages/0/content"),
                    Some(&json!(SYSTEM_TEXT))
                );
                assert_eq!(body.pointer("/messages/1/content"), Some(&json!(USER_TEXT)));
                assert!(body.get("tools").is_none());
            }
            "openai-responses" => {
                assert_eq!(body["model"], "stable-model");
                assert_eq!(
                    body.pointer("/input/0/content/0/text"),
                    Some(&json!(SYSTEM_TEXT))
                );
                assert_eq!(
                    body.pointer("/input/1/content/0/text"),
                    Some(&json!(USER_TEXT))
                );
                assert!(body.get("tools").is_none());
            }
            "anthropic" => {
                assert_eq!(body["model"], "stable-model");
                assert_eq!(body.pointer("/system/0/text"), Some(&json!(SYSTEM_TEXT)));
                assert_eq!(
                    body.pointer("/messages/0/content/0/text"),
                    Some(&json!(USER_TEXT))
                );
                assert!(body.get("tools").is_none());
            }
            "gemini" | "vertex-ai" => {
                assert_eq!(
                    body.pointer("/systemInstruction/parts/0/text"),
                    Some(&json!(SYSTEM_TEXT))
                );
                assert_eq!(
                    body.pointer("/contents/0/parts/0/text"),
                    Some(&json!(USER_TEXT))
                );
                assert_eq!(
                    body.pointer("/generationConfig/temperature"),
                    Some(&json!(0.0))
                );
                assert!(body.get("tools").is_none());
            }
            "ollama" => {
                assert_eq!(body["model"], "stable-model");
                assert_eq!(
                    body.pointer("/messages/0/content"),
                    Some(&json!(SYSTEM_TEXT))
                );
                assert_eq!(body.pointer("/messages/1/content"), Some(&json!(USER_TEXT)));
                assert!(body.get("tools").is_none());
            }
            _ => unreachable!(),
        }
        assert_alias_fields_absent(&body);
    }
}

#[test]
fn allowed_custom_parameters_merge_deeply_and_null_matches_empty() {
    let mut request = prompt_request();
    request.custom_parameters = json!({
        "response_format": {"type": "json_object"},
        "frequency_penalty": 0.4,
        "generationConfig": {
            "candidateCount": 1,
            "responseMimeType": "application/json"
        },
        "safetySettings": [{
            "category": "HARM_CATEGORY_HARASSMENT",
            "threshold": "BLOCK_MEDIUM_AND_ABOVE"
        }]
    });
    let (_, openai) = adapter("openai-chat", "https://api.openai.com")
        .build_chat_request(&request)
        .expect("OpenAI request");
    assert_eq!(
        openai.pointer("/response_format/type"),
        Some(&json!("json_object"))
    );
    assert_eq!(openai["frequency_penalty"], 0.4);

    let (_, gemini) = adapter("gemini", "https://generativelanguage.googleapis.com")
        .build_chat_request(&request)
        .expect("Gemini request");
    assert_eq!(
        gemini.pointer("/generationConfig/temperature"),
        Some(&json!(0.0))
    );
    assert_eq!(
        gemini.pointer("/generationConfig/candidateCount"),
        Some(&json!(1))
    );
    assert_eq!(
        gemini.pointer("/generationConfig/responseMimeType"),
        Some(&json!("application/json"))
    );
    assert_eq!(
        gemini.pointer("/safetySettings/0/category"),
        Some(&json!("HARM_CATEGORY_HARASSMENT"))
    );

    let adapter = adapter("openai-chat", "https://api.openai.com");
    let mut empty = prompt_request();
    empty.custom_parameters = json!({});
    let mut null = prompt_request();
    null.custom_parameters = Value::Null;
    assert_eq!(
        adapter.build_chat_request(&empty).expect("empty").1,
        adapter.build_chat_request(&null).expect("null").1
    );
}

#[test]
fn structured_sampling_thinking_and_search_override_custom_attempts() {
    for protocol in [
        "openai-chat",
        "openai-responses",
        "anthropic",
        "gemini",
        "vertex-ai",
        "ollama",
    ] {
        let mut request = prompt_request();
        request.model = match protocol {
            "openai-chat" => "gpt-5-search-api",
            "openai-responses" => "gpt-5",
            "anthropic" => "claude-sonnet-4",
            "gemini" | "vertex-ai" => "gemini-2.5-pro",
            "ollama" => "qwen3",
            _ => unreachable!(),
        }
        .into();
        request.temperature = Some(0.25);
        request.top_p = Some(0.75);
        request.web_search = protocol != "ollama";
        request.thinking = Some(ThinkingConfig {
            mode: ThinkingMode::Enabled,
            budget_tokens: Some(2048),
            effort: Some(ThinkingEffort::Low),
            summary: None,
        });
        request.custom_parameters = json!({
            "temperature": 1.5,
            "top_p": 0.1,
            "reasoning": {"effort": "high"},
            "thinking": {"type": "disabled"},
            "reasoning_effort": "high",
            "enable_thinking": false,
            "web_search_options": {"search_context_size": "high"},
            "generationConfig": {
                "temperature": 1.5,
                "topP": 0.1,
                "thinkingConfig": {"thinkingBudget": 0}
            },
            "options": {"temperature": 1.5, "top_p": 0.1},
            "think": false
        });
        let (_, body) = adapter(protocol, protocol_base_url(protocol))
            .build_chat_request(&request)
            .expect("request");
        match protocol {
            "openai-chat" => {
                assert_eq!(body["temperature"], 0.25);
                assert_eq!(body["top_p"], 0.75);
                assert_eq!(body["reasoning_effort"], "low");
                assert_eq!(body["web_search_options"], json!({}));
            }
            "openai-responses" => {
                assert_eq!(body["temperature"], 0.25);
                assert_eq!(body["top_p"], 0.75);
                assert_eq!(body.pointer("/reasoning/effort"), Some(&json!("low")));
                assert_eq!(body.pointer("/tools/0/type"), Some(&json!("web_search")));
            }
            "anthropic" => {
                assert_eq!(body["temperature"], 0.25);
                assert_eq!(body["top_p"], 0.75);
                assert_eq!(body.pointer("/thinking/type"), Some(&json!("enabled")));
                assert_eq!(
                    body.pointer("/tools/0/type"),
                    Some(&json!("web_search_20250305"))
                );
            }
            "gemini" | "vertex-ai" => {
                assert_eq!(
                    body.pointer("/generationConfig/temperature"),
                    Some(&json!(0.25))
                );
                assert_eq!(body.pointer("/generationConfig/topP"), Some(&json!(0.75)));
                assert_eq!(
                    body.pointer("/generationConfig/thinkingConfig/thinkingBudget"),
                    Some(&json!(2048))
                );
                assert_eq!(body.pointer("/tools/0/googleSearch"), Some(&json!({})));
            }
            "ollama" => {
                assert_eq!(body.pointer("/options/temperature"), Some(&json!(0.25)));
                assert_eq!(body.pointer("/options/top_p"), Some(&json!(0.75)));
                assert_eq!(body["think"], "low");
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn absent_structured_options_remove_custom_attempts() {
    for protocol in [
        "openai-chat",
        "openai-responses",
        "anthropic",
        "gemini",
        "vertex-ai",
        "ollama",
    ] {
        let mut request = prompt_request();
        request.temperature = None;
        request.top_p = None;
        request.custom_parameters = json!({
            "temperature": 1.0,
            "top_p": 1.0,
            "thinking": {"type": "enabled"},
            "reasoning": {"effort": "high"},
            "web_search_options": {},
            "generationConfig": {"temperature": 1.0, "topP": 1.0, "thinkingConfig": {}},
            "options": {"temperature": 1.0, "top_p": 1.0},
            "think": true
        });
        let (_, body) = adapter(protocol, protocol_base_url(protocol))
            .build_chat_request(&request)
            .expect("request");
        match protocol {
            "gemini" | "vertex-ai" => {
                assert!(body.pointer("/generationConfig/temperature").is_none());
                assert!(body.pointer("/generationConfig/topP").is_none());
                assert!(body.pointer("/generationConfig/thinkingConfig").is_none());
            }
            "ollama" => {
                assert!(body.pointer("/options/temperature").is_none());
                assert!(body.pointer("/options/top_p").is_none());
                assert!(body.get("think").is_none());
            }
            _ => {
                assert!(body.get("temperature").is_none());
                assert!(body.get("top_p").is_none());
                assert!(body.get("thinking").is_none());
                assert!(body.get("reasoning").is_none());
                assert!(body.get("web_search_options").is_none());
                assert!(body.get("tools").is_none());
            }
        }
    }
}

#[test]
fn logprobs_requests_obey_protocol_and_provider_capabilities() {
    let mut request = request();
    request.stream = false;
    let plain = build_openai_chat_body("https://api.openai.com", &request);
    assert!(plain.get("logprobs").is_none());

    request.logprobs = true;
    let openai = build_openai_chat_body("https://api.openai.com", &request);
    assert_eq!(openai["logprobs"], true);

    request.custom_parameters = json!({"logprobs": true, "top_logprobs": 3});
    let dashscope = build_openai_chat_body(
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
        &request,
    );
    assert!(dashscope.get("logprobs").is_none());
    assert!(dashscope.get("top_logprobs").is_none());

    let responses = build_openai_responses_body("https://api.openai.com", &request);
    assert!(responses
        .get("include")
        .and_then(Value::as_array)
        .is_some_and(|items| items
            .iter()
            .any(|item| item == "message.output_text.logprobs")));
    let gemini = build_gemini_body(&request);
    assert_eq!(
        gemini.pointer("/generationConfig/responseLogprobs"),
        Some(&json!(true))
    );
    assert!(build_anthropic_body(&request).get("logprobs").is_none());
}

#[test]
fn thinking_and_web_search_map_to_each_wire_format() {
    let deepseek = build_openai_chat_body("https://api.deepseek.com", &request());
    assert_eq!(deepseek.pointer("/thinking/type"), Some(&json!("enabled")));
    assert_eq!(deepseek["reasoning_effort"], "max");

    let mut gemini_request = request();
    gemini_request.model = "gemini-3-pro".into();
    let gemini = build_gemini_body(&gemini_request);
    assert_eq!(
        gemini.pointer("/generationConfig/thinkingConfig/thinkingLevel"),
        Some(&json!("high"))
    );
    assert!(gemini
        .pointer("/generationConfig/thinkingConfig/thinkingBudget")
        .is_none());

    let mut search = prompt_request();
    search.web_search = true;
    search.model = "claude-sonnet-4-20250514".into();
    let anthropic = build_anthropic_body(&search);
    assert_eq!(
        anthropic.pointer("/tools/0/type"),
        Some(&json!("web_search_20250305"))
    );

    search.model = "gpt-5".into();
    let responses = build_openai_responses_body("https://api.openai.com", &search);
    assert_eq!(
        responses.pointer("/tools/0/type"),
        Some(&json!("web_search"))
    );

    search.model = "gemini-2.5-pro".into();
    for protocol in ["gemini", "vertex-ai"] {
        let (_, body) = adapter(protocol, protocol_base_url(protocol))
            .build_chat_request(&search)
            .expect("Google search request");
        assert_eq!(body.pointer("/tools/0/googleSearch"), Some(&json!({})));
    }
}

#[test]
fn reasoning_history_and_raw_endpoints_preserve_protocol_details() {
    let mut responses_request = request();
    responses_request.stream = false;
    responses_request.messages = vec![
        UnifiedMessage {
            role: "user".into(),
            content: vec![UnifiedContent::Text {
                text: "Translate this.".into(),
            }],
        },
        UnifiedMessage {
            role: "assistant".into(),
            content: vec![
                UnifiedContent::Thinking {
                    text: "Plan".into(),
                    signature: Some("sig".into()),
                    encrypted_data: None,
                },
                UnifiedContent::Text {
                    text: "Need lookup".into(),
                },
            ],
        },
    ];
    let responses = build_openai_responses_body("https://api.openai.com", &responses_request);
    let input = responses["input"].as_array().expect("Responses input");
    assert!(input.iter().any(|item| item["type"] == "reasoning"));

    let mut config = ProviderRuntimeConfig {
        protocol: ProtocolId::registered("openai-chat"),
        base_url: "https://proxy.example/openai/v1".into(),
        use_raw_base_url: true,
        config: json!({}),
        credential: None,
        custom_headers: Vec::new(),
    };
    let adapter = RuntimeAdapter::new(Client::new(), config.clone());
    assert_eq!(
        adapter
            .build_chat_request(&request())
            .expect("raw request")
            .0,
        "https://proxy.example/openai/v1/chat/completions"
    );
    config.protocol = ProtocolId::registered("vertex-ai");
    config.base_url = "https://aiplatform.googleapis.com".into();
    config.use_raw_base_url = false;
    config.config = json!({
        "vertexAi": {
            "projectId": "project-1",
            "location": "global",
            "clientEmail": "svc@example.test"
        }
    });
    config.credential = Some("private-key".into());
    let mut gemini_request = request();
    gemini_request.model = "gemini-3-pro".into();
    gemini_request.stream = false;
    let (url, body) = RuntimeAdapter::new(Client::new(), config)
        .build_chat_request(&gemini_request)
        .expect("Vertex request");
    assert!(url.contains("/projects/project-1/locations/global/publishers/google/models/gemini-3-pro:generateContent"));
    assert_eq!(
        body.pointer("/generationConfig/thinkingConfig/thinkingLevel"),
        Some(&json!("high"))
    );
}

#[test]
fn response_decoders_preserve_reasoning_thoughts_usage_and_logprobs() {
    let openai = decode(
        "openai-chat",
        json!({
            "choices": [{
                "message": {
                    "content": "ok",
                    "reasoning_details": [
                        {"type": "reasoning.text", "text": "think", "signature": "sig"},
                        {"type": "reasoning.encrypted", "data": "secret"}
                    ]
                },
                "logprobs": {"content": [
                    {"token": "o", "logprob": 0.0},
                    {"token": "k", "logprob": -1.3862943611198906}
                ]}
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 2}
        }),
    );
    assert_eq!(openai.text, "ok");
    assert_eq!(openai.reasoning, "think");
    assert_eq!(openai.thinking.len(), 2);
    assert_eq!(openai.usage.expect("usage").output_tokens, 2);
    let stats = openai.logprob_stats.expect("OpenAI logprobs");
    assert_eq!(stats.token_count, 2);
    assert!((stats.average_probability - 0.625).abs() < 0.000001);

    let inline = decode(
        "openai-chat",
        json!({"choices": [{"message": {"content": "<think>plan</think>\ntranslated"}}]}),
    );
    assert_eq!(inline.text, "translated");
    assert_eq!(inline.reasoning, "plan");

    let gemini = decode(
        "gemini",
        json!({
            "candidates": [{
                "content": {"parts": [
                    {"text": "think", "thought": true, "thoughtSignature": "sig"},
                    {"text": "translated"}
                ]},
                "logprobsResult": {"chosenCandidates": [
                    {"token": "translated", "logProbability": -0.6931471805599453},
                    {"token": "!", "logProbability": 0.0}
                ]}
            }]
        }),
    );
    assert_eq!(gemini.text, "translated");
    assert_eq!(gemini.reasoning, "think");
    assert_eq!(gemini.thinking.len(), 1);
    assert_eq!(
        gemini.logprob_stats.expect("Gemini logprobs").token_count,
        1
    );
}

#[test]
fn anthropic_roles_and_openai_history_thinking_keep_legacy_behavior() {
    let mut anthropic_request = request();
    anthropic_request.messages.insert(
        0,
        UnifiedMessage {
            role: "user".into(),
            content: vec![UnifiedContent::Text {
                text: "first".into(),
            }],
        },
    );
    anthropic_request.logprobs = true;
    let anthropic = build_anthropic_body(&anthropic_request);
    assert_eq!(anthropic.pointer("/messages/0/role"), Some(&json!("user")));
    assert!(anthropic.get("logprobs").is_none());

    let mut openai_request = prompt_request();
    openai_request.messages.push(UnifiedMessage {
        role: "assistant".into(),
        content: vec![UnifiedContent::Thinking {
            text: "hidden plan".into(),
            signature: None,
            encrypted_data: None,
        }],
    });
    let body = build_openai_chat_body("https://api.openai.com", &openai_request);
    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2);
    assert!(!messages
        .iter()
        .any(|message| message.get("content") == Some(&json!("hidden plan"))));
}

#[test]
fn provider_errors_classify_model_availability_and_transience_explicitly() {
    let error = |status, message: &str, kind| ProviderChatError {
        status,
        message: message.into(),
        compatibility_text: None,
        rate_limits: RateLimitTelemetry::default(),
        kind,
    };
    assert!(error(
        Some(404),
        "The requested model does not exist",
        ProviderChatErrorKind::HttpStatus,
    )
    .is_model_unavailable());
    assert!(!error(
        Some(404),
        "Resource not found",
        ProviderChatErrorKind::HttpStatus,
    )
    .is_model_unavailable());
    assert!(error(Some(429), "quota", ProviderChatErrorKind::HttpStatus,).is_transient());
    assert!(error(None, "network", ProviderChatErrorKind::Transport).is_transient());
    assert!(!error(Some(400), "invalid", ProviderChatErrorKind::HttpStatus,).is_transient());
}
