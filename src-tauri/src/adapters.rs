use futures_util::StreamExt;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, Method};
use serde_json::{json, Value};
use std::sync::OnceLock;

use crate::domain::{
    LogprobStats, ProviderProtocol, ProviderRuntimeConfig, RemoteModel, ThinkingConfig,
    ThinkingEffort, ThinkingMode, ThinkingSummary, UnifiedChatRequest, UnifiedChatResponse,
    UnifiedContent, UnifiedMessage, UnifiedUsage,
};
use crate::features::{is_feature_supported, FeatureId};
use crate::vertex_ai;
use std::fmt;

pub trait ProviderAdapter {
    async fn list_models(&self) -> Result<Vec<RemoteModel>, String>;
    fn build_chat_request(&self, request: &UnifiedChatRequest) -> Result<(String, Value), String>;
    async fn send_chat(&self, request: &UnifiedChatRequest) -> Result<UnifiedChatResponse, String>;
    async fn stream_chat(
        &self,
        request: &UnifiedChatRequest,
    ) -> Result<Vec<UnifiedChatResponse>, String>;
}

#[derive(Clone)]
pub struct RuntimeAdapter {
    client: Client,
    config: ProviderRuntimeConfig,
}

#[derive(Debug, Clone, Default)]
pub struct RateLimitTelemetry {
    pub request_limit: Option<u64>,
    pub request_remaining: Option<u64>,
    pub request_reset_ms: Option<u64>,
    pub token_limit: Option<u64>,
    pub token_remaining: Option<u64>,
    pub token_reset_ms: Option<u64>,
    pub retry_after_ms: Option<u64>,
    pub source: Option<String>,
}

impl RateLimitTelemetry {
    pub fn has_quota_headers(&self) -> bool {
        self.request_remaining.is_some()
            || self.request_limit.is_some()
            || self.token_remaining.is_some()
            || self.token_limit.is_some()
    }
}

#[derive(Debug, Clone)]
pub struct ProviderChatMeta {
    pub response: UnifiedChatResponse,
    pub status: u16,
    pub rate_limits: RateLimitTelemetry,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProviderChatError {
    pub status: Option<u16>,
    pub message: String,
    pub rate_limits: RateLimitTelemetry,
    pub kind: ProviderChatErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderChatErrorKind {
    HttpStatus,
    Transport,
    LocalRequest,
    InvalidResponse,
}

impl ProviderChatError {
    pub fn is_rate_limited(&self) -> bool {
        self.status == Some(429)
    }

    pub fn is_transient(&self) -> bool {
        if let Some(status) = self.status {
            return matches!(status, 408 | 429 | 499) || status >= 500;
        }
        matches!(self.kind, ProviderChatErrorKind::Transport)
    }
}

impl fmt::Display for ProviderChatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

struct JsonResponseMeta {
    raw: Value,
    status: u16,
    rate_limits: RateLimitTelemetry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogprobsRequestSpec {
    None,
    OpenaiChat,
    OpenaiResponses,
    Gemini,
}

impl RuntimeAdapter {
    pub fn new(client: Client, config: ProviderRuntimeConfig) -> Self {
        Self { client, config }
    }

    async fn headers(&self) -> Result<HeaderMap, String> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if self.config.protocol != ProviderProtocol::VertexAi {
            if let Some(credential) = self
                .config
                .credential
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                let name = HeaderName::from_bytes(self.config.auth_header.as_bytes())
                    .map_err(|error| format!("Invalid authentication header: {error}"))?;
                let value = if self.config.auth_type == "bearer" && name == AUTHORIZATION {
                    format!("Bearer {credential}")
                } else {
                    credential.to_string()
                };
                headers.insert(
                    name,
                    HeaderValue::from_str(&value)
                        .map_err(|error| format!("Invalid authentication value: {error}"))?,
                );
            }
        }
        for (name, value) in &self.config.custom_headers {
            let header_name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|error| format!("Invalid custom header {name}: {error}"))?;
            if self.config.protocol == ProviderProtocol::VertexAi && header_name == AUTHORIZATION {
                continue;
            }
            headers.insert(
                header_name,
                HeaderValue::from_str(value)
                    .map_err(|error| format!("Invalid custom header value for {name}: {error}"))?,
            );
        }
        if self.config.protocol == ProviderProtocol::VertexAi {
            let token = vertex_ai::access_token(&self.client, &self.config).await?;
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|error| format!("Invalid Agent Platform token: {error}"))?,
            );
        }
        if self.config.protocol == ProviderProtocol::Anthropic
            && !headers.contains_key("anthropic-version")
        {
            headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
        }
        Ok(headers)
    }

    fn endpoint(&self, suffix: &str) -> String {
        append_endpoint_suffix(&self.config.base_url, suffix)
    }

    fn openai_endpoint(&self, suffix: &str) -> String {
        let base = endpoint_base_url(&self.config.base_url).trim_end_matches('/');
        if self.config.use_raw_base_url || is_versioned_base_url(base) {
            append_endpoint_suffix(base, suffix)
        } else {
            append_endpoint_suffix(&format!("{base}/v1"), suffix)
        }
    }

    async fn request_json(
        &self,
        method: Method,
        url: String,
        body: Option<Value>,
    ) -> Result<Value, String> {
        self.request_json_with_meta(method, url, body)
            .await
            .map(|meta| meta.raw)
            .map_err(|error| error.to_string())
    }

    async fn request_json_with_meta(
        &self,
        method: Method,
        url: String,
        body: Option<Value>,
    ) -> Result<JsonResponseMeta, ProviderChatError> {
        let mut request = self
            .client
            .request(method, url)
            .headers(self.headers().await.map_err(|error| ProviderChatError {
                status: None,
                message: error,
                rate_limits: RateLimitTelemetry::default(),
                kind: ProviderChatErrorKind::LocalRequest,
            })?);
        if let Some(value) = body {
            request = request.json(&value);
        }
        let response = request.send().await.map_err(|error| ProviderChatError {
            status: None,
            message: error.to_string(),
            rate_limits: RateLimitTelemetry::default(),
            kind: ProviderChatErrorKind::Transport,
        })?;
        let status = response.status();
        let headers = response.headers().clone();
        let mut rate_limits = rate_limits_from_headers(&headers);
        let text = response.text().await.map_err(|error| ProviderChatError {
            status: Some(status.as_u16()),
            message: error.to_string(),
            rate_limits: rate_limits.clone(),
            kind: ProviderChatErrorKind::Transport,
        })?;
        if !status.is_success() {
            merge_retry_after_from_error_body(&mut rate_limits, &text);
            return Err(ProviderChatError {
                status: Some(status.as_u16()),
                message: format!("HTTP {}: {}", status.as_u16(), truncate(&text, 500)),
                rate_limits,
                kind: ProviderChatErrorKind::HttpStatus,
            });
        }
        let raw = serde_json::from_str(&text).map_err(|error| ProviderChatError {
            status: Some(status.as_u16()),
            message: format!("Invalid JSON response: {error}"),
            rate_limits: rate_limits.clone(),
            kind: ProviderChatErrorKind::InvalidResponse,
        })?;
        Ok(JsonResponseMeta {
            raw,
            status: status.as_u16(),
            rate_limits,
        })
    }

    async fn list_models_value(&self) -> Result<Value, String> {
        match self.config.protocol {
            ProviderProtocol::OpenaiChat | ProviderProtocol::OpenaiResponses => {
                self.request_json(Method::GET, self.openai_endpoint("models"), None)
                    .await
            }
            ProviderProtocol::Anthropic => {
                let suffix = if self.config.use_raw_base_url {
                    "models"
                } else {
                    "v1/models"
                };
                self.request_json(Method::GET, self.endpoint(suffix), None)
                    .await
            }
            ProviderProtocol::Gemini => {
                let suffix = if self.config.use_raw_base_url {
                    "models"
                } else {
                    "v1beta/models"
                };
                self.request_json(Method::GET, self.endpoint(suffix), None)
                    .await
            }
            ProviderProtocol::VertexAi => {
                let vertex = vertex_ai::runtime_config(&self.config)?;
                self.request_json(
                    Method::GET,
                    format!(
                        "{}?pageSize=100&listAllVersions=true",
                        vertex_ai::publisher_models_url(
                            &self.config.base_url,
                            &vertex.location,
                            "google"
                        )
                    ),
                    None,
                )
                .await
            }
            ProviderProtocol::Ollama => {
                let base = endpoint_base_url(&self.config.base_url).trim_end_matches('/');
                let url = if self.config.use_raw_base_url {
                    append_endpoint_suffix(base, "tags")
                } else if base.ends_with("/api") {
                    format!("{base}/tags")
                } else {
                    format!("{base}/api/tags")
                };
                self.request_json(Method::GET, url, None).await
            }
        }
    }
}

