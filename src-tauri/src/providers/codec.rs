use serde_json::Value;

use crate::domain::{
    ProviderRuntimeConfig, RemoteModel, ThinkingConfig, ThinkingEffort, UnifiedChatRequest,
    UnifiedChatResponse,
};
use crate::providers::capabilities::ModelCapabilities;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderMode {
    Replace,
    IfAbsent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderDirective {
    pub name: String,
    pub value: String,
    pub mode: HeaderMode,
}

#[derive(Debug, Clone)]
pub struct EncodedRequest {
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<HeaderDirective>,
    pub body: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointPreview {
    pub chat: String,
    pub models: Option<String>,
}

pub trait ProtocolStreamDecoder: Send {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<UnifiedChatResponse>, String>;
    fn finish(&mut self) -> Result<Vec<UnifiedChatResponse>, String>;
}

pub trait ProtocolCodec: Send + Sync {
    fn id(&self) -> &'static str;
    fn encode_model_list(&self, config: &ProviderRuntimeConfig) -> Result<EncodedRequest, String>;
    fn decode_model_list(&self, raw: &Value) -> Result<Vec<RemoteModel>, String>;
    fn encode_chat(
        &self,
        config: &ProviderRuntimeConfig,
        request: &UnifiedChatRequest,
    ) -> Result<EncodedRequest, String>;
    fn decode_chat(&self, raw: Value) -> Result<UnifiedChatResponse, String>;
    fn finish_reason(&self, raw: &Value) -> Option<String>;
    fn new_stream_decoder(&self) -> Box<dyn ProtocolStreamDecoder>;
    fn infer_capabilities(&self, base_url: &str, model_id: &str) -> ModelCapabilities;
    fn supported_thinking_efforts(
        &self,
        base_url: &str,
        model_id: &str,
        reasoning: bool,
    ) -> Vec<ThinkingEffort>;
    fn resolve_thinking(
        &self,
        base_url: &str,
        model_id: &str,
        effort: ThinkingEffort,
    ) -> ThinkingConfig;
    fn preview_endpoints(&self, config: &ProviderRuntimeConfig) -> Result<EndpointPreview, String>;

    fn decode_error(&self, status: u16, body: &str) -> String {
        format!("HTTP {status}: {}", truncate(body, 500))
    }
}

pub struct JsonEventStreamDecoder {
    decode: fn(Value) -> Result<UnifiedChatResponse, String>,
    buffer: Vec<u8>,
}

impl JsonEventStreamDecoder {
    pub fn new(decode: fn(Value) -> Result<UnifiedChatResponse, String>) -> Self {
        Self {
            decode,
            buffer: Vec::new(),
        }
    }

    fn drain_lines(&mut self) -> Result<Vec<UnifiedChatResponse>, String> {
        let mut output = Vec::new();
        while let Some(index) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line = self.buffer.drain(..=index).collect::<Vec<_>>();
            let line = std::str::from_utf8(&line)
                .map_err(|error| format!("Protocol stream contains invalid UTF-8: {error}"))?;
            let line = line.trim();
            let data = line.strip_prefix("data:").map(str::trim).unwrap_or(line);
            if data.is_empty() || data == "[DONE]" || data.starts_with(':') {
                continue;
            }
            if let Ok(raw) = serde_json::from_str::<Value>(data) {
                if let Ok(response) = (self.decode)(raw) {
                    output.push(response);
                }
            }
        }
        Ok(output)
    }

    fn drain_remainder(&mut self) -> Result<Vec<UnifiedChatResponse>, String> {
        if self.buffer.is_empty() {
            return Ok(Vec::new());
        }
        self.buffer.push(b'\n');
        self.drain_lines()
    }
}

impl ProtocolStreamDecoder for JsonEventStreamDecoder {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<UnifiedChatResponse>, String> {
        self.buffer.extend_from_slice(chunk);
        self.drain_lines()
    }

    fn finish(&mut self) -> Result<Vec<UnifiedChatResponse>, String> {
        self.drain_remainder()
    }
}

pub fn append_endpoint_suffix(base_url: &str, suffix: &str) -> String {
    let base = endpoint_base_url(base_url).trim_end_matches('/');
    let suffix = suffix.trim_start_matches('/');
    if suffix.is_empty() {
        base.to_string()
    } else {
        format!("{base}/{suffix}")
    }
}

pub fn endpoint_base_url(base_url: &str) -> &str {
    base_url.split(['?', '#']).next().unwrap_or(base_url)
}

pub fn openai_endpoint(config: &ProviderRuntimeConfig, suffix: &str) -> String {
    let base = endpoint_base_url(&config.base_url).trim_end_matches('/');
    if config.use_raw_base_url || is_versioned_base_url(base) {
        append_endpoint_suffix(base, suffix)
    } else {
        append_endpoint_suffix(&format!("{base}/v1"), suffix)
    }
}

fn is_versioned_base_url(base_url: &str) -> bool {
    base_url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .map(|segment| {
            segment == "v1"
                || segment == "v1beta"
                || segment.strip_prefix('v').is_some_and(|version| {
                    version.chars().all(|character| character.is_ascii_digit())
                })
        })
        .unwrap_or(false)
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        value.to_string()
    } else {
        value.chars().take(max).collect::<String>() + "…"
    }
}
