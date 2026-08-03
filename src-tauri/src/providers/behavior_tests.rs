use reqwest::Client;
use serde_json::{json, Value};

use crate::domain::{
    ProtocolId, ProviderRuntimeConfig, ThinkingConfig, ThinkingEffort, ThinkingMode,
    UnifiedContent, UnifiedMessage,
};
use crate::providers::protocols::anthropic;
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

fn anthropic_thinking_request(
    model: &str,
    effort: ThinkingEffort,
    budget_tokens: Option<u32>,
    max_output_tokens: Option<u32>,
) -> crate::domain::UnifiedChatRequest {
    let mut request = prompt_request();
    request.model = model.into();
    request.temperature = None;
    let adaptive = [
        "claude-sonnet-4-6",
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-fable-5",
        "claude-mythos-5",
        "claude-opus-5",
        "claude-sonnet-5",
    ]
    .contains(&model);
    request.thinking = Some(ThinkingConfig {
        mode: if adaptive {
            ThinkingMode::Auto
        } else {
            ThinkingMode::Enabled
        },
        budget_tokens,
        effort: Some(effort),
        summary: None,
    });
    request.max_output_tokens = max_output_tokens;
    request
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
        if protocol == "anthropic" {
            request.model = "claude-sonnet-4-5".into();
        }
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
                assert_eq!(body["model"], "claude-sonnet-4-5");
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
        "gemini",
        "vertex-ai",
        "ollama",
    ] {
        let mut request = prompt_request();
        request.model = match protocol {
            "openai-chat" => "gpt-5-search-api",
            "openai-responses" => "gpt-5",
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
    request.model = "claude-sonnet-4-20250514".into();
    request.thinking = None;
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
    anthropic_request.model = "claude-sonnet-4-20250514".into();
    anthropic_request.thinking = None;
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
fn anthropic_model_profiles_drive_capabilities_and_thinking_dialects() {
    let codec = descriptor_by_id("anthropic")
        .expect("valid registry")
        .expect("Anthropic descriptor")
        .codec;

    let manual = codec.infer_capabilities("https://api.anthropic.com", "claude-sonnet-4-5");
    assert!(manual.reasoning);
    assert_eq!(
        manual.thinking_efforts,
        vec![
            ThinkingEffort::None,
            ThinkingEffort::Low,
            ThinkingEffort::Medium,
            ThinkingEffort::High,
        ]
    );

    let opus_47 = codec.infer_capabilities("https://api.anthropic.com", "claude-opus-4-7");
    assert!(opus_47.reasoning);
    assert!(opus_47.thinking_efforts.contains(&ThinkingEffort::Xhigh));
    assert!(opus_47.thinking_efforts.contains(&ThinkingEffort::Max));
    assert!(!opus_47.thinking_efforts.contains(&ThinkingEffort::Minimal));

    let unknown = codec.infer_capabilities("https://api.anthropic.com", "claude-opus-4-9");
    assert!(!unknown.reasoning);
    assert_eq!(unknown.thinking_efforts, vec![ThinkingEffort::None]);

    let manual_mapping = codec
        .resolve_thinking(
            "https://api.anthropic.com",
            "claude-opus-4-5",
            ThinkingEffort::Medium,
        )
        .expect("manual effort mapping");
    assert_eq!(manual_mapping.mode, ThinkingMode::Enabled);
    assert_eq!(manual_mapping.budget_tokens, Some(16_000));
    let adaptive_mapping = codec
        .resolve_thinking(
            "https://api.anthropic.com",
            "claude-sonnet-4-6",
            ThinkingEffort::Max,
        )
        .expect("adaptive effort mapping");
    assert_eq!(adaptive_mapping.mode, ThinkingMode::Auto);
    assert_eq!(adaptive_mapping.budget_tokens, None);

    let manual = anthropic::build_body(&anthropic_thinking_request(
        "claude-sonnet-4-5",
        ThinkingEffort::Low,
        Some(2048),
        None,
    ))
    .expect("manual thinking request");
    assert_eq!(
        manual["thinking"],
        json!({"type": "enabled", "budget_tokens": 2048})
    );
    assert_eq!(manual["max_tokens"], 6144);
    assert!(manual.pointer("/output_config/effort").is_none());

    let manual_high = anthropic::build_body(&anthropic_thinking_request(
        "claude-sonnet-4-5",
        ThinkingEffort::High,
        Some(32_000),
        None,
    ))
    .expect("manual high thinking request");
    assert_eq!(manual_high["thinking"]["budget_tokens"], 32_000);
    assert_eq!(manual_high["max_tokens"], 36_096);

    let opus_45 = anthropic::build_body(&anthropic_thinking_request(
        "claude-opus-4-5",
        ThinkingEffort::Medium,
        Some(4096),
        Some(8192),
    ))
    .expect("Opus 4.5 thinking request");
    assert_eq!(opus_45.pointer("/thinking/type"), Some(&json!("enabled")));
    assert_eq!(
        opus_45.pointer("/output_config/effort"),
        Some(&json!("medium"))
    );

    let adaptive = anthropic::build_body(&anthropic_thinking_request(
        "claude-sonnet-4-6",
        ThinkingEffort::Max,
        None,
        Some(64_000),
    ))
    .expect("adaptive thinking request");
    assert_eq!(adaptive["thinking"], json!({"type": "adaptive"}));
    assert_eq!(
        adaptive.pointer("/output_config/effort"),
        Some(&json!("max"))
    );

    let opus_47 = anthropic::build_body(&anthropic_thinking_request(
        "claude-opus-4-7",
        ThinkingEffort::Xhigh,
        None,
        Some(64_000),
    ))
    .expect("Opus 4.7 adaptive thinking request");
    assert_eq!(opus_47["thinking"], json!({"type": "adaptive"}));
    assert_eq!(
        opus_47.pointer("/output_config/effort"),
        Some(&json!("xhigh"))
    );
}

#[test]
fn anthropic_structured_values_override_custom_thinking_and_output_fields() {
    let mut request = anthropic_thinking_request(
        "claude-opus-4-5",
        ThinkingEffort::Low,
        Some(2048),
        Some(8192),
    );
    request.top_p = Some(0.95);
    request.web_search = true;
    request.custom_parameters = json!({
        "max_tokens": 2048,
        "temperature": 0.5,
        "top_p": 0.2,
        "thinking": {"type": "disabled"},
        "output_config": {"effort": "high", "format": {"type": "json_schema"}},
        "tools": []
    });

    let body = anthropic::build_body(&request).expect("valid structured request");
    assert_eq!(body["max_tokens"], 8192);
    assert!(body.get("temperature").is_none());
    assert_eq!(body["top_p"], 0.95);
    assert_eq!(body.pointer("/thinking/type"), Some(&json!("enabled")));
    assert_eq!(body.pointer("/output_config/effort"), Some(&json!("low")));
    assert_eq!(
        body.pointer("/output_config/format/type"),
        Some(&json!("json_schema"))
    );
    assert_eq!(
        body.pointer("/tools/0/type"),
        Some(&json!("web_search_20250305"))
    );
}

#[test]
fn anthropic_output_limit_precedence_uses_structured_then_custom_then_automatic() {
    let automatic = anthropic::build_body(&anthropic_thinking_request(
        "claude-sonnet-4-5",
        ThinkingEffort::Low,
        Some(2048),
        None,
    ))
    .expect("automatic budget");
    assert_eq!(automatic["max_tokens"], 6144);

    let mut custom =
        anthropic_thinking_request("claude-sonnet-4-5", ThinkingEffort::Low, Some(2048), None);
    custom.custom_parameters = json!({"max_tokens": 7000});
    assert_eq!(
        anthropic::build_body(&custom).expect("custom budget")["max_tokens"],
        7000
    );

    custom.max_output_tokens = Some(6500);
    assert_eq!(
        anthropic::build_body(&custom).expect("structured budget")["max_tokens"],
        6500
    );

    custom.max_output_tokens = None;
    custom.custom_parameters = json!({"max_tokens": 6143});
    assert!(anthropic::build_body(&custom)
        .expect_err("custom budget below required output")
        .contains("require at least 6144"));
}

#[test]
fn anthropic_rejects_unknown_thinking_and_invalid_token_plans() {
    let unknown = anthropic_thinking_request(
        "third-party-claude",
        ThinkingEffort::Low,
        Some(2048),
        Some(8192),
    );
    let error = anthropic::build_body(&unknown).expect_err("unknown thinking model");
    assert!(error.contains("unrecognized model"));
    assert!(error.contains("disable thinking"));

    let too_small = anthropic_thinking_request(
        "claude-sonnet-4-5",
        ThinkingEffort::Low,
        Some(1000),
        Some(8192),
    );
    assert!(anthropic::build_body(&too_small)
        .expect_err("small budget")
        .contains("at least 1024"));

    let no_visible_output = anthropic_thinking_request(
        "claude-sonnet-4-5",
        ThinkingEffort::Low,
        Some(2048),
        Some(2048),
    );
    assert!(anthropic::build_body(&no_visible_output)
        .expect_err("budget must leave output room")
        .contains("must be smaller than max_tokens"));

    let mut custom_conflict =
        anthropic_thinking_request("claude-sonnet-4-5", ThinkingEffort::Low, Some(2048), None);
    custom_conflict.custom_parameters = json!({"max_tokens": 2048});
    assert!(anthropic::build_body(&custom_conflict)
        .expect_err("custom max conflict")
        .contains("must be smaller than max_tokens"));

    let mut invalid_custom_max = prompt_request();
    invalid_custom_max.model = "claude-sonnet-4-5".into();
    invalid_custom_max.custom_parameters = json!({"max_tokens": "8192"});
    assert!(anthropic::build_body(&invalid_custom_max)
        .expect_err("custom max type")
        .contains("must be a positive integer"));

    let adaptive_budget = anthropic_thinking_request(
        "claude-sonnet-4-6",
        ThinkingEffort::High,
        Some(2048),
        Some(8192),
    );
    assert!(anthropic::build_body(&adaptive_budget)
        .expect_err("adaptive budget")
        .contains("does not accept budget_tokens"));

    let ceiling = anthropic_thinking_request(
        "claude-haiku-4-5",
        ThinkingEffort::Low,
        Some(2048),
        Some(64_001),
    );
    assert!(anthropic::build_body(&ceiling)
        .expect_err("known output ceiling")
        .contains("64000-token output limit"));

    let mut manual_wrong_mode = anthropic_thinking_request(
        "claude-sonnet-4-5",
        ThinkingEffort::Low,
        Some(2048),
        Some(8192),
    );
    manual_wrong_mode.thinking.as_mut().expect("thinking").mode = ThinkingMode::Auto;
    assert!(anthropic::build_body(&manual_wrong_mode)
        .expect_err("manual wrong mode")
        .contains("ThinkingMode::Enabled"));

    let mut adaptive_wrong_mode = anthropic_thinking_request(
        "claude-sonnet-4-6",
        ThinkingEffort::High,
        None,
        Some(36_096),
    );
    adaptive_wrong_mode
        .thinking
        .as_mut()
        .expect("thinking")
        .mode = ThinkingMode::Enabled;
    assert!(anthropic::build_body(&adaptive_wrong_mode)
        .expect_err("adaptive wrong mode")
        .contains("ThinkingMode::Auto"));
}

#[test]
fn anthropic_rejects_sampling_combinations_the_api_does_not_support() {
    let mut manual_temperature = anthropic_thinking_request(
        "claude-sonnet-4-5",
        ThinkingEffort::Low,
        Some(2048),
        Some(8192),
    );
    manual_temperature.temperature = Some(0.2);
    assert!(anthropic::build_body(&manual_temperature)
        .expect_err("manual thinking temperature")
        .contains("incompatible with temperature"));

    let mut manual_top_p = anthropic_thinking_request(
        "claude-sonnet-4-5",
        ThinkingEffort::Low,
        Some(2048),
        Some(8192),
    );
    manual_top_p.top_p = Some(0.94);
    assert!(anthropic::build_body(&manual_top_p)
        .expect_err("manual thinking top_p")
        .contains("0.95 through 1"));

    let mut manual_top_k = anthropic_thinking_request(
        "claude-sonnet-4-5",
        ThinkingEffort::Low,
        Some(2048),
        Some(8192),
    );
    manual_top_k.custom_parameters = json!({"top_k": 20});
    assert!(anthropic::build_body(&manual_top_k)
        .expect_err("manual thinking top_k")
        .contains("incompatible with top_k"));

    let mut haiku = prompt_request();
    haiku.model = "claude-haiku-4-5".into();
    haiku.temperature = Some(0.2);
    haiku.top_p = Some(0.9);
    assert!(anthropic::build_body(&haiku)
        .expect_err("Haiku sampling combination")
        .contains("cannot use temperature and top_p together"));

    let mut opus_47 =
        anthropic_thinking_request("claude-opus-4-7", ThinkingEffort::Xhigh, None, Some(64_000));
    opus_47.temperature = Some(0.2);
    assert!(anthropic::build_body(&opus_47)
        .expect_err("4.7 sampling")
        .contains("does not accept non-default"));

    let mut sonnet_46_without_thinking = prompt_request();
    sonnet_46_without_thinking.model = "claude-sonnet-4-6".into();
    sonnet_46_without_thinking.temperature = Some(0.2);
    assert!(anthropic::build_body(&sonnet_46_without_thinking).is_ok());

    let mut opus_47_default_sampling = prompt_request();
    opus_47_default_sampling.model = "claude-opus-4-7".into();
    opus_47_default_sampling.temperature = Some(1.0);
    opus_47_default_sampling.top_p = Some(1.0);
    assert!(anthropic::build_body(&opus_47_default_sampling).is_ok());

    let mut invalid_temperature = prompt_request();
    invalid_temperature.model = "claude-sonnet-4-5".into();
    invalid_temperature.temperature = Some(1.1);
    assert!(anthropic::build_body(&invalid_temperature)
        .expect_err("temperature range")
        .contains("from 0 through 1"));
}

#[test]
fn anthropic_preflight_and_body_validate_only_effective_sampling_fields() {
    let codec = descriptor_by_id("anthropic")
        .expect("valid registry")
        .expect("Anthropic descriptor")
        .codec;
    let mut request = anthropic_thinking_request(
        "claude-sonnet-4-5",
        ThinkingEffort::Low,
        Some(2048),
        Some(8192),
    );
    request.custom_parameters = json!({
        "temperature": 0.2,
        "top_p": 0.2
    });

    codec
        .validate_chat_options(
            "https://api.anthropic.com",
            &request.model,
            request.thinking.as_ref(),
            request.temperature,
            request.top_p,
            &request.custom_parameters,
        )
        .expect("protected custom sampling fields do not affect preflight");
    let body = anthropic::build_body(&request).expect("protected custom sampling fields");
    assert!(body.get("temperature").is_none());
    assert!(body.get("top_p").is_none());

    request.custom_parameters = json!({
        "temperature": 0.2,
        "top_p": 0.2,
        "top_k": 20
    });
    let preflight_error = codec
        .validate_chat_options(
            "https://api.anthropic.com",
            &request.model,
            request.thinking.as_ref(),
            request.temperature,
            request.top_p,
            &request.custom_parameters,
        )
        .expect_err("effective custom top_k must fail preflight");
    let body_error = anthropic::build_body(&request).expect_err("effective custom top_k must fail");
    assert_eq!(preflight_error, body_error);
    assert!(preflight_error.contains("top_k"));
}

#[test]
fn claude_five_default_and_required_thinking_follow_the_exact_profile() {
    let codec = descriptor_by_id("anthropic")
        .expect("valid registry")
        .expect("Anthropic descriptor")
        .codec;
    let fable = codec.infer_capabilities("https://api.anthropic.com", "claude-fable-5");
    assert!(fable.thinking_required);
    assert_eq!(fable.default_thinking_effort, Some(ThinkingEffort::High));
    assert!(!fable.thinking_efforts.contains(&ThinkingEffort::None));

    let opus = codec.infer_capabilities("https://api.anthropic.com", "claude-opus-5");
    assert!(!opus.thinking_required);
    assert_eq!(opus.default_thinking_effort, Some(ThinkingEffort::High));
    assert!(opus.thinking_efforts.contains(&ThinkingEffort::None));

    let mut omitted = prompt_request();
    omitted.model = "claude-fable-5".into();
    omitted.temperature = None;
    let omitted_body = anthropic::build_body(&omitted).expect("always-on omitted thinking");
    assert!(omitted_body.get("thinking").is_none());
    assert_eq!(omitted_body["max_tokens"], 36_096);

    let mut always_on_disabled = omitted.clone();
    always_on_disabled.thinking = Some(ThinkingConfig {
        mode: ThinkingMode::Disabled,
        budget_tokens: None,
        effort: Some(ThinkingEffort::None),
        summary: None,
    });
    assert!(anthropic::build_body(&always_on_disabled)
        .expect_err("always-on disabled")
        .contains("always on"));

    let mut opus_disabled = omitted.clone();
    opus_disabled.model = "claude-opus-5".into();
    opus_disabled.thinking = Some(ThinkingConfig {
        mode: ThinkingMode::Disabled,
        budget_tokens: None,
        effort: Some(ThinkingEffort::High),
        summary: None,
    });
    assert_eq!(
        anthropic::build_body(&opus_disabled).expect("Opus 5 high disabled")["thinking"],
        json!({"type": "disabled"})
    );

    opus_disabled.thinking.as_mut().expect("thinking").effort = Some(ThinkingEffort::Max);
    assert!(anthropic::build_body(&opus_disabled)
        .expect_err("Opus 5 max disabled")
        .contains("cannot disable thinking"));

    let mut sonnet_disabled = opus_disabled;
    sonnet_disabled.model = "claude-sonnet-5".into();
    sonnet_disabled.thinking.as_mut().expect("thinking").effort = Some(ThinkingEffort::None);
    assert_eq!(
        anthropic::build_body(&sonnet_disabled).expect("Sonnet 5 disabled")["thinking"],
        json!({"type": "disabled"})
    );

    let mut default_temperature = omitted;
    default_temperature.temperature = Some(1.0);
    assert!(anthropic::build_body(&default_temperature).is_ok());
}

#[test]
fn anthropic_preserves_signature_only_thinking_blocks() {
    let response = decode(
        "anthropic",
        json!({
            "content": [{"type": "thinking", "thinking": "", "signature": "sig"}],
            "stop_reason": "end_turn"
        }),
    );
    assert_eq!(response.reasoning, "");
    assert!(matches!(
        response.thinking.as_slice(),
        [UnifiedContent::Thinking { text, signature: Some(signature), encrypted_data: None }]
            if text.is_empty() && signature == "sig"
    ));
}

#[test]
fn anthropic_role_errors_are_not_converted_to_empty_messages() {
    let mut assistant_first = prompt_request();
    assistant_first.model = "claude-sonnet-4-5".into();
    assistant_first.messages = vec![UnifiedMessage {
        role: "assistant".into(),
        content: vec![UnifiedContent::Text {
            text: "answer".into(),
        }],
    }];
    assert!(anthropic::build_body(&assistant_first)
        .expect_err("assistant first")
        .contains("must have the user role"));

    let mut system_only = prompt_request();
    system_only.model = "claude-sonnet-4-5".into();
    system_only.messages.truncate(1);
    assert!(anthropic::build_body(&system_only)
        .expect_err("system only")
        .contains("must have the user role"));

    let mut empty = prompt_request();
    empty.model = "claude-sonnet-4-5".into();
    empty.messages.clear();
    assert!(anthropic::build_body(&empty)
        .expect_err("empty messages")
        .contains("must have the user role"));

    let mut unknown_role = prompt_request();
    unknown_role.model = "claude-sonnet-4-5".into();
    unknown_role.messages = vec![UnifiedMessage {
        role: "tool".into(),
        content: vec![UnifiedContent::Text {
            text: "tool output".into(),
        }],
    }];
    assert!(anthropic::build_body(&unknown_role)
        .expect_err("unknown role")
        .contains("only support user and assistant roles"));

    let mut empty_content = prompt_request();
    empty_content.model = "claude-sonnet-4-5".into();
    empty_content.messages[1].content.clear();
    assert!(anthropic::build_body(&empty_content)
        .expect_err("empty message content")
        .contains("at least one content block"));

    let mut empty_system = prompt_request();
    empty_system.model = "claude-sonnet-4-5".into();
    empty_system.messages[0].content.clear();
    assert!(anthropic::build_body(&empty_system)
        .expect_err("empty system content")
        .contains("system messages must contain at least one content block"));
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
