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

#[allow(dead_code)]
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

#[allow(dead_code)]
pub struct JsonEventStreamDecoder {
    protocol_id: &'static str,
    decode: fn(Value) -> Result<UnifiedChatResponse, String>,
    buffer: Vec<u8>,
    sse_data: Vec<String>,
}

impl JsonEventStreamDecoder {
    pub fn new(
        protocol_id: &'static str,
        decode: fn(Value) -> Result<UnifiedChatResponse, String>,
    ) -> Self {
        Self {
            protocol_id,
            decode,
            buffer: Vec::new(),
            sse_data: Vec::new(),
        }
    }

    fn drain_lines(&mut self) -> Result<Vec<UnifiedChatResponse>, String> {
        let mut output = Vec::new();
        while let Some(index) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let mut line = self.buffer.drain(..=index).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = std::str::from_utf8(&line).map_err(|error| {
                format!(
                    "{} protocol stream contains invalid UTF-8: {error}",
                    self.protocol_id
                )
            })?;
            output.extend(self.process_line(line)?);
        }
        Ok(output)
    }

    fn drain_remainder(&mut self) -> Result<Vec<UnifiedChatResponse>, String> {
        let mut output = Vec::new();
        if !self.buffer.is_empty() {
            let remainder = std::mem::take(&mut self.buffer);
            let line = std::str::from_utf8(&remainder).map_err(|error| {
                format!(
                    "{} protocol stream contains invalid UTF-8: {error}",
                    self.protocol_id
                )
            })?;
            output.extend(self.process_line(line.trim_end_matches('\r'))?);
        }
        output.extend(self.dispatch_sse_event()?);
        Ok(output)
    }

    fn process_line(&mut self, line: &str) -> Result<Vec<UnifiedChatResponse>, String> {
        if line.is_empty() {
            return self.dispatch_sse_event();
        }
        if line.starts_with(':') {
            return Ok(Vec::new());
        }
        let (field, value) = line
            .split_once(':')
            .map(|(field, value)| (field, value.strip_prefix(' ').unwrap_or(value)))
            .unwrap_or((line, ""));
        match field {
            "data" => {
                self.sse_data.push(value.to_string());
                Ok(Vec::new())
            }
            "event" | "id" | "retry" => Ok(Vec::new()),
            _ => {
                if !self.sse_data.is_empty() {
                    return Err(format!(
                        "{} protocol stream mixes SSE data with an NDJSON line",
                        self.protocol_id
                    ));
                }
                self.decode_payload(line)
            }
        }
    }

    fn dispatch_sse_event(&mut self) -> Result<Vec<UnifiedChatResponse>, String> {
        if self.sse_data.is_empty() {
            return Ok(Vec::new());
        }
        let payload = std::mem::take(&mut self.sse_data).join("\n");
        self.decode_payload(&payload)
    }

    fn decode_payload(&self, payload: &str) -> Result<Vec<UnifiedChatResponse>, String> {
        let payload = payload.trim();
        if payload.is_empty() || payload == "[DONE]" {
            return Ok(Vec::new());
        }
        let raw = serde_json::from_str::<Value>(payload).map_err(|error| {
            format!(
                "{} protocol stream contains invalid JSON: {error}; payload={}",
                self.protocol_id,
                truncate(payload, 300)
            )
        })?;
        let response = (self.decode)(raw).map_err(|error| {
            format!(
                "{} protocol stream event could not be decoded: {error}",
                self.protocol_id
            )
        })?;
        Ok(vec![response])
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode_test(raw: Value) -> Result<UnifiedChatResponse, String> {
        let text = raw
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing text".to_string())?;
        Ok(crate::providers::shared::unified_response(
            raw.clone(),
            text.to_string(),
            String::new(),
            Vec::new(),
            None,
            Vec::new(),
        ))
    }

    fn decoder() -> JsonEventStreamDecoder {
        JsonEventStreamDecoder::new("test-stream", decode_test)
    }

    #[test]
    fn parses_sse_across_every_byte_boundary_with_metadata_and_done_marker() {
        let payload = concat!(
            ": keepalive\r\n",
            "event: message\r\n",
            "id: 1\r\n",
            "data: {\"text\":\"你好\"}\r\n",
            "\r\n",
            "data: [DONE]\r\n",
            "\r\n"
        );
        let mut decoder = decoder();
        let mut output = Vec::new();
        for byte in payload.as_bytes() {
            output.extend(decoder.push(&[*byte]).expect("single-byte chunk"));
        }
        output.extend(decoder.finish().expect("finish"));
        assert_eq!(
            output
                .iter()
                .map(|response| response.text.as_str())
                .collect::<Vec<_>>(),
            vec!["你好"]
        );
    }

    #[test]
    fn parses_multiline_sse_and_ndjson_with_an_unterminated_remainder() {
        let mut sse = decoder();
        let output = sse
            .push(b"data: {\"text\":\ndata: \"joined\"}\n\n")
            .expect("multiline SSE");
        assert_eq!(output[0].text, "joined");

        let mut ndjson = decoder();
        let mut output = ndjson
            .push(b"{\"text\":\"one\"}\n{\"text\":\"two\"}")
            .expect("NDJSON chunk");
        output.extend(ndjson.finish().expect("NDJSON remainder"));
        assert_eq!(
            output
                .iter()
                .map(|response| response.text.as_str())
                .collect::<Vec<_>>(),
            vec!["one", "two"]
        );
    }

    #[test]
    fn rejects_invalid_utf8_json_and_codec_events() {
        let mut invalid_utf8 = decoder();
        assert!(invalid_utf8.push(&[0xff, b'\n']).is_err());

        let mut invalid_json = decoder();
        let error = invalid_json
            .push(b"data: {not-json}\n\n")
            .expect_err("invalid JSON");
        assert!(error.contains("test-stream"));
        assert!(error.contains("invalid JSON"));

        let mut invalid_event = decoder();
        let error = invalid_event
            .push(format!("data: {}\n\n", json!({"bad": true})).as_bytes())
            .expect_err("codec error");
        assert!(error.contains("could not be decoded"));
        assert!(error.contains("missing text"));
    }
}
