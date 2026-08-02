use serde_json::{json, Value};

use crate::domain::{
    ProviderRuntimeConfig, RemoteModel, ThinkingConfig, ThinkingEffort, UnifiedChatRequest,
    UnifiedChatResponse,
};
use crate::providers::capabilities::ModelCapabilities;
use crate::providers::codec::{
    append_endpoint_suffix, EncodedRequest, EndpointPreview, HttpMethod, JsonEventStreamDecoder,
    ProtocolCodec, ProtocolStreamDecoder,
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
            body: Some(json!({"engine": request.model, "stream": request.stream})),
        })
    }

    fn decode_chat(&self, raw: Value) -> Result<UnifiedChatResponse, String> {
        decode(raw)
    }

    fn finish_reason(&self, raw: &Value) -> Option<String> {
        raw.get("stop").and_then(Value::as_str).map(str::to_string)
    }

    fn new_stream_decoder(&self) -> Box<dyn ProtocolStreamDecoder> {
        Box::new(JsonEventStreamDecoder::new(self.id(), decode))
    }

    fn infer_capabilities(&self, _base_url: &str, model_id: &str) -> ModelCapabilities {
        ModelCapabilities {
            reasoning: model_id.ends_with("-r"),
            web: false,
            thinking_efforts: self.supported_thinking_efforts(
                "",
                model_id,
                model_id.ends_with("-r"),
            ),
        }
    }

    fn supported_thinking_efforts(
        &self,
        _base_url: &str,
        _model_id: &str,
        reasoning: bool,
    ) -> Vec<ThinkingEffort> {
        if reasoning {
            vec![ThinkingEffort::None, ThinkingEffort::High]
        } else {
            vec![ThinkingEffort::None]
        }
    }

    fn resolve_thinking(
        &self,
        _base_url: &str,
        _model_id: &str,
        effort: ThinkingEffort,
    ) -> ThinkingConfig {
        crate::providers::thinking::base_config(effort)
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
