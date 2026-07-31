use serde_json::Value;

use crate::domain::{
    ProviderRuntimeConfig, RemoteModel, ThinkingConfig, ThinkingEffort, UnifiedChatRequest,
    UnifiedChatResponse,
};
use crate::features::{is_feature_supported, FeatureId};
use crate::providers::capabilities::ModelCapabilities;
use crate::providers::codec::{
    EncodedRequest, EndpointPreview, HttpMethod, JsonEventStreamDecoder, ProtocolCodec,
    ProtocolStreamDecoder,
};
use crate::providers::thinking;

pub struct VertexAiCodec;

pub static CODEC: VertexAiCodec = VertexAiCodec;

impl ProtocolCodec for VertexAiCodec {
    fn id(&self) -> &'static str {
        "vertex-ai"
    }

    fn encode_model_list(&self, config: &ProviderRuntimeConfig) -> Result<EncodedRequest, String> {
        let vertex = crate::vertex_ai::runtime_config(config)?;
        let url = format!(
            "{}?pageSize=100&listAllVersions=true",
            crate::vertex_ai::publisher_models_url(&config.base_url, &vertex.location, "google")
        );
        Ok(EncodedRequest {
            method: HttpMethod::Get,
            url,
            headers: Vec::new(),
            body: None,
        })
    }

    fn decode_model_list(&self, raw: &Value) -> Result<Vec<RemoteModel>, String> {
        Ok(super::gemini::google_models(raw, true, true))
    }

    fn encode_chat(
        &self,
        config: &ProviderRuntimeConfig,
        request: &UnifiedChatRequest,
    ) -> Result<EncodedRequest, String> {
        let vertex = crate::vertex_ai::runtime_config(config)?;
        let url = crate::vertex_ai::generate_content_url(
            &config.base_url,
            &vertex.project_id,
            &vertex.location,
            &request.model,
            request.stream,
        );
        Ok(EncodedRequest {
            method: HttpMethod::Post,
            url,
            headers: Vec::new(),
            body: Some(super::gemini::build_body(&config.base_url, request)?),
        })
    }

    fn decode_chat(&self, raw: Value) -> Result<UnifiedChatResponse, String> {
        decode_chat(raw)
    }

    fn finish_reason(&self, raw: &Value) -> Option<String> {
        super::gemini::finish_reason(raw)
    }

    fn new_stream_decoder(&self) -> Box<dyn ProtocolStreamDecoder> {
        Box::new(JsonEventStreamDecoder::new(decode_chat))
    }

    fn infer_capabilities(&self, base_url: &str, model_id: &str) -> ModelCapabilities {
        let inferred = crate::features::gemini_capabilities(model_id);
        ModelCapabilities {
            reasoning: inferred.reasoning,
            web: inferred.web,
            thinking_efforts: self.supported_thinking_efforts(
                base_url,
                model_id,
                inferred.reasoning,
            ),
        }
    }

    fn supported_thinking_efforts(
        &self,
        base_url: &str,
        model_id: &str,
        reasoning: bool,
    ) -> Vec<ThinkingEffort> {
        crate::features::gemini_thinking_efforts(base_url, model_id, reasoning)
    }

    fn resolve_thinking(
        &self,
        base_url: &str,
        model_id: &str,
        effort: ThinkingEffort,
    ) -> ThinkingConfig {
        let mut config = thinking::base_config(effort);
        if is_feature_supported(FeatureId::GeminiThinkingLevel, base_url, model_id) {
            config.effort = Some(thinking::gemini_level_effort(effort));
        } else {
            config.budget_tokens = Some(thinking::budget_tokens(effort));
        }
        config
    }

    fn preview_endpoints(&self, config: &ProviderRuntimeConfig) -> Result<EndpointPreview, String> {
        let vertex = crate::vertex_ai::runtime_config(config)?;
        Ok(EndpointPreview {
            chat: crate::vertex_ai::generate_content_url(
                &config.base_url,
                &vertex.project_id,
                &vertex.location,
                "{model}",
                false,
            ),
            models: Some(format!(
                "{}?pageSize=100&listAllVersions=true",
                crate::vertex_ai::publisher_models_url(
                    &config.base_url,
                    &vertex.location,
                    "google",
                )
            )),
        })
    }
}

fn decode_chat(raw: Value) -> Result<UnifiedChatResponse, String> {
    super::gemini::decode_chat(raw)
}
