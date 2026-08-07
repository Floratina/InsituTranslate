use serde_json::{json, Value};

use crate::domain::{
    ProtocolId, ProviderRuntimeConfig, UnifiedChatRequest, UnifiedContent, UnifiedMessage,
};
use crate::providers::codec::HttpMethod;
use crate::providers::config_schema::materialize_defaults;
use crate::providers::registry::{descriptor_by_id, DESCRIPTORS};
use crate::providers::ProtocolCodec;

struct ProtocolCase {
    id: &'static str,
    model: &'static str,
    model_url: &'static str,
    chat_url: &'static str,
    body_key: &'static str,
    models: Value,
    first_model: &'static str,
    response: Value,
    finish_reason: &'static str,
}

fn cases() -> Vec<ProtocolCase> {
    vec![
        ProtocolCase {
            id: "openai-chat",
            model: "gpt-4.1",
            model_url: "/v1/models",
            chat_url: "/v1/chat/completions",
            body_key: "messages",
            models: json!({"data": [{"id": "z-model"}, {"id": "a-model"}]}),
            first_model: "a-model",
            response: json!({
                "choices": [{"message": {"content": "ok"}, "finish_reason": "length"}]
            }),
            finish_reason: "length",
        },
        ProtocolCase {
            id: "openai-responses",
            model: "gpt-5",
            model_url: "/v1/models",
            chat_url: "/v1/responses",
            body_key: "input",
            models: json!({"data": [{"id": "z-model"}, {"id": "a-model"}]}),
            first_model: "a-model",
            response: json!({
                "status": "completed",
                "output": [{
                    "type": "message",
                    "content": [{"type": "output_text", "text": "ok"}]
                }]
            }),
            finish_reason: "completed",
        },
        ProtocolCase {
            id: "anthropic",
            model: "claude-sonnet-4-20250514",
            model_url: "/v1/models",
            chat_url: "/v1/messages",
            body_key: "messages",
            models: json!({"data": [{"id": "z-model"}, {"id": "a-model"}]}),
            first_model: "a-model",
            response: json!({
                "content": [{"type": "text", "text": "ok"}],
                "stop_reason": "end_turn"
            }),
            finish_reason: "end_turn",
        },
        ProtocolCase {
            id: "gemini",
            model: "gemini-2.5-pro",
            model_url: "/v1beta/models",
            chat_url: "/v1beta/models/gemini-2.5-pro:generateContent",
            body_key: "contents",
            models: json!({
                "models": [{"name": "models/gemini-z"}, {"name": "models/gemini-a"}]
            }),
            first_model: "models/gemini-a",
            response: json!({
                "candidates": [{
                    "content": {"parts": [{"text": "ok"}]},
                    "finishReason": "STOP"
                }]
            }),
            finish_reason: "STOP",
        },
        ProtocolCase {
            id: "vertex-ai",
            model: "gemini-2.5-pro",
            model_url: "/v1beta1/publishers/google/models?pageSize=100&listAllVersions=true",
            chat_url: "/publishers/google/models/gemini-2.5-pro:generateContent",
            body_key: "contents",
            models: json!({
                "publisherModels": [
                    {"name": "publishers/google/models/gemini-z"},
                    {"name": "publishers/google/models/gemini-a"}
                ]
            }),
            first_model: "gemini-a",
            response: json!({
                "candidates": [{
                    "content": {"parts": [{"text": "ok"}]},
                    "finishReason": "STOP"
                }]
            }),
            finish_reason: "STOP",
        },
        ProtocolCase {
            id: "ollama",
            model: "qwen3:8b",
            model_url: "/api/tags",
            chat_url: "/api/chat",
            body_key: "messages",
            models: json!({"models": [{"name": "z-model"}, {"name": "a-model"}]}),
            first_model: "a-model",
            response: json!({
                "message": {"content": "ok"},
                "done_reason": "stop"
            }),
            finish_reason: "stop",
        },
        ProtocolCase {
            id: "test-seventh",
            model: "test-r",
            model_url: "/catalog",
            chat_url: "/conversation",
            body_key: "engine",
            models: json!({"models": ["a-model", "z-model"]}),
            first_model: "a-model",
            response: json!({"answer": "ok", "stop": "done"}),
            finish_reason: "done",
        },
    ]
}

