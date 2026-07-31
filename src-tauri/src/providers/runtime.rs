use std::fmt;

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, Method};
use serde_json::Value;

use crate::domain::{ProviderRuntimeConfig, RemoteModel, UnifiedChatRequest, UnifiedChatResponse};
use crate::providers::negotiation::{
    plan_retry, write_audit, NegotiationFailure, NegotiationFailureKind,
};
use crate::providers::registry::{descriptor_for, AuthStrategy};
use crate::providers::{EncodedRequest, HeaderDirective, HeaderMode, HttpMethod, ProtocolCodec};

pub trait ProviderAdapter {
    async fn list_models(&self) -> Result<Vec<RemoteModel>, String>;
    #[allow(dead_code)]
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

    pub fn is_model_unavailable(&self) -> bool {
        if !matches!(self.status, Some(400 | 404)) {
            return false;
        }
        let message = self.message.to_ascii_lowercase();
        let mentions_model = message.contains("model") || message.contains("deployment");
        mentions_model
            && [
                "not found",
                "does not exist",
                "doesn't exist",
                "no longer available",
                "not available",
                "deprecated",
                "decommissioned",
                "retired",
                "unsupported model",
                "unknown model",
                "model_not_found",
                "modelnotfound",
                "deploymentnotfound",
                "model_not_available",
                "model_deprecated",
            ]
            .iter()
            .any(|marker| message.contains(marker))
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

impl RuntimeAdapter {
    pub fn new(client: Client, config: ProviderRuntimeConfig) -> Self {
        Self { client, config }
    }

    fn codec(&self) -> Result<&'static dyn ProtocolCodec, String> {
        Ok(descriptor_for(&self.config.protocol)?.codec)
    }

    async fn headers(&self, directives: &[HeaderDirective]) -> Result<HeaderMap, String> {
        let descriptor = descriptor_for(&self.config.protocol)?;
        let auth_strategy = descriptor.auth.strategy;
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let AuthStrategy::StaticHeader { header, scheme } = auth_strategy {
            if let Some(credential) = self
                .config
                .credential
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                let name = HeaderName::from_bytes(header.as_bytes())
                    .map_err(|error| format!("Invalid authentication header: {error}"))?;
                let value = match scheme {
                    Some(scheme) => format!("{scheme} {credential}"),
                    None => credential.to_string(),
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
            if auth_strategy == AuthStrategy::VertexServiceAccount && header_name == AUTHORIZATION {
                continue;
            }
            headers.insert(
                header_name,
                HeaderValue::from_str(value)
                    .map_err(|error| format!("Invalid custom header value for {name}: {error}"))?,
            );
        }
        if auth_strategy == AuthStrategy::VertexServiceAccount {
            let token = crate::vertex_ai::access_token(&self.client, &self.config).await?;
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|error| format!("Invalid Agent Platform token: {error}"))?,
            );
        }
        for directive in directives {
            let name = HeaderName::from_bytes(directive.name.as_bytes())
                .map_err(|error| format!("Invalid protocol header {}: {error}", directive.name))?;
            if directive.mode == HeaderMode::Replace || !headers.contains_key(&name) {
                let value = HeaderValue::from_str(&directive.value)
                    .map_err(|error| format!("Invalid protocol header value: {error}"))?;
                headers.insert(name, value);
            }
        }
        Ok(headers)
    }

    fn encoded_chat_request(&self, request: &UnifiedChatRequest) -> Result<EncodedRequest, String> {
        let encoded = self.codec()?.encode_chat(&self.config, request)?;
        if encoded.body.is_none() {
            return Err("Chat request body is missing".into());
        }
        Ok(encoded)
    }

    async fn request_json(&self, request: EncodedRequest) -> Result<Value, String> {
        self.request_json_with_meta(request)
            .await
            .map(|meta| meta.raw)
            .map_err(|error| error.to_string())
    }

