use serde_json::Value;

use crate::domain::{ProviderRuntimeConfig, RemoteModel, UnifiedChatRequest, UnifiedChatResponse};
use crate::providers::budget::{CompletionBudgetAlias, GEMINI_ALIASES};
use crate::providers::codec::{EncodedRequest, EndpointPreview, HttpMethod, ProtocolCodec};

pub struct VertexAiCodec;

pub static CODEC: VertexAiCodec = VertexAiCodec;

impl ProtocolCodec for VertexAiCodec {
    fn id(&self) -> &'static str {
        "vertex-ai"
    }

    fn completion_budget_aliases(&self) -> &'static [CompletionBudgetAlias] {
        GEMINI_ALIASES
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

    fn preview_endpoints(&self, config: &ProviderRuntimeConfig) -> Result<EndpointPreview, String> {
        let vertex = crate::vertex_ai::runtime_config(config)?;
        Ok(EndpointPreview {
            chat: crate::vertex_ai::generate_content_url(
                &config.base_url,
                &vertex.project_id,
                &vertex.location,
                "{model}",
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
    super::gemini::decode_chat_for_protocol(raw, "Vertex AI")
}

#[cfg(test)]
mod usage_tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn vertex_usage_uses_gemini_normalization() {
        let response = decode_chat(json!({
            "usageMetadata": {
                "promptTokenCount": 40,
                "candidatesTokenCount": 12,
                "thoughtsTokenCount": 8,
                "totalTokenCount": 60
            }
        }))
        .expect("valid Vertex response");
        let usage = response.usage.expect("usage");

        assert_eq!(usage.output_tokens, 12);
        assert_eq!(usage.thinking_tokens, 8);
        assert_eq!(usage.total_tokens, 60);
    }
}