impl ProviderAdapter for RuntimeAdapter {
    async fn list_models(&self) -> Result<Vec<RemoteModel>, String> {
        let value = self.list_models_value().await?;
        let items = match self.config.protocol {
            ProviderProtocol::OpenaiChat
            | ProviderProtocol::OpenaiResponses
            | ProviderProtocol::Anthropic => value
                .get("data")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            ProviderProtocol::Gemini => value
                .get("models")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            ProviderProtocol::VertexAi => value
                .get("publisherModels")
                .or_else(|| value.get("models"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            ProviderProtocol::Ollama => value
                .get("models")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        };
        let mut models = Vec::new();
        for item in items {
            let mut request_name = item
                .get("id")
                .or_else(|| item.get("name"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if self.config.protocol == ProviderProtocol::VertexAi {
                request_name = vertex_ai::model_id(&request_name);
            }
            if request_name.is_empty() {
                continue;
            }
            if self.config.protocol == ProviderProtocol::VertexAi
                && !request_name.to_ascii_lowercase().starts_with("gemini")
            {
                continue;
            }
            let alias = item
                .get("display_name")
                .or_else(|| item.get("displayName"))
                .and_then(Value::as_str)
                .unwrap_or(&request_name)
                .to_string();
            models.push(RemoteModel {
                request_name,
                alias,
                added: false,
            });
        }
        models.sort_by(|a, b| a.request_name.cmp(&b.request_name));
        Ok(models)
    }

    fn build_chat_request(&self, request: &UnifiedChatRequest) -> Result<(String, Value), String> {
        let (url, body) = match self.config.protocol {
            ProviderProtocol::OpenaiChat => (
                self.openai_endpoint("chat/completions"),
                build_openai_chat_body(&self.config.base_url, request),
            ),
            ProviderProtocol::OpenaiResponses => (
                self.openai_endpoint("responses"),
                build_openai_responses_body(&self.config.base_url, request),
            ),
            ProviderProtocol::Anthropic => {
                let suffix = if self.config.use_raw_base_url {
                    "messages"
                } else {
                    "v1/messages"
                };
                (self.endpoint(suffix), build_anthropic_body(request))
            }
            ProviderProtocol::Gemini => (
                self.endpoint(&format!(
                    "{}models/{}:{}",
                    if self.config.use_raw_base_url {
                        ""
                    } else {
                        "v1beta/"
                    },
                    request.model.trim_start_matches("models/"),
                    if request.stream {
                        "streamGenerateContent?alt=sse"
                    } else {
                        "generateContent"
                    }
                )),
                build_gemini_body(request),
            ),
            ProviderProtocol::VertexAi => {
                let vertex = vertex_ai::runtime_config(&self.config)?;
                (
                    vertex_ai::generate_content_url(
                        &self.config.base_url,
                        &vertex.project_id,
                        &vertex.location,
                        &request.model,
                        request.stream,
                    ),
                    build_gemini_body(request),
                )
            }
            ProviderProtocol::Ollama => {
                let base = endpoint_base_url(&self.config.base_url).trim_end_matches('/');
                (
                    if self.config.use_raw_base_url {
                        append_endpoint_suffix(base, "chat")
                    } else if base.ends_with("/api") {
                        format!("{base}/chat")
                    } else {
                        format!("{base}/api/chat")
                    },
                    build_ollama_body(request),
                )
            }
        };
        let mut body = merge_custom_parameters(body, &request.custom_parameters)?;
        apply_request_overrides(&mut body, &self.config, request);
        Ok((url, body))
    }

    async fn send_chat(&self, request: &UnifiedChatRequest) -> Result<UnifiedChatResponse, String> {
        self.send_chat_with_meta(request)
            .await
            .map(|meta| meta.response)
            .map_err(|error| error.to_string())
    }

    async fn stream_chat(
        &self,
        request: &UnifiedChatRequest,
    ) -> Result<Vec<UnifiedChatResponse>, String> {
        let (url, body) = self.build_chat_request(request)?;
        let response = self
            .client
            .post(url)
            .headers(self.headers().await?)
            .json(&body)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.map_err(|error| error.to_string())?;
            return Err(format!(
                "HTTP {}: {}",
                status.as_u16(),
                truncate(&text, 500)
            ));
        }
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut output = Vec::new();
        while let Some(chunk) = stream.next().await {
            buffer.push_str(&String::from_utf8_lossy(
                &chunk.map_err(|error| error.to_string())?,
            ));
            while let Some(index) = buffer.find('\n') {
                let line = buffer[..index].trim().to_string();
                buffer.drain(..=index);
                let data = line.strip_prefix("data:").map(str::trim).unwrap_or(&line);
                if data.is_empty() || data == "[DONE]" || data.starts_with(':') {
                    continue;
                }
                if let Ok(raw) = serde_json::from_str::<Value>(data) {
                    if let Ok(part) = normalize_response(self.config.protocol, raw) {
                        output.push(part);
                    }
                }
            }
        }
        Ok(output)
    }
}

impl RuntimeAdapter {
    pub async fn send_chat_with_meta(
        &self,
        request: &UnifiedChatRequest,
    ) -> Result<ProviderChatMeta, ProviderChatError> {
        let (url, body) = self
            .build_chat_request(request)
            .map_err(|error| ProviderChatError {
                status: None,
                message: error,
                rate_limits: RateLimitTelemetry::default(),
                kind: ProviderChatErrorKind::LocalRequest,
            })?;
        let meta = self
            .request_json_with_meta(Method::POST, url, Some(body))
            .await?;
        let finish_reason = finish_reason_from_raw(self.config.protocol, &meta.raw);
        let response = normalize_response(self.config.protocol, meta.raw).map_err(|error| {
            ProviderChatError {
                status: Some(meta.status),
                message: error,
                rate_limits: meta.rate_limits.clone(),
                kind: ProviderChatErrorKind::InvalidResponse,
            }
        })?;
        Ok(ProviderChatMeta {
            response,
            status: meta.status,
            rate_limits: meta.rate_limits,
            finish_reason,
        })
    }
}

fn rate_limits_from_headers(headers: &HeaderMap) -> RateLimitTelemetry {
    let request_limit = header_u64(
        headers,
        &[
            "x-ratelimit-limit-requests",
            "anthropic-ratelimit-requests-limit",
        ],
    );
    let request_remaining = header_u64(
        headers,
        &[
            "x-ratelimit-remaining-requests",
            "anthropic-ratelimit-requests-remaining",
        ],
    );
    let token_limit = header_u64(
        headers,
        &[
            "x-ratelimit-limit-tokens",
            "anthropic-ratelimit-tokens-limit",
        ],
    );
    let token_remaining = header_u64(
        headers,
        &[
            "x-ratelimit-remaining-tokens",
            "anthropic-ratelimit-tokens-remaining",
        ],
    );
    let request_reset_ms = header_duration_ms(
        headers,
        &[
            "x-ratelimit-reset-requests",
            "anthropic-ratelimit-requests-reset",
        ],
    );
    let token_reset_ms = header_duration_ms(
        headers,
        &[
            "x-ratelimit-reset-tokens",
            "anthropic-ratelimit-tokens-reset",
        ],
    );
    let retry_after_ms = header_duration_ms(headers, &["retry-after"]);
    let source = if headers.get("anthropic-ratelimit-requests-limit").is_some()
        || headers.get("anthropic-ratelimit-tokens-limit").is_some()
    {
        Some("anthropic".to_string())
    } else if headers.get("x-ratelimit-limit-requests").is_some()
        || headers.get("x-ratelimit-limit-tokens").is_some()
    {
        Some("openai-compatible".to_string())
    } else {
        None
    };

    RateLimitTelemetry {
        request_limit,
        request_remaining,
        request_reset_ms,
        token_limit,
        token_remaining,
        token_reset_ms,
        retry_after_ms,
        source,
    }
}

fn merge_retry_after_from_error_body(rate_limits: &mut RateLimitTelemetry, text: &str) {
    if let Some(delay) = retry_after_ms_from_error_body(text) {
        rate_limits.retry_after_ms = Some(
            rate_limits
                .retry_after_ms
                .map_or(delay, |value| value.max(delay)),
        );
        if rate_limits.source.is_none() {
            rate_limits.source = Some("google-rpc".to_string());
        }
    }
}

fn retry_after_ms_from_error_body(text: &str) -> Option<u64> {
    let raw = serde_json::from_str::<Value>(text).ok()?;
    let details = raw
        .pointer("/error/details")
        .and_then(Value::as_array)
        .or_else(|| raw.pointer("/details").and_then(Value::as_array))?;
    details.iter().find_map(|detail| {
        let type_name = detail
            .get("@type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !type_name.ends_with("google.rpc.RetryInfo") {
            return None;
        }
        detail.get("retryDelay").and_then(parse_retry_delay_ms)
    })
}

fn parse_retry_delay_ms(value: &Value) -> Option<u64> {
    if let Some(text) = value.as_str() {
        return parse_duration_ms(text);
    }
    let seconds = value.get("seconds").and_then(Value::as_u64).unwrap_or(0);
    let nanos = value.get("nanos").and_then(Value::as_u64).unwrap_or(0);
    if seconds == 0 && nanos == 0 {
        None
    } else {
        Some(
            seconds
                .saturating_mul(1000)
                .saturating_add(nanos / 1_000_000),
        )
    }
}

fn header_text(headers: &HeaderMap, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        headers
            .get(*name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn header_u64(headers: &HeaderMap, names: &[&str]) -> Option<u64> {
    header_text(headers, names).and_then(|value| {
        value
            .split(',')
            .next()
            .unwrap_or(&value)
            .trim()
            .parse::<u64>()
            .ok()
    })
}

fn header_duration_ms(headers: &HeaderMap, names: &[&str]) -> Option<u64> {
    header_text(headers, names).and_then(|value| parse_duration_ms(&value))
}

fn parse_duration_ms(value: &str) -> Option<u64> {
    let trimmed = value.trim().trim_matches('"').to_ascii_lowercase();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(number) = trimmed.strip_suffix("ms") {
        return number
            .trim()
            .parse::<f64>()
            .ok()
            .map(|value| value.ceil() as u64);
    }
    if let Some(number) = trimmed.strip_suffix('s') {
        return number
            .trim()
            .parse::<f64>()
            .ok()
            .map(|value| (value * 1000.0).ceil() as u64);
    }
    if let Some(number) = trimmed.strip_suffix('m') {
        return number
            .trim()
            .parse::<f64>()
            .ok()
            .map(|value| (value * 60_000.0).ceil() as u64);
    }
    if let Some(number) = trimmed.strip_suffix('h') {
        return number
            .trim()
            .parse::<f64>()
            .ok()
            .map(|value| (value * 3_600_000.0).ceil() as u64);
    }
    trimmed
        .parse::<f64>()
        .ok()
        .map(|value| (value * 1000.0).ceil() as u64)
}

fn append_endpoint_suffix(base_url: &str, suffix: &str) -> String {
    let base = endpoint_base_url(base_url).trim_end_matches('/');
    let suffix = suffix.trim_start_matches('/');
    let suffix_path = suffix.split('?').next().unwrap_or(suffix);
    if base.ends_with(suffix) || (!suffix_path.is_empty() && base.ends_with(suffix_path)) {
        base.to_string()
    } else {
        format!("{base}/{suffix}")
    }
}

fn endpoint_base_url(base_url: &str) -> &str {
    base_url.split('#').next().unwrap_or(base_url).trim()
}

fn is_versioned_base_url(base_url: &str) -> bool {
    url::Url::parse(base_url)
        .ok()
        .and_then(|url| {
            url.path_segments()
                .and_then(|mut segments| segments.next_back().map(str::to_string))
        })
        .is_some_and(|last| {
            let bytes = last.as_bytes();
            bytes.len() >= 2 && bytes[0] == b'v' && bytes[1..].iter().all(u8::is_ascii_digit)
        })
}

fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

const PROTECTED_CUSTOM_PARAMETER_KEYS: &[&str] = &[
    "model",
    "messages",
    "message",
    "input",
    "instructions",
    "contents",
    "system",
    "systemInstruction",
    "system_instruction",
    "system_prompt",
    "systemPrompt",
    "prompt",
    "tools",
    "insituTools",
    "tool_choice",
    "toolChoice",
    "toolConfig",
    "tool_config",
    "stream",
    "stream_options",
    "streamOptions",
];

fn is_protected_custom_parameter_key(key: &str) -> bool {
    PROTECTED_CUSTOM_PARAMETER_KEYS.contains(&key)
}

fn merge_custom_parameters(mut body: Value, custom_parameters: &Value) -> Result<Value, String> {
    if custom_parameters.is_null() {
        return Ok(body);
    }
    let Some(custom) = custom_parameters.as_object() else {
        return Err("Custom request body parameters must be a JSON object".into());
    };
    if custom.is_empty() {
        return Ok(body);
    }
    let Some(body_object) = body.as_object_mut() else {
        return Err("Provider request body must be a JSON object".into());
    };
    let sanitized = custom
        .iter()
        .filter(|(key, _)| !is_protected_custom_parameter_key(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    if sanitized.is_empty() {
        return Ok(body);
    }
    deep_merge_object(body_object, &sanitized);
    Ok(body)
}

fn apply_request_overrides(
    body: &mut Value,
    config: &ProviderRuntimeConfig,
    request: &UnifiedChatRequest,
) {
    if !request.logprobs {
        disable_logprobs_request_params(body, config.protocol);
        return;
    }

    match logprobs_request_spec(config.protocol, &config.base_url) {
        LogprobsRequestSpec::OpenaiChat => {
            body["logprobs"] = json!(true);
        }
        LogprobsRequestSpec::OpenaiResponses => enable_openai_response_logprobs(body),
        LogprobsRequestSpec::Gemini => enable_gemini_logprobs(body),
        LogprobsRequestSpec::None => disable_logprobs_request_params(body, config.protocol),
    }
}

fn logprobs_request_spec(protocol: ProviderProtocol, base_url: &str) -> LogprobsRequestSpec {
    match protocol {
        ProviderProtocol::OpenaiChat => {
            if openai_chat_logprobs_supported(base_url) {
                LogprobsRequestSpec::OpenaiChat
            } else {
                LogprobsRequestSpec::None
            }
        }
        ProviderProtocol::OpenaiResponses => LogprobsRequestSpec::OpenaiResponses,
        ProviderProtocol::Gemini | ProviderProtocol::VertexAi => LogprobsRequestSpec::Gemini,
        ProviderProtocol::Anthropic | ProviderProtocol::Ollama => LogprobsRequestSpec::None,
    }
}

fn openai_chat_logprobs_supported(base_url: &str) -> bool {
    let host = provider_host(base_url);
    if host.is_empty() {
        return true;
    }
    !(host.contains("dashscope") && host.ends_with("aliyuncs.com"))
        && !host.ends_with("modelscope.cn")
        && !host.contains(".modelscope.cn")
}

fn provider_host(base_url: &str) -> String {
    url::Url::parse(endpoint_base_url(base_url))
        .ok()
        .and_then(|url| url.host_str().map(|host| host.to_ascii_lowercase()))
        .unwrap_or_default()
}

fn enable_openai_response_logprobs(body: &mut Value) {
    const INCLUDE: &str = "message.output_text.logprobs";
    let Some(object) = body.as_object_mut() else {
        return;
    };
    match object.get_mut("include") {
        Some(Value::Array(items)) => {
            if !items.iter().any(|item| item.as_str() == Some(INCLUDE)) {
                items.push(json!(INCLUDE));
            }
        }
        Some(value @ Value::String(_)) => {
            let existing = value.as_str().unwrap_or_default().to_string();
            if existing == INCLUDE {
                return;
            }
            *value = json!([existing, INCLUDE]);
        }
        Some(value) if value.is_null() => {
            *value = json!([INCLUDE]);
        }
        Some(_) => {}
        None => {
            object.insert("include".into(), json!([INCLUDE]));
        }
    }
}

fn enable_gemini_logprobs(body: &mut Value) {
    if !body.get("generationConfig").is_some_and(Value::is_object) {
        body["generationConfig"] = json!({});
    }
    if let Some(generation) = body
        .get_mut("generationConfig")
        .and_then(Value::as_object_mut)
    {
        generation.insert("responseLogprobs".into(), json!(true));
    }
}

fn disable_logprobs_request_params(body: &mut Value, protocol: ProviderProtocol) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    object.remove("logprobs");
    object.remove("top_logprobs");
    if protocol == ProviderProtocol::OpenaiResponses {
        let remove_include = if let Some(Value::Array(items)) = object.get_mut("include") {
            items.retain(|item| item.as_str() != Some("message.output_text.logprobs"));
            items.is_empty()
        } else {
            false
        };
        if remove_include {
            object.remove("include");
        }
    }
    if matches!(
        protocol,
        ProviderProtocol::Gemini | ProviderProtocol::VertexAi
    ) {
        if let Some(generation) = object
            .get_mut("generationConfig")
            .and_then(Value::as_object_mut)
        {
            generation.remove("responseLogprobs");
            generation.remove("logprobs");
        }
    }
}

fn deep_merge_object(
    target: &mut serde_json::Map<String, Value>,
    incoming: &serde_json::Map<String, Value>,
) {
    for (key, value) in incoming {
        match (target.get_mut(key), value) {
            (Some(Value::Object(target_object)), Value::Object(incoming_object)) => {
                deep_merge_object(target_object, incoming_object);
            }
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

fn content_text(parts: &[UnifiedContent]) -> String {
    parts
        .iter()
        .filter_map(|part| match part {
            UnifiedContent::Text { text } | UnifiedContent::CacheableText { text } => {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_plain_openai_text_part(part: &Value) -> bool {
    part.get("type").and_then(Value::as_str) == Some("text")
        && part.get("text").and_then(Value::as_str).is_some()
        && part.get("cache_control").is_none()
}

fn openai_messages(
    messages: &[UnifiedMessage],
    cache_control: bool,
    base_url: &str,
    model: &str,
) -> Vec<Value> {
    let mut output = Vec::new();
    for message in messages {
        let mut text_parts = Vec::new();
        let mut reasoning_texts = Vec::new();
        let mut reasoning_details = Vec::new();
        for content in &message.content {
            match content {
                UnifiedContent::Text { text } => {
                    text_parts.push(json!({"type": "text", "text": text}))
                }
                UnifiedContent::CacheableText { text } => {
                    let mut part = json!({"type": "text", "text": text});
                    if cache_control {
                        part["cache_control"] = json!({"type": "ephemeral"});
                    }
                    text_parts.push(part);
                }
                UnifiedContent::Image { media_type, data } => text_parts.push(json!({
                    "type": "image_url",
                    "image_url": {"url": format!("data:{media_type};base64,{data}")}
                })),
                UnifiedContent::Thinking {
                    text,
                    signature,
                    encrypted_data,
                } => {
                    if message.role == "assistant" {
                        if is_feature_supported(FeatureId::OpenAiReasoningDetails, base_url, model)
                        {
                            if let Some(data) = encrypted_data {
                                reasoning_details.push(json!({
                                    "type": "reasoning.encrypted",
                                    "data": data
                                }));
                            } else if !text.is_empty() {
                                let mut detail = json!({
                                    "type": "reasoning.text",
                                    "text": text
                                });
                                if let Some(signature) =
                                    signature.as_deref().filter(|value| !value.is_empty())
                                {
                                    detail["signature"] = json!(signature);
                                }
                                reasoning_details.push(detail);
                            }
                        } else if !text.is_empty() {
                            reasoning_texts.push(text.clone());
                        }
                    }
                }
            }
        }
        let supports_reasoning_text =
            is_feature_supported(FeatureId::OpenAiReasoningField, base_url, model)
                || is_feature_supported(FeatureId::OpenAiReasoningContent, base_url, model);
        if !text_parts.is_empty()
            || (!reasoning_texts.is_empty() && supports_reasoning_text)
            || !reasoning_details.is_empty()
        {
            let content = if text_parts.is_empty() {
                Value::String(String::new())
            } else if text_parts.iter().all(is_plain_openai_text_part) {
                Value::String(
                    text_parts
                        .iter()
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                )
            } else {
                Value::Array(text_parts)
            };
            let mut item = json!({
                "role": message.role,
                "content": content
            });
            if !reasoning_details.is_empty() {
                item["reasoning_details"] = Value::Array(reasoning_details);
            } else if !reasoning_texts.is_empty() {
                let reasoning = reasoning_texts.join("");
                if is_feature_supported(FeatureId::OpenAiReasoningField, base_url, model) {
                    item["reasoning"] = json!(reasoning);
                } else if is_feature_supported(FeatureId::OpenAiReasoningContent, base_url, model) {
                    item["reasoning_content"] = json!(reasoning);
                }
            }
            output.push(item);
        }
    }
    output
}

pub fn build_openai_chat_body(base_url: &str, request: &UnifiedChatRequest) -> Value {
    let cache_control =
        is_feature_supported(FeatureId::OpenAiCacheControl, base_url, &request.model);
    let mut body = json!({
        "model": request.model,
        "messages": openai_messages(
            &request.messages,
            cache_control,
            base_url,
            &request.model
        ),
        "stream": request.stream
    });
    if request.stream {
        body["stream_options"] = json!({"include_usage": true});
    }
    if let Some(tokens) = request.max_output_tokens {
        if is_feature_supported(
            FeatureId::OpenAiOnlyMaxCompletionTokens,
            base_url,
            &request.model,
        ) {
            body["max_completion_tokens"] = json!(tokens);
        } else if is_feature_supported(FeatureId::OpenAiOnlyMaxTokens, base_url, &request.model) {
            body["max_tokens"] = json!(tokens);
        } else {
            body["max_tokens"] = json!(tokens);
            body["max_completion_tokens"] = json!(tokens);
        }
    }
    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if request.logprobs && openai_chat_logprobs_supported(base_url) {
        body["logprobs"] = json!(true);
    }
    if let Some(thinking) = &request.thinking {
        merge_object(
            &mut body,
            build_openai_reasoning_params(
                base_url,
                &request.model,
                thinking,
                request.max_output_tokens,
            ),
        );
    }
    if is_feature_supported(FeatureId::OpenAiClearThinking, base_url, &request.model) {
        body["clear_thinking"] = json!(false);
    }
    if is_feature_supported(FeatureId::OpenAiReasoningSplit, base_url, &request.model) {
        body["reasoning_split"] = json!(true);
    }
    body
}

pub fn build_openai_reasoning_params(
    base_url: &str,
    model: &str,
    thinking: &ThinkingConfig,
    max_output_tokens: Option<u32>,
) -> Value {
    let disabled = thinking.mode == ThinkingMode::Disabled
        || thinking.effort == Some(ThinkingEffort::None)
        || thinking.budget_tokens == Some(0);
    let mut out = json!({});
    if is_feature_supported(FeatureId::OpenAiReasoningObject, base_url, model) {
        out["reasoning"] = if disabled {
            json!({"enabled": false})
        } else if let Some(tokens) = thinking.budget_tokens {
            json!({"max_tokens": max_output_tokens.map(|max| tokens.min(max.saturating_sub(1))).unwrap_or(tokens)})
        } else if let Some(effort) = thinking.effort {
            json!({"effort": openai_effort(effort)})
        } else {
            json!({"enabled": true})
        };
    } else if is_feature_supported(FeatureId::OpenAiThinkingObject, base_url, model) {
        out["thinking"] = json!({"type": if disabled { "disabled" } else { "enabled" }});
        if is_feature_supported(FeatureId::OpenAiDeepSeekReasoningEffort, base_url, model)
            && !disabled
        {
            if let Some(effort) = thinking.effort {
                out["reasoning_effort"] = json!(if matches!(
                    effort,
                    ThinkingEffort::Max | ThinkingEffort::Xhigh
                ) {
                    "max"
                } else {
                    "high"
                });
            }
        } else if is_feature_supported(FeatureId::OpenAiReasoningEffort, base_url, model) {
            out["reasoning_effort"] = json!(thinking.effort.map(openai_effort).unwrap_or("medium"));
        }
    } else if is_feature_supported(FeatureId::OpenAiDisableReasoning, base_url, model) {
        out["disable_reasoning"] = json!(disabled);
    } else if is_feature_supported(FeatureId::OpenAiEnableThinking, base_url, model) {
        out["enable_thinking"] = json!(!disabled);
        if !disabled && is_feature_supported(FeatureId::OpenAiThinkingBudget, base_url, model) {
            if let Some(tokens) = thinking.budget_tokens {
                out["thinking_budget"] = json!(tokens);
            }
        }
    } else {
        out["reasoning_effort"] = json!(if disabled {
            "none"
        } else {
            thinking.effort.map(openai_effort).unwrap_or("medium")
        });
    }
    if is_feature_supported(FeatureId::OpenAiThinkingStrategy, base_url, model) && !disabled {
        if let Some(summary) = thinking.summary {
            match summary {
                ThinkingSummary::Concise => out["thinking_strategy"] = json!("short_think"),
                ThinkingSummary::Detailed => out["thinking_strategy"] = json!("chain_of_draft"),
                ThinkingSummary::None | ThinkingSummary::Auto => {}
            }
        }
    }
    out
}

fn openai_effort(effort: ThinkingEffort) -> &'static str {
    match effort {
        ThinkingEffort::None => "none",
        ThinkingEffort::Minimal => "minimal",
        ThinkingEffort::Low => "low",
        ThinkingEffort::Medium => "medium",
        ThinkingEffort::High => "high",
        ThinkingEffort::Xhigh | ThinkingEffort::Max => "xhigh",
    }
}

fn merge_object(target: &mut Value, source: Value) {
    if let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
}

fn push_responses_message(output: &mut Vec<Value>, role: &str, content: &mut Vec<Value>) {
    if content.is_empty() {
        return;
    }
    output.push(json!({
        "role": role,
        "content": std::mem::take(content)
    }));
}

fn openai_responses_input(messages: &[UnifiedMessage]) -> Vec<Value> {
    let mut output = Vec::new();
    for (message_index, message) in messages.iter().enumerate() {
        let role = match message.role.as_str() {
            "assistant" => "assistant",
            "system" => "system",
            _ => "user",
        };
        let mut content = Vec::new();
        for (part_index, part) in message.content.iter().enumerate() {
            match part {
                UnifiedContent::Text { text } | UnifiedContent::CacheableText { text } => {
                    if text.trim().is_empty() {
                        continue;
                    }
                    content.push(json!({
                        "type": if role == "assistant" { "output_text" } else { "input_text" },
                        "text": text
                    }));
                }
                UnifiedContent::Image { media_type, data } => {
                    content.push(json!({
                        "type": "input_image",
                        "image_url": format!("data:{media_type};base64,{data}")
                    }));
                }
                UnifiedContent::Thinking {
                    text,
                    signature,
                    encrypted_data,
                } => {
                    push_responses_message(&mut output, role, &mut content);
                    let id = signature
                        .as_deref()
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("reasoning_{message_index}_{part_index}"));
                    if let Some(data) = encrypted_data {
                        output.push(json!({
                            "type": "reasoning",
                            "id": id,
                            "summary": [],
                            "encrypted_content": data
                        }));
                    } else if !text.is_empty() {
                        output.push(json!({
                            "type": "reasoning",
                            "id": id,
                            "summary": [],
                            "content": [{"type": "reasoning_text", "text": text}]
                        }));
                    }
                }
            }
        }
        push_responses_message(&mut output, role, &mut content);
    }
    output
}

pub fn build_openai_responses_body(base_url: &str, request: &UnifiedChatRequest) -> Value {
    let mut body = json!({
        "model": request.model,
        "input": openai_responses_input(&request.messages),
        "stream": request.stream
    });
    if let Some(tokens) = request.max_output_tokens {
        body["max_output_tokens"] = json!(tokens);
    }
    if let Some(thinking) = &request.thinking {
        let disabled = thinking.mode == ThinkingMode::Disabled
            || thinking.effort == Some(ThinkingEffort::None);
        body["reasoning"] = json!({
            "effort": if disabled { "none" } else { thinking.effort.map(openai_effort).unwrap_or("medium") },
            "summary": thinking.summary.map(|summary| match summary {
                ThinkingSummary::None => "none",
                ThinkingSummary::Auto => "auto",
                ThinkingSummary::Concise => "concise",
                ThinkingSummary::Detailed => "detailed",
            }).unwrap_or("auto")
        });
        if is_feature_supported(FeatureId::OpenAiThinkingObject, base_url, &request.model) {
            body["thinking"] = json!({"type": if disabled { "disabled" } else { "enabled" }});
        }
    }
    if request.web_search {
        body["tools"] = json!([{"type": "web_search"}]);
    }
    if request.logprobs {
        enable_openai_response_logprobs(&mut body);
    }
    body
}

fn anthropic_content(message: &UnifiedMessage) -> Vec<Value> {
    message
        .content
        .iter()
        .map(|part| match part {
            UnifiedContent::Text { text } => json!({"type": "text", "text": text}),
            UnifiedContent::CacheableText { text } => json!({
                "type": "text",
                "text": text,
                "cache_control": {"type": "ephemeral"}
            }),
            UnifiedContent::Image { media_type, data } => json!({
                "type": "image", "source": {"type": "base64", "media_type": media_type, "data": data}
            }),
            UnifiedContent::Thinking { text, signature, encrypted_data } => {
                if let Some(data) = encrypted_data {
                    json!({"type": "redacted_thinking", "data": data})
                } else {
                    json!({"type": "thinking", "thinking": text, "signature": signature.clone().unwrap_or_default()})
                }
            }
        })
        .collect()
}

pub fn ensure_anthropic_alternating_roles(messages: Vec<Value>) -> Result<Vec<Value>, String> {
    let mut output: Vec<Value> = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if output
            .last()
            .and_then(|item| item.get("role"))
            .and_then(Value::as_str)
            == Some(role)
        {
            let incoming = message
                .get("content")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(parts) = output
                .last_mut()
                .and_then(|item| item.get_mut("content"))
                .and_then(Value::as_array_mut)
            {
                parts.extend(incoming);
            }
        } else {
            output.push(message);
        }
    }
    if output
        .first()
        .and_then(|item| item.get("role"))
        .and_then(Value::as_str)
        != Some("user")
    {
        return Err("The first Anthropic message must have the user role".into());
    }
    Ok(output)
}

pub fn build_anthropic_body(request: &UnifiedChatRequest) -> Value {
    let mut system = Vec::new();
    let mut messages = Vec::new();
    for message in &request.messages {
        if message.role == "system" {
            system.extend(anthropic_content(message));
        } else {
            messages.push(json!({"role": message.role, "content": anthropic_content(message)}));
        }
    }
    let messages = ensure_anthropic_alternating_roles(messages).unwrap_or_default();
    let thinking_enabled = request.thinking.as_ref().is_some_and(|thinking| {
        thinking.mode != ThinkingMode::Disabled && thinking.effort != Some(ThinkingEffort::None)
    });
    let mut body = json!({
        "model": request.model,
        "messages": messages,
        "max_tokens": request.max_output_tokens.unwrap_or(4096),
        "stream": request.stream
    });
    if !system.is_empty() {
        body["system"] = Value::Array(system);
    }
    if let Some(thinking) = &request.thinking {
        body["thinking"] = if thinking_enabled {
            let max = request.max_output_tokens.unwrap_or(4096);
            let budget = thinking
                .budget_tokens
                .unwrap_or(1024)
                .max(1024)
                .min(max.saturating_sub(1));
            json!({"type": "enabled", "budget_tokens": budget})
        } else {
            json!({"type": "disabled"})
        };
    }
    if request.web_search {
        body["tools"] = json!([{"type": "web_search_20250305", "name": "web_search"}]);
    }
    body
}

pub fn build_gemini_body(request: &UnifiedChatRequest) -> Value {
    let mut system_parts = Vec::new();
    let mut contents = Vec::new();
    for message in &request.messages {
        let parts: Vec<Value> = message
            .content
            .iter()
            .map(|part| match part {
                UnifiedContent::Text { text } => json!({"text": text}),
                UnifiedContent::CacheableText { text } => json!({"text": text}),
                UnifiedContent::Image { media_type, data } => {
                    json!({"inlineData": {"mimeType": media_type, "data": data}})
                }
                UnifiedContent::Thinking {
                    text, signature, ..
                } => {
                    let mut part = json!({"text": text, "thought": true});
                    if let Some(signature) = signature.as_deref().filter(|value| !value.is_empty())
                    {
                        part["thoughtSignature"] = json!(signature);
                    }
                    part
                }
            })
            .collect();
        if message.role == "system" {
            system_parts.extend(parts);
        } else if !parts.is_empty() {
            contents.push(json!({"role": if message.role == "assistant" { "model" } else { "user" }, "parts": parts}));
        }
    }
    let mut generation = json!({});
    if let Some(tokens) = request.max_output_tokens {
        generation["maxOutputTokens"] = json!(tokens);
    }
    if let Some(temperature) = request.temperature {
        generation["temperature"] = json!(temperature);
    }
    if let Some(thinking) = &request.thinking {
        let enabled = thinking.mode != ThinkingMode::Disabled
            && thinking.effort != Some(ThinkingEffort::None);
        generation["thinkingConfig"] = if is_feature_supported(
            FeatureId::GeminiThinkingLevel,
            "",
            &request.model,
        ) {
            json!({
                "includeThoughts": false,
                "thinkingLevel": if enabled {
                    gemini_thinking_level(thinking.effort)
                } else {
                    "minimal"
                }
            })
        } else {
            json!({
                "includeThoughts": false,
                "thinkingBudget": if enabled { thinking.budget_tokens.map(Value::from).unwrap_or(json!(-1)) } else { json!(0) }
            })
        };
    }
    if request.logprobs {
        generation["responseLogprobs"] = json!(true);
    }
    let mut body = json!({"contents": contents, "generationConfig": generation});
    if !system_parts.is_empty() {
        body["systemInstruction"] = json!({"parts": system_parts});
    }
    if request.web_search {
        body["tools"] = json!([{"googleSearch": {}}]);
    }
    body
}

fn gemini_thinking_level(effort: Option<ThinkingEffort>) -> &'static str {
    match effort.unwrap_or(ThinkingEffort::Medium) {
        ThinkingEffort::None | ThinkingEffort::Minimal => "minimal",
        ThinkingEffort::Low => "low",
        ThinkingEffort::Medium => "medium",
        ThinkingEffort::High | ThinkingEffort::Xhigh | ThinkingEffort::Max => "high",
    }
}

pub fn build_ollama_body(request: &UnifiedChatRequest) -> Value {
    let messages: Vec<Value> = request
        .messages
        .iter()
        .map(|message| {
            let mut item = json!({"role": message.role, "content": content_text(&message.content)});
            let thinking = message
                .content
                .iter()
                .filter_map(|part| match part {
                    UnifiedContent::Thinking { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            if !thinking.is_empty() && message.role == "assistant" {
                item["thinking"] = json!(thinking);
            }
            item
        })
        .collect();
    let mut body = json!({"model": request.model, "messages": messages, "stream": request.stream});
    if let Some(thinking) = &request.thinking {
        body["think"] = match thinking.effort {
            Some(ThinkingEffort::None) => json!(false),
            Some(ThinkingEffort::Minimal | ThinkingEffort::Low) => json!("low"),
            Some(ThinkingEffort::Medium) => json!("medium"),
            Some(ThinkingEffort::High | ThinkingEffort::Xhigh | ThinkingEffort::Max) => {
                json!("high")
            }
            None => json!(thinking.mode != ThinkingMode::Disabled),
        };
    }
    let mut options = json!({});
    if let Some(tokens) = request.max_output_tokens {
        options["num_predict"] = json!(tokens);
    }
    if let Some(temperature) = request.temperature {
        options["temperature"] = json!(temperature);
    }
    if options.as_object().is_some_and(|object| !object.is_empty()) {
        body["options"] = options;
    }
    body
}

fn push_thinking_text(
    reasoning: &mut String,
    thinking: &mut Vec<UnifiedContent>,
    text: &str,
    signature: Option<String>,
) {
    if text.is_empty() {
        return;
    }
    reasoning.push_str(text);
    thinking.push(UnifiedContent::Thinking {
        text: text.to_string(),
        signature,
        encrypted_data: None,
    });
}

fn push_encrypted_thinking(thinking: &mut Vec<UnifiedContent>, encrypted_data: &str) {
    if encrypted_data.is_empty() {
        return;
    }
    thinking.push(UnifiedContent::Thinking {
        text: String::new(),
        signature: None,
        encrypted_data: Some(encrypted_data.to_string()),
    });
}

fn strip_leading_inline_thinking(
    text: &mut String,
    reasoning: &mut String,
    thinking: &mut Vec<UnifiedContent>,
) {
    let trimmed = text.trim_start();
    let leading_whitespace = text.len() - trimmed.len();
    let lower = trimmed.to_ascii_lowercase();
    let Some((open_tag, close_tag)) = [("<thinking>", "</thinking>"), ("<think>", "</think>")]
        .into_iter()
        .find(|(open, _)| lower.starts_with(open))
    else {
        return;
    };

    let content_start = leading_whitespace + open_tag.len();
    let after_open = &text[content_start..];
    let after_open_lower = after_open.to_ascii_lowercase();
    let Some(close_start_relative) = after_open_lower.find(close_tag) else {
        let thinking_text = after_open.to_string();
        push_thinking_text(reasoning, thinking, thinking_text.trim(), None);
        text.clear();
        return;
    };

    let close_end = content_start + close_start_relative + close_tag.len();
    let thinking_text = text[content_start..content_start + close_start_relative].to_string();
    let remaining = text[close_end..].trim_start().to_string();
    push_thinking_text(reasoning, thinking, thinking_text.trim(), None);
    *text = remaining;
}

fn append_openai_reasoning_details(
    value: Option<&Value>,
    reasoning: &mut String,
    thinking: &mut Vec<UnifiedContent>,
) {
    let Some(details) = value.and_then(Value::as_array) else {
        return;
    };
    for detail in details {
        match detail.get("type").and_then(Value::as_str) {
            Some("reasoning.encrypted") => {
                if let Some(data) = detail
                    .get("data")
                    .or_else(|| detail.get("encrypted_content"))
                    .and_then(Value::as_str)
                {
                    push_encrypted_thinking(thinking, data);
                }
            }
            Some("reasoning.text") | Some("reasoning.summary") => {
                if let Some(text) = detail.get("text").and_then(Value::as_str) {
                    let signature = detail
                        .get("signature")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    push_thinking_text(reasoning, thinking, text, signature);
                }
            }
            _ => {
                if let Some(text) = detail.get("text").and_then(Value::as_str) {
                    push_thinking_text(reasoning, thinking, text, None);
                }
            }
        }
    }
}

fn append_responses_reasoning_item(
    item: &Value,
    reasoning: &mut String,
    thinking: &mut Vec<UnifiedContent>,
) {
    if let Some(data) = item.get("encrypted_content").and_then(Value::as_str) {
        push_encrypted_thinking(thinking, data);
    }
    for key in ["summary", "content"] {
        if let Some(parts) = item.get(key).and_then(Value::as_array) {
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    push_thinking_text(reasoning, thinking, text, None);
                }
            }
        }
    }
}

fn append_responses_output_item(
    item: &Value,
    text: &mut String,
    reasoning: &mut String,
    thinking: &mut Vec<UnifiedContent>,
) {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => {
            if let Some(content) = item.get("content").and_then(Value::as_array) {
                for part in content {
                    match part.get("type").and_then(Value::as_str) {
                        Some("output_text") | Some("refusal") => text
                            .push_str(part.get("text").and_then(Value::as_str).unwrap_or_default()),
                        Some("reasoning_text") | Some("summary_text") => push_thinking_text(
                            reasoning,
                            thinking,
                            part.get("text").and_then(Value::as_str).unwrap_or_default(),
                            None,
                        ),
                        _ => {}
                    }
                }
            }
        }
        Some("reasoning") => append_responses_reasoning_item(item, reasoning, thinking),
        _ => {}
    }
}

pub fn finish_reason_from_raw(protocol: ProviderProtocol, raw: &Value) -> Option<String> {
    match protocol {
        ProviderProtocol::OpenaiChat => raw
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
            .map(str::to_string),
        ProviderProtocol::OpenaiResponses => raw
            .pointer("/incomplete_details/reason")
            .or_else(|| raw.pointer("/response/incomplete_details/reason"))
            .or_else(|| raw.get("status"))
            .and_then(Value::as_str)
            .map(str::to_string),
        ProviderProtocol::Anthropic => raw
            .get("stop_reason")
            .and_then(Value::as_str)
            .map(str::to_string),
        ProviderProtocol::Gemini => raw
            .pointer("/candidates/0/finishReason")
            .and_then(Value::as_str)
            .map(str::to_string),
        ProviderProtocol::VertexAi => raw
            .pointer("/candidates/0/finishReason")
            .and_then(Value::as_str)
            .map(str::to_string),
        ProviderProtocol::Ollama => raw
            .get("done_reason")
            .or_else(|| raw.get("doneReason"))
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

pub fn finish_reason_is_truncation(reason: Option<&str>) -> bool {
    reason
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "length"
                    | "max_tokens"
                    | "max_output_tokens"
                    | "max_tokens_reached"
                    | "model_context_window_exceeded"
                    | "incomplete"
            )
        })
        .unwrap_or(false)
}

pub fn normalize_response(
    protocol: ProviderProtocol,
    mut raw: Value,
) -> Result<UnifiedChatResponse, String> {
    if raw.get("choices").is_none() {
        if let Some(data) = raw
            .get("data")
            .filter(|data| data.get("choices").is_some())
            .cloned()
        {
            raw = data;
        }
    }
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut thinking = Vec::new();
    let usage;
    match protocol {
        ProviderProtocol::OpenaiChat => {
            let message = raw
                .pointer("/choices/0/message")
                .or_else(|| raw.pointer("/choices/0/delta"))
                .cloned()
                .unwrap_or(json!({}));
            text = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            reasoning = message
                .get("reasoning_content")
                .or_else(|| message.get("reasoning"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if !reasoning.is_empty() {
                thinking.push(UnifiedContent::Thinking {
                    text: reasoning.clone(),
                    signature: None,
                    encrypted_data: None,
                });
            }
            append_openai_reasoning_details(
                message.get("reasoning_details"),
                &mut reasoning,
                &mut thinking,
            );
            usage = usage_from_openai(raw.get("usage"));
        }
        ProviderProtocol::OpenaiResponses => {
            if let Some(event_type) = raw.get("type").and_then(Value::as_str) {
                match event_type {
                    "response.output_text.delta" | "response.refusal.delta" => {
                        text.push_str(raw.get("delta").and_then(Value::as_str).unwrap_or_default());
                    }
                    "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
                        push_thinking_text(
                            &mut reasoning,
                            &mut thinking,
                            raw.get("delta").and_then(Value::as_str).unwrap_or_default(),
                            None,
                        );
                    }
                    "response.output_item.done" => {
                        if let Some(item) = raw.get("item") {
                            append_responses_output_item(
                                item,
                                &mut text,
                                &mut reasoning,
                                &mut thinking,
                            );
                        }
                    }
                    "response.completed" => {
                        if let Some(response) = raw.get("response") {
                            if let Some(output) = response.get("output").and_then(Value::as_array) {
                                for item in output {
                                    append_responses_output_item(
                                        item,
                                        &mut text,
                                        &mut reasoning,
                                        &mut thinking,
                                    );
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            if let Some(output) = raw.get("output").and_then(Value::as_array) {
                for item in output {
                    append_responses_output_item(item, &mut text, &mut reasoning, &mut thinking);
                }
            }
            text.push_str(raw.get("delta").and_then(Value::as_str).unwrap_or_default());
            usage = raw
                .get("response")
                .and_then(|response| usage_from_openai(response.get("usage")))
                .or_else(|| usage_from_openai(raw.get("usage")));
        }
        ProviderProtocol::Anthropic => {
            if let Some(content) = raw.get("content").and_then(Value::as_array) {
                for part in content {
                    match part.get("type").and_then(Value::as_str) {
                        Some("text") => text
                            .push_str(part.get("text").and_then(Value::as_str).unwrap_or_default()),
                        Some("thinking") => push_thinking_text(
                            &mut reasoning,
                            &mut thinking,
                            part.get("thinking")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                            part.get("signature")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                        ),
                        Some("redacted_thinking") => {
                            if let Some(data) = part.get("data").and_then(Value::as_str) {
                                push_encrypted_thinking(&mut thinking, data);
                            }
                        }
                        _ => {}
                    }
                }
            }
            if let Some(delta) = raw.get("delta") {
                text.push_str(
                    delta
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                );
                push_thinking_text(
                    &mut reasoning,
                    &mut thinking,
                    delta
                        .get("thinking")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    None,
                );
            }
            usage = raw.get("usage").map(|value| UnifiedUsage {
                input_tokens: value
                    .get("input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                output_tokens: value
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                cached_tokens: value
                    .get("cache_read_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            });
        }
        ProviderProtocol::Gemini | ProviderProtocol::VertexAi => {
            if let Some(parts) = raw
                .pointer("/candidates/0/content/parts")
                .and_then(Value::as_array)
            {
                for part in parts {
                    if part.get("thought").and_then(Value::as_bool) == Some(true) {
                        push_thinking_text(
                            &mut reasoning,
                            &mut thinking,
                            part.get("text").and_then(Value::as_str).unwrap_or_default(),
                            part.get("thoughtSignature")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                        );
                    } else {
                        text.push_str(part.get("text").and_then(Value::as_str).unwrap_or_default());
                    }
                }
            }
            usage = raw.get("usageMetadata").map(|value| UnifiedUsage {
                input_tokens: value
                    .get("promptTokenCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                output_tokens: value
                    .get("candidatesTokenCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
                    + value
                        .get("thoughtsTokenCount")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                cached_tokens: value
                    .get("cachedContentTokenCount")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            });
        }
        ProviderProtocol::Ollama => {
            text = raw
                .pointer("/message/content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            reasoning = raw
                .pointer("/message/thinking")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if !reasoning.is_empty() {
                thinking.push(UnifiedContent::Thinking {
                    text: reasoning.clone(),
                    signature: None,
                    encrypted_data: None,
                });
            }
            usage = Some(UnifiedUsage {
                input_tokens: raw
                    .get("prompt_eval_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                output_tokens: raw.get("eval_count").and_then(Value::as_u64).unwrap_or(0),
                cached_tokens: 0,
            });
        }
    }
    strip_leading_inline_thinking(&mut text, &mut reasoning, &mut thinking);
    let logprob_stats = logprob_stats_from_raw(protocol, &raw);
    Ok(UnifiedChatResponse {
        text,
        reasoning,
        thinking,
        usage,
        logprob_stats,
        raw,
    })
}

fn logprob_stats_from_raw(protocol: ProviderProtocol, raw: &Value) -> Option<LogprobStats> {
    let logprobs = match protocol {
        ProviderProtocol::OpenaiChat => openai_chat_logprobs(raw),
        ProviderProtocol::OpenaiResponses => openai_responses_logprobs(raw),
        ProviderProtocol::Gemini | ProviderProtocol::VertexAi => gemini_logprobs(raw),
        ProviderProtocol::Anthropic | ProviderProtocol::Ollama => Vec::new(),
    };
    confidence_index(&logprobs)
}

fn openai_chat_logprobs(raw: &Value) -> Vec<f64> {
    let mut logprobs = Vec::new();
    if let Some(content) = raw
        .pointer("/choices/0/logprobs/content")
        .and_then(Value::as_array)
    {
        logprobs.extend(filtered_token_logprobs(content, "logprob"));
    }
    if logprobs.is_empty() {
        if let Some(token_logprobs) = raw
            .pointer("/choices/0/logprobs/token_logprobs")
            .and_then(Value::as_array)
        {
            logprobs.extend(
                token_logprobs
                    .iter()
                    .filter_map(Value::as_f64)
                    .filter(|value| value.is_finite()),
            );
        }
    }
    logprobs
}

fn openai_responses_logprobs(raw: &Value) -> Vec<f64> {
    let mut logprobs = Vec::new();
    collect_response_logprob_arrays(raw.pointer("/output"), &mut logprobs);
    if logprobs.is_empty() {
        collect_response_logprob_arrays(raw.pointer("/response/output"), &mut logprobs);
    }
    logprobs
}

fn collect_response_logprob_arrays(value: Option<&Value>, output: &mut Vec<f64>) {
    let Some(value) = value else {
        return;
    };
    match value {
        Value::Array(items) => {
            for item in items {
                collect_response_logprob_arrays(Some(item), output);
            }
        }
        Value::Object(object) => {
            if let Some(items) = object.get("logprobs").and_then(Value::as_array) {
                output.extend(filtered_token_logprobs(items, "logprob"));
            }
            for key in ["content", "output"] {
                if let Some(child) = object.get(key) {
                    collect_response_logprob_arrays(Some(child), output);
                }
            }
        }
        _ => {}
    }
}

fn gemini_logprobs(raw: &Value) -> Vec<f64> {
    raw.pointer("/candidates/0/logprobsResult/chosenCandidates")
        .or_else(|| raw.pointer("/candidates/0/logprobs_result/chosen_candidates"))
        .and_then(Value::as_array)
        .map(|items| {
            let mut values = filtered_token_logprobs(items, "logProbability");
            if values.is_empty() {
                values = filtered_token_logprobs(items, "log_probability");
            }
            values
        })
        .unwrap_or_default()
}

fn filtered_token_logprobs(content: &[Value], logprob_field: &str) -> Vec<f64> {
    let mut output = Vec::new();
    let mut skipping_placeholder = false;
    for item in content {
        let token = item
            .get("token")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let trimmed = token.trim();
        let placeholder_piece = placeholder_tag_piece(trimmed);
        if skipping_placeholder || placeholder_piece {
            skipping_placeholder = !trimmed.contains('>');
            continue;
        }
        if !meaningful_confidence_token(trimmed) {
            continue;
        }
        if let Some(logprob) = item.get(logprob_field).and_then(Value::as_f64) {
            if logprob.is_finite() {
                output.push(logprob);
            }
        }
    }
    output
}

fn placeholder_tag_piece(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }
    if placeholder_tag_regex().is_match(token) {
        return true;
    }
    let lower = token.to_ascii_lowercase();
    lower == "<"
        || lower == "</"
        || lower == "/"
        || lower == ">"
        || lower.starts_with("<t")
        || lower.starts_with("</t")
}

fn meaningful_confidence_token(token: &str) -> bool {
    !token.is_empty() && !punctuation_regex().is_match(token)
}

fn placeholder_tag_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?i)^</?t\d+>$").expect("static placeholder regex"))
}

fn punctuation_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"^\p{P}+$").expect("static punctuation regex"))
}

fn confidence_index(logprobs: &[f64]) -> Option<LogprobStats> {
    if logprobs.is_empty() {
        return None;
    }
    let probabilities: Vec<f64> = logprobs
        .iter()
        .map(|value| value.exp().clamp(0.0, 1.0))
        .filter(|value| value.is_finite())
        .collect();
    if probabilities.is_empty() {
        return None;
    }
    let token_count = probabilities.len() as u64;
    let average_probability = probabilities.iter().sum::<f64>() / probabilities.len() as f64;
    let variance = probabilities
        .iter()
        .map(|value| {
            let diff = value - average_probability;
            diff * diff
        })
        .sum::<f64>()
        / probabilities.len() as f64;
    let standard_deviation = variance.sqrt();
    let confidence = (average_probability - 0.5 * standard_deviation).clamp(0.0, 1.0);
    Some(LogprobStats {
        token_count,
        average_probability,
        standard_deviation,
        confidence,
    })
}

fn usage_from_openai(value: Option<&Value>) -> Option<UnifiedUsage> {
    value.map(|value| UnifiedUsage {
        input_tokens: value
            .get("prompt_tokens")
            .or_else(|| value.get("input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: value
            .get("completion_tokens")
            .or_else(|| value.get("output_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cached_tokens: value
            .pointer("/prompt_tokens_details/cached_tokens")
            .or_else(|| value.pointer("/input_tokens_details/cached_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ThinkingSummary;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const SYSTEM_TEXT: &str = "Always translate formally.";
    const USER_TEXT: &str = "Hello.";

    fn chat_error(status: Option<u16>, kind: ProviderChatErrorKind) -> ProviderChatError {
        ProviderChatError {
            status,
            message: "test error".into(),
            rate_limits: RateLimitTelemetry::default(),
            kind,
        }
    }

    fn request() -> UnifiedChatRequest {
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
            stream: true,
            logprobs: false,
            custom_parameters: json!({}),
        }
    }

    fn prompt_request() -> UnifiedChatRequest {
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
            stream: false,
            logprobs: false,
            custom_parameters: json!({}),
        }
    }

    fn adapter_for(protocol: ProviderProtocol, base_url: &str) -> RuntimeAdapter {
        RuntimeAdapter::new(
            Client::new(),
            ProviderRuntimeConfig {
                protocol,
                base_url: base_url.into(),
                use_raw_base_url: false,
                config: if protocol == ProviderProtocol::VertexAi {
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
                auth_type: "none".into(),
                auth_header: "Authorization".into(),
                credential: if protocol == ProviderProtocol::VertexAi {
                    Some("private-key".into())
                } else {
                    None
                },
                custom_headers: Vec::new(),
            },
        )
    }

    fn protected_custom_parameters() -> Value {
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
            "insituTools": {"tools": [{"name": "lookup", "description": "Lookup", "inputSchema": {"type": "object"}}]},
            "tool_choice": "required",
            "toolChoice": {"functionCallingConfig": {"mode": "ANY"}},
            "toolConfig": {"functionCallingConfig": {"mode": "ANY"}},
            "tool_config": {"functionCallingConfig": {"mode": "ANY"}},
            "stream": true,
            "stream_options": {"include_usage": false},
            "streamOptions": {"includeUsage": false}
        })
    }

    fn assert_protected_top_level_absent(body: &Value) {
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
    fn maps_deepseek_reasoning_and_cache_control() {
        let body = build_openai_chat_body("https://api.deepseek.com", &request());
        assert_eq!(body.pointer("/thinking/type"), Some(&json!("enabled")));
        assert_eq!(body.get("reasoning_effort"), Some(&json!("max")));
    }

    #[test]
    fn builds_openai_logprobs_only_when_requested() {
        let mut request = request();
        request.stream = false;
        let body = build_openai_chat_body("https://api.openai.com", &request);
        assert!(body.get("logprobs").is_none());

        request.logprobs = true;
        let body = build_openai_chat_body("https://api.openai.com", &request);
        assert_eq!(body.get("logprobs"), Some(&json!(true)));
    }

    #[test]
    fn suppresses_known_unsupported_openai_chat_logprobs() {
        let mut request = request();
        request.stream = false;
        request.logprobs = true;
        let body = build_openai_chat_body(
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            &request,
        );
        assert!(body.get("logprobs").is_none());
    }

    #[test]
    fn builds_responses_and_gemini_logprobs_requests() {
        let mut request = request();
        request.stream = false;
        request.logprobs = true;

        let responses = build_openai_responses_body("https://api.openai.com", &request);
        assert!(responses
            .get("include")
            .and_then(Value::as_array)
            .is_some_and(|items| items
                .iter()
                .any(|item| item.as_str() == Some("message.output_text.logprobs"))));

        let gemini = build_gemini_body(&request);
        assert_eq!(
            gemini.pointer("/generationConfig/responseLogprobs"),
            Some(&json!(true))
        );
    }

    #[test]
    fn requested_logprobs_override_custom_parameters() {
        let adapter = RuntimeAdapter::new(
            Client::new(),
            ProviderRuntimeConfig {
                protocol: ProviderProtocol::OpenaiChat,
                base_url: "https://api.openai.com".into(),
                use_raw_base_url: false,
                config: json!({}),
                auth_type: "none".into(),
                auth_header: "Authorization".into(),
                credential: None,
                custom_headers: Vec::new(),
            },
        );
        let mut request = request();
        request.stream = false;
        request.logprobs = true;
        request.custom_parameters = json!({"logprobs": false});
        let (_, body) = adapter.build_chat_request(&request).expect("request");
        assert_eq!(body.get("logprobs"), Some(&json!(true)));
    }

    #[test]
    fn unsupported_provider_removes_custom_logprobs_parameters() {
        let adapter = RuntimeAdapter::new(
            Client::new(),
            ProviderRuntimeConfig {
                protocol: ProviderProtocol::OpenaiChat,
                base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1".into(),
                use_raw_base_url: false,
                config: json!({}),
                auth_type: "none".into(),
                auth_header: "Authorization".into(),
                credential: None,
                custom_headers: Vec::new(),
            },
        );
        let mut request = request();
        request.stream = false;
        request.logprobs = true;
        request.custom_parameters = json!({"logprobs": true, "top_logprobs": 3});
        let (_, body) = adapter.build_chat_request(&request).expect("request");
        assert!(body.get("logprobs").is_none());
        assert!(body.get("top_logprobs").is_none());
    }

    #[test]
    fn custom_parameters_cannot_override_openai_chat_prompt_fields() {
        let adapter = adapter_for(ProviderProtocol::OpenaiChat, "https://api.deepseek.com");
        let mut request = prompt_request();
        request.custom_parameters = protected_custom_parameters();

        let (_, body) = adapter.build_chat_request(&request).expect("request");

        assert_eq!(body.get("model"), Some(&json!("stable-model")));
        assert_eq!(body.get("stream"), Some(&json!(false)));
        assert_eq!(body.pointer("/messages/0/role"), Some(&json!("system")));
        assert_eq!(
            body.pointer("/messages/0/content"),
            Some(&json!(SYSTEM_TEXT))
        );
        assert_eq!(body.pointer("/messages/1/role"), Some(&json!("user")));
        assert_eq!(body.pointer("/messages/1/content"), Some(&json!(USER_TEXT)));
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
        assert!(body.get("stream_options").is_none());
        assert_protected_top_level_absent(&body);
    }

    #[test]
    fn custom_parameters_cannot_override_openai_responses_prompt_fields() {
        let adapter = adapter_for(ProviderProtocol::OpenaiResponses, "https://api.openai.com");
        let mut request = prompt_request();
        request.custom_parameters = protected_custom_parameters();

        let (_, body) = adapter.build_chat_request(&request).expect("request");

        assert_eq!(body.get("model"), Some(&json!("stable-model")));
        assert_eq!(body.get("stream"), Some(&json!(false)));
        assert_eq!(body.pointer("/input/0/role"), Some(&json!("system")));
        assert_eq!(
            body.pointer("/input/0/content/0/text"),
            Some(&json!(SYSTEM_TEXT))
        );
        assert_eq!(body.pointer("/input/1/role"), Some(&json!("user")));
        assert_eq!(
            body.pointer("/input/1/content/0/text"),
            Some(&json!(USER_TEXT))
        );
        assert!(body.get("instructions").is_none());
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
        assert_protected_top_level_absent(&body);
    }

    #[test]
    fn custom_parameters_cannot_override_anthropic_prompt_fields() {
        let adapter = adapter_for(ProviderProtocol::Anthropic, "https://api.anthropic.com");
        let mut request = prompt_request();
        request.custom_parameters = protected_custom_parameters();

        let (_, body) = adapter.build_chat_request(&request).expect("request");

        assert_eq!(body.get("model"), Some(&json!("stable-model")));
        assert_eq!(body.pointer("/system/0/text"), Some(&json!(SYSTEM_TEXT)));
        assert_eq!(
            body.pointer("/messages/0/content/0/text"),
            Some(&json!(USER_TEXT))
        );
        assert!(body.get("contents").is_none());
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
        assert_protected_top_level_absent(&body);
    }

    #[test]
    fn custom_parameters_cannot_override_gemini_prompt_fields() {
        for protocol in [ProviderProtocol::Gemini, ProviderProtocol::VertexAi] {
            let adapter = adapter_for(
                protocol,
                if protocol == ProviderProtocol::VertexAi {
                    "https://aiplatform.googleapis.com"
                } else {
                    "https://generativelanguage.googleapis.com"
                },
            );
            let mut request = prompt_request();
            request.custom_parameters = protected_custom_parameters();

            let (_, body) = adapter.build_chat_request(&request).expect("request");

            assert_eq!(
                body.pointer("/systemInstruction/parts/0/text"),
                Some(&json!(SYSTEM_TEXT))
            );
            assert_eq!(body.pointer("/contents/0/role"), Some(&json!("user")));
            assert_eq!(
                body.pointer("/contents/0/parts/0/text"),
                Some(&json!(USER_TEXT))
            );
            assert_eq!(
                body.pointer("/generationConfig/temperature"),
                Some(&json!(0.0))
            );
            assert!(body.get("tools").is_none());
            assert!(body.get("toolConfig").is_none());
            assert_protected_top_level_absent(&body);
        }
    }

    #[test]
    fn custom_parameters_cannot_override_ollama_prompt_fields() {
        let adapter = adapter_for(ProviderProtocol::Ollama, "http://localhost:11434/api");
        let mut request = prompt_request();
        request.custom_parameters = protected_custom_parameters();

        let (_, body) = adapter.build_chat_request(&request).expect("request");

        assert_eq!(body.get("model"), Some(&json!("stable-model")));
        assert_eq!(body.get("stream"), Some(&json!(false)));
        assert_eq!(body.pointer("/messages/0/role"), Some(&json!("system")));
        assert_eq!(
            body.pointer("/messages/0/content"),
            Some(&json!(SYSTEM_TEXT))
        );
        assert_eq!(body.pointer("/messages/1/role"), Some(&json!("user")));
        assert_eq!(body.pointer("/messages/1/content"), Some(&json!(USER_TEXT)));
        assert!(body.get("tools").is_none());
        assert_protected_top_level_absent(&body);
    }

    #[test]
    fn allowed_custom_parameters_still_merge_deeply() {
        let mut request = prompt_request();
        request.custom_parameters = json!({
            "response_format": {"type": "json_object"},
            "top_p": 0.9,
            "generationConfig": {
                "candidateCount": 1,
                "responseMimeType": "application/json"
            },
            "safetySettings": [{
                "category": "HARM_CATEGORY_HARASSMENT",
                "threshold": "BLOCK_MEDIUM_AND_ABOVE"
            }]
        });

        let (_, openai) = adapter_for(ProviderProtocol::OpenaiChat, "https://api.openai.com")
            .build_chat_request(&request)
            .expect("openai request");
        assert_eq!(
            openai.pointer("/response_format/type"),
            Some(&json!("json_object"))
        );
        assert_eq!(openai.get("top_p"), Some(&json!(0.9)));

        let (_, gemini) = adapter_for(
            ProviderProtocol::Gemini,
            "https://generativelanguage.googleapis.com",
        )
        .build_chat_request(&request)
        .expect("gemini request");
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
    }

    #[test]
    fn empty_custom_parameters_match_null_custom_parameters() {
        let adapter = adapter_for(ProviderProtocol::OpenaiChat, "https://api.openai.com");
        let mut empty = prompt_request();
        empty.custom_parameters = json!({});
        let mut null = prompt_request();
        null.custom_parameters = Value::Null;

        let (_, empty_body) = adapter.build_chat_request(&empty).expect("empty request");
        let (_, null_body) = adapter.build_chat_request(&null).expect("null request");

        assert_eq!(empty_body, null_body);
    }

    #[test]
    fn anthropic_alternates_adjacent_same_roles() {
        let mut request = request();
        request.messages.insert(
            0,
            UnifiedMessage {
                role: "user".into(),
                content: vec![UnifiedContent::Text {
                    text: "first".into(),
                }],
            },
        );
        let body = build_anthropic_body(&request);
        assert_eq!(body.pointer("/messages/0/role"), Some(&json!("user")));
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn anthropic_body_ignores_logprobs_request_flag() {
        let mut request = request();
        request.logprobs = true;
        let body = build_anthropic_body(&request);
        assert!(body.get("logprobs").is_none());
    }

    #[test]
    fn gemini_three_uses_thinking_level() {
        let mut request = request();
        request.model = "gemini-3-pro".into();
        request.thinking = Some(ThinkingConfig {
            mode: ThinkingMode::Enabled,
            budget_tokens: Some(32_000),
            effort: Some(ThinkingEffort::Max),
            summary: None,
        });

        let body = build_gemini_body(&request);

        assert_eq!(
            body.pointer("/generationConfig/thinkingConfig/includeThoughts"),
            Some(&json!(false))
        );
        assert_eq!(
            body.pointer("/generationConfig/thinkingConfig/thinkingLevel"),
            Some(&json!("high"))
        );
        assert!(body
            .pointer("/generationConfig/thinkingConfig/thinkingBudget")
            .is_none());
    }

    #[test]
    fn vertex_ai_keeps_gemini_body_for_thinking() {
        let adapter = adapter_for(
            ProviderProtocol::VertexAi,
            "https://aiplatform.googleapis.com",
        );
        let mut request = request();
        request.model = "gemini-3-pro".into();
        request.stream = false;

        let (url, body) = adapter
            .build_chat_request(&request)
            .expect("vertex request");

        assert!(url.contains(
            "/projects/project-1/locations/global/publishers/google/models/gemini-3-pro:generateContent"
        ));
        assert!(body.get("tools").is_none());
        assert_eq!(
            body.pointer("/generationConfig/thinkingConfig/thinkingLevel"),
            Some(&json!("high"))
        );
        assert!(body.get("messages").is_none());
        assert!(body.get("reasoning").is_none());
    }

    #[test]
    fn raw_base_url_skips_openai_version_injection() {
        let adapter = RuntimeAdapter::new(
            Client::new(),
            ProviderRuntimeConfig {
                protocol: ProviderProtocol::OpenaiChat,
                base_url: "https://proxy.example/openai/v1".into(),
                use_raw_base_url: true,
                config: json!({}),
                auth_type: "none".into(),
                auth_header: "Authorization".into(),
                credential: None,
                custom_headers: Vec::new(),
            },
        );
        let (url, _) = adapter.build_chat_request(&request()).expect("request");
        assert_eq!(url, "https://proxy.example/openai/v1/chat/completions");
    }

    #[test]
    fn builds_responses_input_with_thinking() {
        let mut request = request();
        request.stream = false;
        request.messages = vec![
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
        let body = build_openai_responses_body("https://api.openai.com", &request);
        let input = body.get("input").and_then(Value::as_array).expect("input");
        assert_eq!(
            input[0].pointer("/content/0/type"),
            Some(&json!("input_text"))
        );
        assert!(input
            .iter()
            .any(|item| item.get("type") == Some(&json!("reasoning"))));
        assert!(!input
            .iter()
            .any(|item| item.get("type") == Some(&json!("function_call"))));
    }

    #[test]
    fn openai_responses_appends_web_search_tool() {
        let mut request = request();
        request.stream = false;
        request.web_search = true;

        let body = build_openai_responses_body("https://api.openai.com", &request);
        let tools = body.get("tools").and_then(Value::as_array).expect("tools");

        assert!(tools
            .iter()
            .any(|tool| tool.get("type") == Some(&json!("web_search"))));
    }

    #[test]
    fn openai_chat_search_model_does_not_inject_responses_tool() {
        let mut request = prompt_request();
        request.model = "gpt-5-search-api".into();
        request.web_search = true;

        let body = build_openai_chat_body("https://api.openai.com", &request);

        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
    }

    #[test]
    fn anthropic_appends_web_search_server_tool() {
        let mut request = prompt_request();
        request.model = "claude-sonnet-4-20250514".into();
        request.web_search = true;

        let body = build_anthropic_body(&request);

        assert_eq!(
            body.pointer("/tools/0/type"),
            Some(&json!("web_search_20250305"))
        );
        assert_eq!(body.pointer("/tools/0/name"), Some(&json!("web_search")));
    }

    #[test]
    fn gemini_and_vertex_append_google_search_tool() {
        for protocol in [ProviderProtocol::Gemini, ProviderProtocol::VertexAi] {
            let adapter = adapter_for(
                protocol,
                if protocol == ProviderProtocol::VertexAi {
                    "https://aiplatform.googleapis.com"
                } else {
                    "https://generativelanguage.googleapis.com"
                },
            );
            let mut request = prompt_request();
            request.model = "gemini-2.5-pro".into();
            request.web_search = true;

            let (url, body) = adapter.build_chat_request(&request).expect("request");

            if protocol == ProviderProtocol::VertexAi {
                assert!(url.contains(
                    "/projects/project-1/locations/global/publishers/google/models/gemini-2.5-pro:generateContent"
                ));
            }
            assert_eq!(body.pointer("/tools/0/googleSearch"), Some(&json!({})));
        }
    }

    #[test]
    fn normalizes_openai_reasoning_details() {
        let response = normalize_response(
            ProviderProtocol::OpenaiChat,
            json!({
                "choices": [{
                    "message": {
                        "content": "ok",
                        "reasoning_details": [
                            {"type": "reasoning.text", "text": "think", "signature": "sig"},
                            {"type": "reasoning.encrypted", "data": "secret"}
                        ]
                    }
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 2}
            }),
        )
        .expect("normalize");
        assert_eq!(response.text, "ok");
        assert_eq!(response.reasoning, "think");
        assert_eq!(response.thinking.len(), 2);
    }

    #[test]
    fn drops_unsupported_openai_history_thinking() {
        let mut request = prompt_request();
        request.messages.push(UnifiedMessage {
            role: "assistant".into(),
            content: vec![UnifiedContent::Thinking {
                text: "hidden plan".into(),
                signature: None,
                encrypted_data: None,
            }],
        });

        let body = build_openai_chat_body("https://api.openai.com", &request);
        let messages = body
            .get("messages")
            .and_then(Value::as_array)
            .expect("messages");

        assert_eq!(messages.len(), 2);
        assert!(!messages
            .iter()
            .any(|message| message.get("content") == Some(&json!("hidden plan"))));
    }

    #[test]
    fn normalizes_gemini_thought_parts_without_text_leakage() {
        let response = normalize_response(
            ProviderProtocol::Gemini,
            json!({
                "candidates": [{
                    "content": {
                        "parts": [
                            {
                                "text": "think",
                                "thought": true,
                                "thoughtSignature": "sig"
                            },
                            {"text": "translated"}
                        ]
                    }
                }]
            }),
        )
        .expect("gemini response");

        assert_eq!(response.text, "translated");
        assert_eq!(response.reasoning, "think");
        assert_eq!(response.thinking.len(), 1);
        assert!(matches!(
            &response.thinking[0],
            UnifiedContent::Thinking {
                text,
                signature: Some(signature),
                ..
            } if text == "think" && signature == "sig"
        ));

        let thought_only = normalize_response(
            ProviderProtocol::Gemini,
            json!({
                "candidates": [{
                    "content": {
                        "parts": [{"text": "think", "thought": true}]
                    }
                }]
            }),
        )
        .expect("gemini thought only");

        assert!(thought_only.text.is_empty());
        assert_eq!(thought_only.reasoning, "think");
    }

    #[test]
    fn strips_leading_inline_thinking_tags() {
        let response = normalize_response(
            ProviderProtocol::OpenaiChat,
            json!({
                "choices": [{
                    "message": {
                        "content": "<think>plan</think>\ntranslated"
                    }
                }]
            }),
        )
        .expect("tagged response");

        assert_eq!(response.text, "translated");
        assert_eq!(response.reasoning, "plan");

        let unclosed = normalize_response(
            ProviderProtocol::OpenaiChat,
            json!({
                "choices": [{
                    "message": {
                        "content": "<thinking>unfinished"
                    }
                }]
            }),
        )
        .expect("unclosed tagged response");

        assert!(unclosed.text.is_empty());
        assert_eq!(unclosed.reasoning, "unfinished");
    }

    #[test]
    fn normalizes_openai_logprob_stats() {
        let response = normalize_response(
            ProviderProtocol::OpenaiChat,
            json!({
                "choices": [{
                    "message": {"content": "ok"},
                    "logprobs": {
                        "content": [
                            {"token": "<t1>", "logprob": 0.0},
                            {"token": "o", "logprob": 0.0},
                            {"token": "，", "logprob": 0.0},
                            {"token": "k", "logprob": -1.3862943611198906},
                            {"token": "</", "logprob": 0.0},
                            {"token": "t1", "logprob": 0.0},
                            {"token": ">", "logprob": 0.0}
                        ]
                    }
                }]
            }),
        )
        .expect("normalize");
        let stats = response.logprob_stats.expect("stats");
        assert_eq!(response.text, "ok");
        assert_eq!(stats.token_count, 2);
        assert!((stats.average_probability - 0.625).abs() < 0.000001);
        assert!((stats.standard_deviation - 0.375).abs() < 0.000001);
        assert!((stats.confidence - 0.4375).abs() < 0.000001);
    }

    #[test]
    fn normalizes_responses_and_gemini_logprob_stats() {
        let responses = normalize_response(
            ProviderProtocol::OpenaiResponses,
            json!({
                "output": [{
                    "type": "message",
                    "content": [{
                        "type": "output_text",
                        "text": "ok",
                        "logprobs": [
                            {"token": "<", "logprob": 0.0},
                            {"token": "t1", "logprob": 0.0},
                            {"token": ">", "logprob": 0.0},
                            {"token": "ok", "logprob": -0.6931471805599453},
                            {"token": "。", "logprob": 0.0}
                        ]
                    }]
                }],
                "usage": {"input_tokens": 1, "output_tokens": 2}
            }),
        )
        .expect("responses");
        let stats = responses.logprob_stats.expect("responses stats");
        assert_eq!(responses.text, "ok");
        assert_eq!(stats.token_count, 1);
        assert!((stats.confidence - 0.5).abs() < 0.000001);

        let gemini = normalize_response(
            ProviderProtocol::Gemini,
            json!({
                "candidates": [{
                    "content": {"parts": [{"text": "ok"}]},
                    "logprobsResult": {
                        "chosenCandidates": [
                            {"token": "ok", "logProbability": -0.6931471805599453},
                            {"token": "!", "logProbability": 0.0}
                        ]
                    }
                }]
            }),
        )
        .expect("gemini");
        let stats = gemini.logprob_stats.expect("gemini stats");
        assert_eq!(gemini.text, "ok");
        assert_eq!(stats.token_count, 1);
        assert!((stats.confidence - 0.5).abs() < 0.000001);
    }

    #[test]
    fn parses_provider_rate_limit_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-ratelimit-limit-requests",
            HeaderValue::from_static("100"),
        );
        headers.insert(
            "x-ratelimit-remaining-requests",
            HeaderValue::from_static("2"),
        );
        headers.insert(
            "x-ratelimit-reset-requests",
            HeaderValue::from_static("1.5s"),
        );
        headers.insert("x-ratelimit-limit-tokens", HeaderValue::from_static("9000"));
        headers.insert(
            "x-ratelimit-remaining-tokens",
            HeaderValue::from_static("800"),
        );
        let telemetry = rate_limits_from_headers(&headers);
        assert_eq!(telemetry.request_limit, Some(100));
        assert_eq!(telemetry.request_remaining, Some(2));
        assert_eq!(telemetry.request_reset_ms, Some(1500));
        assert_eq!(telemetry.token_limit, Some(9000));
        assert_eq!(telemetry.token_remaining, Some(800));
        assert_eq!(telemetry.source.as_deref(), Some("openai-compatible"));
    }

    #[test]
    fn classifies_transient_provider_chat_errors() {
        for status in [408, 429, 499, 500, 502, 503, 504] {
            assert!(
                chat_error(Some(status), ProviderChatErrorKind::HttpStatus).is_transient(),
                "HTTP {status} should be transient"
            );
        }
        for status in [400, 401, 403, 404, 422] {
            assert!(
                !chat_error(Some(status), ProviderChatErrorKind::HttpStatus).is_transient(),
                "HTTP {status} should be permanent"
            );
        }
        assert!(chat_error(None, ProviderChatErrorKind::Transport).is_transient());
        assert!(chat_error(Some(503), ProviderChatErrorKind::Transport).is_transient());
        assert!(!chat_error(Some(401), ProviderChatErrorKind::Transport).is_transient());
        assert!(!chat_error(None, ProviderChatErrorKind::LocalRequest).is_transient());
        assert!(!chat_error(Some(200), ProviderChatErrorKind::InvalidResponse).is_transient());
    }

    #[test]
    fn parses_google_rpc_retry_delay_from_error_body() {
        let body = json!({
            "error": {
                "code": 429,
                "status": "RESOURCE_EXHAUSTED",
                "details": [{
                    "@type": "type.googleapis.com/google.rpc.RetryInfo",
                    "retryDelay": "2.25s"
                }]
            }
        })
        .to_string();
        assert_eq!(retry_after_ms_from_error_body(&body), Some(2250));

        let object_body = json!({
            "error": {
                "details": [{
                    "@type": "type.googleapis.com/google.rpc.RetryInfo",
                    "retryDelay": {"seconds": 1, "nanos": 500000000}
                }]
            }
        })
        .to_string();
        assert_eq!(retry_after_ms_from_error_body(&object_body), Some(1500));
    }

    #[test]
    fn detects_truncation_finish_reasons() {
        assert!(finish_reason_is_truncation(Some("length")));
        assert!(finish_reason_is_truncation(Some("MAX_TOKENS")));
        assert!(finish_reason_is_truncation(Some("max_output_tokens")));
        assert!(!finish_reason_is_truncation(Some("stop")));
        assert!(!finish_reason_is_truncation(None));
    }

    #[tokio::test]
    async fn fetches_models_from_mock_openai_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            let body = r#"{"data":[{"id":"mock-model","name":"Mock Model"}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write");
        });
        let adapter = RuntimeAdapter::new(
            Client::new(),
            ProviderRuntimeConfig {
                protocol: ProviderProtocol::OpenaiChat,
                base_url: format!("http://{address}"),
                use_raw_base_url: false,
                config: json!({}),
                auth_type: "none".into(),
                auth_header: "Authorization".into(),
                credential: None,
                custom_headers: Vec::new(),
            },
        );
        let models = adapter.list_models().await.expect("list models");
        assert_eq!(models[0].request_name, "mock-model");
        server.join().expect("server");
    }
}