    async fn request_json_with_meta(
        &self,
        encoded: EncodedRequest,
    ) -> Result<JsonResponseMeta, ProviderChatError> {
        let mut request = self
            .client
            .request(reqwest_method(encoded.method), encoded.url)
            .headers(
                self.headers(&encoded.headers)
                    .await
                    .map_err(local_request_error)?,
            );
        if let Some(value) = encoded.body {
            request = request.json(&value);
        }
        let response = request.send().await.map_err(|error| ProviderChatError {
            status: None,
            message: error.to_string(),
            rate_limits: RateLimitTelemetry::default(),
            kind: ProviderChatErrorKind::Transport,
        })?;
        let status = response.status();
        let mut rate_limits = rate_limits_from_headers(response.headers());
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

    pub async fn send_chat_with_meta(
        &self,
        request: &UnifiedChatRequest,
    ) -> Result<ProviderChatMeta, ProviderChatError> {
        let encoded = self
            .encoded_chat_request(request)
            .map_err(local_request_error)?;
        let meta = match self.request_json_with_meta(encoded.clone()).await {
            Ok(meta) => meta,
            Err(original_error) => {
                let descriptor =
                    descriptor_for(&self.config.protocol).map_err(local_request_error)?;
                let Some(mut plan) = plan_retry(
                    Some(descriptor.wire_family),
                    &self.config.base_url,
                    &request.model,
                    &encoded,
                    negotiation_failure(&original_error),
                    0,
                    !request.stream,
                ) else {
                    return Err(original_error);
                };
                match self.request_json_with_meta(plan.request).await {
                    Ok(meta) => {
                        plan.audit.record_success(meta.status);
                        write_audit(&plan.audit);
                        meta
                    }
                    Err(mut retry_error) => {
                        plan.audit.record_failure(retry_error.status);
                        write_audit(&plan.audit);
                        retry_error.message = format!(
                            "Compatibility retry {} failed: {}; original error: {}",
                            plan.audit.rule_id, retry_error.message, original_error.message
                        );
                        return Err(retry_error);
                    }
                }
            }
        };
        let codec = self.codec().map_err(local_request_error)?;
        let finish_reason = codec.finish_reason(&meta.raw);
        let response = codec
            .decode_chat(meta.raw)
            .map_err(|message| ProviderChatError {
                status: Some(meta.status),
                message,
                rate_limits: meta.rate_limits.clone(),
                kind: ProviderChatErrorKind::InvalidResponse,
            })?;
        Ok(ProviderChatMeta {
            response,
            status: meta.status,
            rate_limits: meta.rate_limits,
            finish_reason,
        })
    }
}

impl ProviderAdapter for RuntimeAdapter {
    async fn list_models(&self) -> Result<Vec<RemoteModel>, String> {
        let request = self.codec()?.encode_model_list(&self.config)?;
        let value = self.request_json(request).await?;
        let mut models = self.codec()?.decode_model_list(&value)?;
        models.sort_by(|left, right| left.request_name.cmp(&right.request_name));
        Ok(models)
    }

    fn build_chat_request(&self, request: &UnifiedChatRequest) -> Result<(String, Value), String> {
        let encoded = self.encoded_chat_request(request)?;
        Ok((
            encoded.url,
            encoded
                .body
                .ok_or_else(|| "Chat request body is missing".to_string())?,
        ))
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
        let encoded = self.encoded_chat_request(request)?;
        let headers = self.headers(&encoded.headers).await?;
        let body = encoded
            .body
            .ok_or_else(|| "Chat request body is missing".to_string())?;
        let response = self
            .client
            .request(reqwest_method(encoded.method), encoded.url)
            .headers(headers)
            .json(&body)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.map_err(|error| error.to_string())?;
            return Err(self.codec()?.decode_error(status.as_u16(), &text));
        }
        let mut stream = response.bytes_stream();
        let mut decoder = self.codec()?.new_stream_decoder();
        let mut output = Vec::new();
        while let Some(chunk) = stream.next().await {
            output.extend(decoder.push(&chunk.map_err(|error| error.to_string())?)?);
        }
        output.extend(decoder.finish()?);
        Ok(output)
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

fn reqwest_method(method: HttpMethod) -> Method {
    match method {
        HttpMethod::Get => Method::GET,
        HttpMethod::Post => Method::POST,
        HttpMethod::Put => Method::PUT,
        HttpMethod::Patch => Method::PATCH,
        HttpMethod::Delete => Method::DELETE,
    }
}

fn local_request_error(message: String) -> ProviderChatError {
    ProviderChatError {
        status: None,
        message,
        rate_limits: RateLimitTelemetry::default(),
        kind: ProviderChatErrorKind::LocalRequest,
    }
}

fn negotiation_failure(error: &ProviderChatError) -> NegotiationFailure<'_> {
    let kind = match (error.kind, error.status) {
        (ProviderChatErrorKind::HttpStatus, Some(401 | 403)) => {
            NegotiationFailureKind::Authentication
        }
        (ProviderChatErrorKind::HttpStatus, Some(429)) => NegotiationFailureKind::RateLimit,
        (ProviderChatErrorKind::HttpStatus, _) => NegotiationFailureKind::HttpStatus,
        (ProviderChatErrorKind::Transport, _) => NegotiationFailureKind::Transport,
        (ProviderChatErrorKind::InvalidResponse, _) => NegotiationFailureKind::InvalidResponse,
        (ProviderChatErrorKind::LocalRequest, _) => NegotiationFailureKind::LocalRequest,
    };
    NegotiationFailure {
        status: error.status,
        kind,
        message: &error.message,
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
        if type_name.ends_with("google.rpc.RetryInfo") {
            detail.get("retryDelay").and_then(parse_retry_delay_ms)
        } else {
            None
        }
    })
}