fn config(id: &str) -> ProviderRuntimeConfig {
    let descriptor = descriptor_by_id(id)
        .expect("valid registry")
        .expect("registered protocol");
    ProviderRuntimeConfig {
        protocol: ProtocolId::registered(id),
        base_url: descriptor.default_base_url.into(),
        use_raw_base_url: false,
        config: materialize_defaults(
            if id == "vertex-ai" {
                json!({
                    "vertexAi": {
                        "projectId": "project-1",
                        "location": "global",
                        "clientEmail": "svc@example.test"
                    }
                })
            } else {
                json!({})
            },
            descriptor.config_fields,
        )
        .expect("protocol defaults"),
        credential: (id == "vertex-ai").then(|| "test-private-key".into()),
        custom_headers: Vec::new(),
    }
}

fn request(model: &str) -> UnifiedChatRequest {
    UnifiedChatRequest {
        model: model.into(),
        messages: vec![UnifiedMessage {
            role: "user".into(),
            content: vec![UnifiedContent::Text {
                text: "Hello".into(),
            }],
        }],
        web_search: false,
        thinking: None,
        max_output_tokens: Some(4096),
        temperature: Some(0.2),
        top_p: None,
        logprobs: false,
        custom_parameters: json!({}),
    }
}

fn assert_object_safe(_: &'static dyn ProtocolCodec) {}

#[test]
fn every_registered_protocol_matches_request_and_response_golden_shapes() {
    let cases = cases();
    let mut case_ids = cases.iter().map(|case| case.id).collect::<Vec<_>>();
    let mut descriptor_ids = DESCRIPTORS
        .iter()
        .map(|descriptor| descriptor.id)
        .collect::<Vec<_>>();
    case_ids.sort_unstable();
    descriptor_ids.sort_unstable();
    assert_eq!(
        case_ids, descriptor_ids,
        "every descriptor needs a golden case"
    );

    for case in cases {
        let descriptor = descriptor_by_id(case.id)
            .expect("valid registry")
            .expect("descriptor");
        let codec = descriptor.codec;
        assert_object_safe(codec);
        let config = config(case.id);
        if case.id == "test-seventh" {
            assert_eq!(config.config["mode"], "fast");
        }

        let model_request = codec.encode_model_list(&config).expect("model request");
        assert_eq!(model_request.method, HttpMethod::Get, "{}", case.id);
        assert!(model_request.url.ends_with(case.model_url), "{}", case.id);
        assert!(model_request.body.is_none(), "{}", case.id);

        let chat_request = codec
            .encode_chat(&config, &request(case.model))
            .expect("chat request");
        assert_eq!(chat_request.method, HttpMethod::Post, "{}", case.id);
        assert!(chat_request.url.ends_with(case.chat_url), "{}", case.id);
        assert!(
            chat_request
                .body
                .as_ref()
                .and_then(|body| body.get(case.body_key))
                .is_some(),
            "{}",
            case.id
        );

        if case.id == "anthropic" {
            assert_eq!(chat_request.headers.len(), 1);
            assert_eq!(chat_request.headers[0].name, "anthropic-version");
            assert_eq!(chat_request.headers[0].value, "2023-06-01");
        } else {
            assert!(chat_request.headers.is_empty(), "{}", case.id);
        }

        let models = codec.decode_model_list(&case.models).expect("model list");
        assert_eq!(models.len(), 2, "{}", case.id);
        assert_eq!(models[0].request_name, case.first_model, "{}", case.id);

        assert_eq!(
            codec
                .decode_chat(case.response.clone())
                .expect("chat response")
                .text,
            "ok",
            "{}",
            case.id
        );
        assert_eq!(
            codec.finish_reason(&case.response).as_deref(),
            Some(case.finish_reason),
            "{}",
            case.id
        );

        let error = codec.decode_error(422, r#"{"error":"unsupported field"}"#);
        assert!(error.contains("HTTP 422"), "{}", case.id);
        assert!(error.contains("unsupported field"), "{}", case.id);

        let preview = codec.preview_endpoints(&config).expect("endpoint preview");
        assert_eq!(preview.models.as_deref(), Some(model_request.url.as_str()));
        assert!(preview.chat.contains("{model}") || preview.chat == chat_request.url);
    }
}
