use serde_json::{json, Value};

use crate::domain::{ProviderRuntimeConfig, RemoteModel, UnifiedChatRequest, UnifiedChatResponse};
use crate::providers::codec::{
    append_endpoint_suffix, EncodedRequest, EndpointPreview, HttpMethod, ProtocolCodec,
};

pub struct TestProtocolCodec;

pub static CODEC: TestProtocolCodec = TestProtocolCodec;

impl ProtocolCodec for TestProtocolCodec {
    fn id(&self) -> &'static str {
        "test-seventh"
    }

    fn encode_model_list(&self, config: &ProviderRuntimeConfig) -> Result<EncodedRequest, String> {
        Ok(EncodedRequest {
            method: HttpMethod::Get,
            url: append_endpoint_suffix(&config.base_url, "catalog"),
            headers: Vec::new(),
            body: None,
        })
    }

    fn decode_model_list(&self, raw: &Value) -> Result<Vec<RemoteModel>, String> {
        Ok(raw
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|model| RemoteModel {
                request_name: model.into(),
                alias: model.into(),
                added: false,
            })
            .collect())
    }

    fn encode_chat(
        &self,
        config: &ProviderRuntimeConfig,
        request: &UnifiedChatRequest,
    ) -> Result<EncodedRequest, String> {
        Ok(EncodedRequest {
            method: HttpMethod::Post,
            url: append_endpoint_suffix(&config.base_url, "conversation"),
            headers: Vec::new(),
            body: Some(json!({"engine": request.model})),
        })
    }

    fn decode_chat(&self, raw: Value) -> Result<UnifiedChatResponse, String> {
        decode(raw)
    }

    fn finish_reason(&self, raw: &Value) -> Option<String> {
        raw.get("stop").and_then(Value::as_str).map(str::to_string)
    }

    fn preview_endpoints(&self, config: &ProviderRuntimeConfig) -> Result<EndpointPreview, String> {
        Ok(EndpointPreview {
            chat: append_endpoint_suffix(&config.base_url, "conversation"),
            models: Some(append_endpoint_suffix(&config.base_url, "catalog")),
        })
    }

    fn decode_error(&self, status: u16, body: &str) -> String {
        let message = serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| body.to_string());
        format!("test-seventh HTTP {status}: {message}")
    }
}

fn decode(raw: Value) -> Result<UnifiedChatResponse, String> {
    Ok(crate::providers::shared::unified_response(
        raw.clone(),
        raw.get("answer")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        String::new(),
        Vec::new(),
        None,
        Vec::new(),
    ))
}