fn parse_retry_delay_ms(value: &Value) -> Option<u64> {
    if let Some(text) = value.as_str() {
        return parse_duration_ms(text);
    }
    let seconds = value.get("seconds").and_then(Value::as_u64).unwrap_or(0);
    let nanos = value.get("nanos").and_then(Value::as_u64).unwrap_or(0);
    (seconds != 0 || nanos != 0).then(|| {
        seconds
            .saturating_mul(1000)
            .saturating_add(nanos / 1_000_000)
    })
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
            .parse()
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
    for (suffix, multiplier) in [
        ("ms", 1.0),
        ("s", 1000.0),
        ("m", 60_000.0),
        ("h", 3_600_000.0),
    ] {
        if let Some(number) = trimmed.strip_suffix(suffix) {
            return number
                .trim()
                .parse::<f64>()
                .ok()
                .map(|value| (value * multiplier).ceil() as u64);
        }
    }
    trimmed
        .parse::<f64>()
        .ok()
        .map(|value| (value * 1000.0).ceil() as u64)
}

fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;
    use serde_json::json;

    fn config(id: &str) -> ProviderRuntimeConfig {
        let descriptor =
            crate::providers::registry::descriptor_by_id(id).expect("registered protocol");
        ProviderRuntimeConfig {
            protocol: crate::domain::ProtocolId::registered(id),
            base_url: descriptor.default_base_url.into(),
            use_raw_base_url: false,
            config: json!({}),
            credential: None,
            custom_headers: Vec::new(),
        }
    }

    #[test]
    fn parses_rate_limit_headers_and_retry_durations() {
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
        let telemetry = rate_limits_from_headers(&headers);
        assert_eq!(telemetry.request_limit, Some(100));
        assert_eq!(telemetry.request_remaining, Some(2));
        assert_eq!(telemetry.request_reset_ms, Some(1500));
        assert_eq!(telemetry.source.as_deref(), Some("openai-compatible"));
    }

    #[test]
    fn recognizes_google_rpc_retry_info() {
        assert_eq!(
            retry_after_ms_from_error_body(
                r#"{"error":{"details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"2.25s"}]}}"#,
            ),
            Some(2250)
        );
    }

    #[tokio::test]
    async fn header_priority_preserves_custom_values_for_if_absent_directives() {
        let mut config = config("anthropic");
        config.credential = Some("credential-key".into());
        config.custom_headers = vec![
            ("x-api-key".into(), "custom-key".into()),
            ("anthropic-version".into(), "custom-version".into()),
        ];
        let adapter = RuntimeAdapter::new(Client::new(), config);
        let headers = adapter
            .headers(&[HeaderDirective {
                name: "anthropic-version".into(),
                value: "2023-06-01".into(),
                mode: HeaderMode::IfAbsent,
            }])
            .await
            .expect("headers");

        assert_eq!(headers["x-api-key"], "custom-key");
        assert_eq!(headers["anthropic-version"], "custom-version");
        assert_eq!(headers[CONTENT_TYPE], "application/json");
    }

    #[tokio::test]
    async fn unknown_protocol_is_rejected_before_any_http_request() {
        let mut config = config("openai-chat");
        config.protocol = crate::domain::ProtocolId::unknown();
        config.base_url = "http://127.0.0.1:1".into();
        let adapter = RuntimeAdapter::new(Client::new(), config);

        let error = adapter.list_models().await.expect_err("unknown protocol");
        assert!(error.contains("Unknown or unavailable provider protocol"));
    }
}
