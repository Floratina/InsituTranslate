use std::fmt;

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, Method};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::document_parsing::count_tokens;
use crate::domain::{
    ProviderRuntimeConfig, RemoteModel, UnifiedChatRequest, UnifiedChatResponse, UnifiedUsage,
    UnifiedUsageProvenance,
};
use crate::providers::negotiation::{
    plan_retry, write_audit, NegotiationFailure, NegotiationFailureKind,
};
use crate::providers::registry::{descriptor_for, AuthStrategy};
use crate::providers::{
    CompletionBudget, EncodedRequest, HeaderDirective, HeaderMode, HttpMethod, ProtocolCodec,
};

pub trait ProviderAdapter {
    async fn list_models(&self) -> Result<Vec<RemoteModel>, String>;
    #[allow(dead_code)]
    fn build_chat_request(&self, request: &UnifiedChatRequest) -> Result<(String, Value), String>;
    async fn send_chat(&self, request: &UnifiedChatRequest) -> Result<UnifiedChatResponse, String>;
    #[allow(dead_code)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatAttemptKind {
    Primary,
    LogprobsFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatAttemptContext {
    pub kind: ChatAttemptKind,
    pub logical_attempt: u32,
    pub compatibility_retry: bool,
}

impl ChatAttemptContext {
    pub fn compatibility_retry(self) -> Self {
        Self {
            compatibility_retry: true,
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestCost {
    pub estimated_input_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct PreparedChatRequest {
    pub request: UnifiedChatRequest,
    pub encoded: EncodedRequest,
    pub completion_budget: CompletionBudget,
    pub cost: RequestCost,
}

#[derive(Debug, Clone)]
pub struct ChatAttemptOutcome {
    pub status: Option<u16>,
    pub rate_limits: RateLimitTelemetry,
    pub actual_total_tokens: Option<u64>,
}

pub trait ChatAttemptGate: Send + Sync {
    type Reservation: Send;

    async fn acquire(
        &self,
        context: ChatAttemptContext,
        cost: RequestCost,
        cancellation: &CancellationToken,
    ) -> Result<Self::Reservation, String>;

    async fn settle(
        &self,
        reservation: Self::Reservation,
        outcome: ChatAttemptOutcome,
    ) -> Result<(), String>;
}

#[derive(Debug, Clone)]
pub struct ProviderChatError {
    pub status: Option<u16>,
    pub message: String,
    pub compatibility_text: Option<String>,
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
    pub fn local(message: impl Into<String>) -> Self {
        local_request_error(message.into())
    }

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

struct GatedJsonResponseMeta<R> {
    meta: JsonResponseMeta,
    reservation: R,
    cost: RequestCost,
}

impl RuntimeAdapter {
    pub fn new(client: Client, config: ProviderRuntimeConfig) -> Self {
        Self { client, config }
    }

    fn codec(&self) -> Result<&'static dyn ProtocolCodec, String> {
        Ok(descriptor_for(&self.config.protocol)?.codec)
    }

    pub fn validate_and_plan_chat_request(
        &self,
        request: &UnifiedChatRequest,
        visible_output_tokens: u32,
    ) -> Result<CompletionBudget, String> {
        let codec = self.codec()?;
        codec.validate_chat_options(
            &self.config.base_url,
            &request.model,
            request.thinking.as_ref(),
            request.temperature,
            request.top_p,
            &request.custom_parameters,
        )?;
        codec.plan_completion_budget(
            &self.config.base_url,
            &request.model,
            request.thinking.as_ref(),
            request.max_output_tokens,
            &request.custom_parameters,
            visible_output_tokens,
        )
    }

    pub fn prepare_chat_request(
        &self,
        request: &UnifiedChatRequest,
        visible_output_tokens: u32,
    ) -> Result<PreparedChatRequest, String> {
        let completion_budget =
            self.validate_and_plan_chat_request(request, visible_output_tokens)?;
        let mut normalized_request = request.clone();
        normalized_request.max_output_tokens = completion_budget.wire_max_output_tokens;
        normalized_request.custom_parameters = completion_budget.custom_parameters.clone();
        let encoded = self.encoded_chat_request(&normalized_request)?;
        let cost = request_cost(&encoded, completion_budget.total_output_tokens)?;
        Ok(PreparedChatRequest {
            request: normalized_request,
            encoded,
            completion_budget,
            cost,
        })
    }

    pub fn output_truncation_error(
        &self,
        request: &UnifiedChatRequest,
        finish_reason: &str,
    ) -> String {
        let max_output_tokens = request
            .max_output_tokens
            .map(|value| value.to_string())
            .unwrap_or_else(|| "provider-default".to_string());
        let thinking = request.thinking.as_ref().map_or_else(
            || "omitted/provider-default".to_string(),
            |thinking| {
                format!(
                    "mode={:?}, effort={:?}, budget_tokens={:?}",
                    thinking.mode, thinking.effort, thinking.budget_tokens
                )
            },
        );
        format!(
            "OUTPUT_TRUNCATED: protocol={} model=\"{}\" finish_reason={} max_output_tokens={} thinking={}",
            self.config.protocol.as_str(),
            request.model,
            finish_reason,
            max_output_tokens,
            thinking
        )
    }

    async fn headers(&self, directives: &[HeaderDirective]) -> Result<HeaderMap, String> {
        let descriptor = descriptor_for(&self.config.protocol)?;
        let auth_strategy = descriptor.auth.strategy;
        let vertex_token = if auth_strategy == AuthStrategy::VertexServiceAccount {
            Some(crate::vertex_ai::access_token(&self.client, &self.config).await?)
        } else {
            None
        };
        self.build_headers(directives, vertex_token.as_deref())
    }

    fn build_headers(
        &self,
        directives: &[HeaderDirective],
        vertex_token: Option<&str>,
    ) -> Result<HeaderMap, String> {
        let descriptor = descriptor_for(&self.config.protocol)?;
        let auth_strategy = descriptor.auth.strategy;
        let mut headers = HeaderMap::new();
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
            let token = vertex_token.ok_or_else(|| {
                "Agent Platform OAuth token is required to construct request headers".to_string()
            })?;
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
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
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
        let codec = self.codec().map_err(local_request_error)?;
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
            compatibility_text: None,
            rate_limits: RateLimitTelemetry::default(),
            kind: ProviderChatErrorKind::Transport,
        })?;
        let status = response.status();
        let mut rate_limits = rate_limits_from_headers(response.headers());
        let text = response.text().await.map_err(|error| ProviderChatError {
            status: Some(status.as_u16()),
            message: error.to_string(),
            compatibility_text: None,
            rate_limits: rate_limits.clone(),
            kind: ProviderChatErrorKind::Transport,
        })?;
        if !status.is_success() {
            merge_retry_after_from_error_body(&mut rate_limits, &text);
            return Err(ProviderChatError {
                status: Some(status.as_u16()),
                message: codec.decode_error(status.as_u16(), &text),
                compatibility_text: Some(text),
                rate_limits,
                kind: ProviderChatErrorKind::HttpStatus,
            });
        }
        let raw = serde_json::from_str(&text).map_err(|error| ProviderChatError {
            status: Some(status.as_u16()),
            message: format!("Invalid JSON response: {error}"),
            compatibility_text: None,
            rate_limits: rate_limits.clone(),
            kind: ProviderChatErrorKind::InvalidResponse,
        })?;
        Ok(JsonResponseMeta {
            raw,
            status: status.as_u16(),
            rate_limits,
        })
    }

    async fn request_json_with_gate<G: ChatAttemptGate>(
        &self,
        encoded: EncodedRequest,
        cost: RequestCost,
        model_id: &str,
        gate: &G,
        context: ChatAttemptContext,
        cancellation: &CancellationToken,
    ) -> Result<GatedJsonResponseMeta<G::Reservation>, ProviderChatError> {
        let codec = self.codec().map_err(local_request_error)?;
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
        let reservation = gate
            .acquire(context, cost, cancellation)
            .await
            .map_err(|error| {
                local_request_error(format!(
                    "protocol={} model=\"{}\" estimated_input_tokens={} completion_tokens={} reserved_total_tokens={}: {error}",
                    self.config.protocol.as_str(),
                    model_id,
                    cost.estimated_input_tokens,
                    cost.completion_tokens,
                    cost.total_tokens,
                ))
            })?;
        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                let mut chat_error = ProviderChatError {
                    status: None,
                    message: error.to_string(),
                    compatibility_text: None,
                    rate_limits: RateLimitTelemetry::default(),
                    kind: ProviderChatErrorKind::Transport,
                };
                append_settlement_error(
                    &mut chat_error,
                    gate.settle(
                        reservation,
                        ChatAttemptOutcome {
                            status: None,
                            rate_limits: RateLimitTelemetry::default(),
                            actual_total_tokens: None,
                        },
                    )
                    .await,
                );
                return Err(chat_error);
            }
        };
        let status = response.status();
        let mut rate_limits = rate_limits_from_headers(response.headers());
        let text = match response.text().await {
            Ok(text) => text,
            Err(error) => {
                let mut chat_error = ProviderChatError {
                    status: Some(status.as_u16()),
                    message: error.to_string(),
                    compatibility_text: None,
                    rate_limits: rate_limits.clone(),
                    kind: ProviderChatErrorKind::Transport,
                };
                append_settlement_error(
                    &mut chat_error,
                    gate.settle(
                        reservation,
                        ChatAttemptOutcome {
                            status: Some(status.as_u16()),
                            rate_limits,
                            actual_total_tokens: None,
                        },
                    )
                    .await,
                );
                return Err(chat_error);
            }
        };
        if !status.is_success() {
            merge_retry_after_from_error_body(&mut rate_limits, &text);
            let mut chat_error = ProviderChatError {
                status: Some(status.as_u16()),
                message: codec.decode_error(status.as_u16(), &text),
                compatibility_text: Some(text),
                rate_limits: rate_limits.clone(),
                kind: ProviderChatErrorKind::HttpStatus,
            };
            append_settlement_error(
                &mut chat_error,
                gate.settle(
                    reservation,
                    ChatAttemptOutcome {
                        status: Some(status.as_u16()),
                        rate_limits,
                        actual_total_tokens: None,
                    },
                )
                .await,
            );
            return Err(chat_error);
        }
        let raw = match serde_json::from_str(&text) {
            Ok(raw) => raw,
            Err(error) => {
                let mut chat_error = ProviderChatError {
                    status: Some(status.as_u16()),
                    message: format!("Invalid JSON response: {error}"),
                    compatibility_text: None,
                    rate_limits: rate_limits.clone(),
                    kind: ProviderChatErrorKind::InvalidResponse,
                };
                append_settlement_error(
                    &mut chat_error,
                    gate.settle(
                        reservation,
                        ChatAttemptOutcome {
                            status: Some(status.as_u16()),
                            rate_limits,
                            actual_total_tokens: None,
                        },
                    )
                    .await,
                );
                return Err(chat_error);
            }
        };
        Ok(GatedJsonResponseMeta {
            meta: JsonResponseMeta {
                raw,
                status: status.as_u16(),
                rate_limits,
            },
            reservation,
            cost,
        })
    }

    pub async fn send_prepared_chat_with_gate<G: ChatAttemptGate>(
        &self,
        prepared: &PreparedChatRequest,
        gate: &G,
        context: ChatAttemptContext,
        cancellation: &CancellationToken,
    ) -> Result<ProviderChatMeta, ProviderChatError> {
        let gated = match self
            .request_json_with_gate(
                prepared.encoded.clone(),
                prepared.cost,
                &prepared.request.model,
                gate,
                context,
                cancellation,
            )
            .await
        {
            Ok(meta) => meta,
            Err(original_error) => {
                let descriptor =
                    descriptor_for(&self.config.protocol).map_err(local_request_error)?;
                let Some(mut plan) = plan_retry(
                    Some(descriptor.wire_family),
                    &self.config.base_url,
                    &prepared.request.model,
                    &prepared.encoded,
                    negotiation_failure(&original_error),
                    0,
                    !prepared.request.stream,
                ) else {
                    return Err(original_error);
                };
                let retry_cost = request_cost(
                    &plan.request,
                    prepared.completion_budget.total_output_tokens,
                )
                .map_err(local_request_error)?;
                match self
                    .request_json_with_gate(
                        plan.request,
                        retry_cost,
                        &prepared.request.model,
                        gate,
                        context.compatibility_retry(),
                        cancellation,
                    )
                    .await
                {
                    Ok(meta) => {
                        plan.audit.record_success(meta.meta.status);
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
        let finish_reason = codec.finish_reason(&gated.meta.raw);
        let mut response = match codec.decode_chat(gated.meta.raw) {
            Ok(response) => response,
            Err(message) => {
                let mut error = ProviderChatError {
                    status: Some(gated.meta.status),
                    message,
                    compatibility_text: None,
                    rate_limits: gated.meta.rate_limits.clone(),
                    kind: ProviderChatErrorKind::InvalidResponse,
                };
                append_settlement_error(
                    &mut error,
                    gate.settle(
                        gated.reservation,
                        ChatAttemptOutcome {
                            status: Some(gated.meta.status),
                            rate_limits: gated.meta.rate_limits,
                            actual_total_tokens: None,
                        },
                    )
                    .await,
                );
                return Err(error);
            }
        };
        if let Err(message) =
            finalize_response_usage(&mut response, gated.cost.estimated_input_tokens)
        {
            let mut error = local_request_error(message);
            append_settlement_error(
                &mut error,
                gate.settle(
                    gated.reservation,
                    ChatAttemptOutcome {
                        status: Some(gated.meta.status),
                        rate_limits: gated.meta.rate_limits,
                        actual_total_tokens: None,
                    },
                )
                .await,
            );
            return Err(error);
        }
        let actual_total_tokens = response.usage.as_ref().map(|usage| usage.total_tokens);
        gate.settle(
            gated.reservation,
            ChatAttemptOutcome {
                status: Some(gated.meta.status),
                rate_limits: gated.meta.rate_limits.clone(),
                actual_total_tokens,
            },
        )
        .await
        .map_err(local_request_error)?;
        Ok(ProviderChatMeta {
            response,
            status: gated.meta.status,
            rate_limits: gated.meta.rate_limits,
            finish_reason,
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
                compatibility_text: None,
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
        if !request.stream {
            return Err("Stream chat requires request.stream to be true".into());
        }
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

fn request_cost(encoded: &EncodedRequest, completion_tokens: u32) -> Result<RequestCost, String> {
    let body = encoded
        .body
        .as_ref()
        .ok_or_else(|| "Chat request body is missing".to_string())?;
    let serialized = serde_json::to_string(body)
        .map_err(|error| format!("Failed to serialize the final chat request body: {error}"))?;
    let estimated_input_tokens = u64::try_from(count_tokens(&serialized))
        .map_err(|_| "Estimated chat input token count exceeds u64".to_string())?;
    let completion_tokens = u64::from(completion_tokens);
    let total_tokens = estimated_input_tokens
        .checked_add(completion_tokens)
        .ok_or_else(|| "Estimated chat request token count overflows u64".to_string())?;
    Ok(RequestCost {
        estimated_input_tokens,
        completion_tokens,
        total_tokens,
    })
}

fn append_settlement_error(error: &mut ProviderChatError, settlement: Result<(), String>) {
    if let Err(settlement) = settlement {
        error.message = format!(
            "{}; request quota settlement failed: {settlement}",
            error.message
        );
    }
}

fn finalize_response_usage(
    response: &mut UnifiedChatResponse,
    estimated_input_tokens: u64,
) -> Result<(), String> {
    let visible_estimate = u64::try_from(count_tokens(&response.text))
        .map_err(|_| "Estimated visible output token count exceeds u64".to_string())?;
    let thinking_estimate = u64::try_from(count_tokens(&response.reasoning))
        .map_err(|_| "Estimated thinking token count exceeds u64".to_string())?;

    match response.usage.as_mut() {
        Some(usage) => {
            if !usage.provenance.input_tokens_reported {
                usage.input_tokens = estimated_input_tokens;
            }
            if usage.provenance.output_includes_unreported_thinking {
                let aggregate = usage.output_tokens;
                if thinking_estimate == 0 {
                    usage.output_tokens = aggregate;
                    usage.thinking_tokens = 0;
                } else {
                    let estimated_total = visible_estimate
                        .checked_add(thinking_estimate)
                        .ok_or_else(|| {
                            "Estimated response token count overflows u64".to_string()
                        })?;
                    let thinking = if estimated_total == 0 {
                        0
                    } else {
                        u64::try_from(
                            (u128::from(aggregate) * u128::from(thinking_estimate))
                                / u128::from(estimated_total),
                        )
                        .map_err(|_| "Estimated thinking token split exceeds u64".to_string())?
                    };
                    usage.thinking_tokens = thinking.min(aggregate);
                    usage.output_tokens = aggregate - usage.thinking_tokens;
                }
            } else {
                if !usage.provenance.output_tokens_reported {
                    usage.output_tokens = visible_estimate;
                }
                if !usage.provenance.thinking_tokens_reported {
                    usage.thinking_tokens = thinking_estimate;
                }
            }
            usage.total_tokens = usage
                .input_tokens
                .checked_add(usage.output_tokens)
                .and_then(|total| total.checked_add(usage.thinking_tokens))
                .ok_or_else(|| "Normalized response usage overflows u64".to_string())?;
        }
        None => {
            let total_tokens = estimated_input_tokens
                .checked_add(visible_estimate)
                .and_then(|total| total.checked_add(thinking_estimate))
                .ok_or_else(|| "Estimated response usage overflows u64".to_string())?;
            response.usage = Some(UnifiedUsage {
                input_tokens: estimated_input_tokens,
                output_tokens: visible_estimate,
                cached_tokens: 0,
                thinking_tokens: thinking_estimate,
                total_tokens,
                provenance: UnifiedUsageProvenance::default(),
            });
        }
    }
    Ok(())
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
        compatibility_text: None,
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
        message: error
            .compatibility_text
            .as_deref()
            .unwrap_or(&error.message),
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

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener};
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Arc, Mutex as StdMutex};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum GateEvent {
        Acquired {
            context: ChatAttemptContext,
            cost: RequestCost,
        },
        Settled {
            status: Option<u16>,
            actual_total_tokens: Option<u64>,
        },
    }

    #[derive(Clone, Default)]
    struct RecordingGate {
        events: Arc<StdMutex<Vec<GateEvent>>>,
    }

    impl RecordingGate {
        fn events(&self) -> Vec<GateEvent> {
            self.events.lock().expect("recording gate lock").clone()
        }
    }

    impl ChatAttemptGate for RecordingGate {
        type Reservation = ();

        async fn acquire(
            &self,
            context: ChatAttemptContext,
            cost: RequestCost,
            cancellation: &CancellationToken,
        ) -> Result<Self::Reservation, String> {
            if cancellation.is_cancelled() {
                return Err("cancelled before reservation".into());
            }
            self.events
                .lock()
                .expect("recording gate lock")
                .push(GateEvent::Acquired { context, cost });
            Ok(())
        }

        async fn settle(
            &self,
            _reservation: Self::Reservation,
            outcome: ChatAttemptOutcome,
        ) -> Result<(), String> {
            self.events
                .lock()
                .expect("recording gate lock")
                .push(GateEvent::Settled {
                    status: outcome.status,
                    actual_total_tokens: outcome.actual_total_tokens,
                });
            Ok(())
        }
    }

    fn config(id: &str) -> ProviderRuntimeConfig {
        let descriptor = crate::providers::registry::descriptor_by_id(id)
            .expect("valid registry")
            .expect("registered protocol");
        ProviderRuntimeConfig {
            protocol: crate::domain::ProtocolId::registered(id),
            base_url: descriptor.default_base_url.into(),
            use_raw_base_url: false,
            config: json!({}),
            credential: None,
            custom_headers: Vec::new(),
        }
    }

    fn serve_once(
        status: u16,
        extra_headers: &str,
        body: &'static str,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        let extra_headers = extra_headers.to_owned();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept test request");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request);
            write!(
                stream,
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra_headers}Connection: close\r\n\r\n{body}",
                body.len(),
            )
            .expect("write test response");
        });
        (format!("http://{address}"), server)
    }

    fn serve_sequence(
        responses: Vec<(u16, &'static str)>,
    ) -> (
        SocketAddr,
        Receiver<Vec<String>>,
        std::thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        let (sender, receiver) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().expect("accept test request");
                let mut request = [0_u8; 8192];
                let length = stream.read(&mut request).expect("read test request");
                requests.push(String::from_utf8_lossy(&request[..length]).into_owned());
                write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len(),
                )
                .expect("write test response");
            }
            sender.send(requests).expect("send captured requests");
        });
        (address, receiver, server)
    }

    fn chat_request(stream: bool) -> UnifiedChatRequest {
        UnifiedChatRequest {
            model: "test-model".into(),
            messages: Vec::new(),
            web_search: false,
            thinking: None,
            max_output_tokens: None,
            temperature: None,
            top_p: None,
            stream,
            logprobs: false,
            custom_parameters: json!({}),
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

    #[test]
    fn truncation_finish_reasons_are_case_insensitive_and_explicit() {
        for reason in [
            "length",
            "max_tokens",
            "MAX_TOKENS",
            "max_output_tokens",
            "max_tokens_reached",
            "model_context_window_exceeded",
            "incomplete",
        ] {
            assert!(finish_reason_is_truncation(Some(reason)), "{reason}");
        }
        assert!(!finish_reason_is_truncation(Some("stop")));
        assert!(!finish_reason_is_truncation(None));
    }

    #[test]
    fn truncation_diagnostics_distinguish_omitted_and_disabled_thinking() {
        let adapter = RuntimeAdapter::new(Client::new(), config("anthropic"));
        let mut request = chat_request(false);
        request.model = "claude-fable-5".into();
        request.max_output_tokens = Some(128_000);

        let implicit = adapter.output_truncation_error(&request, "max_tokens");
        assert!(implicit.contains("thinking=omitted/provider-default"));

        request.thinking = Some(crate::domain::ThinkingConfig {
            mode: crate::domain::ThinkingMode::Disabled,
            effort: Some(crate::domain::ThinkingEffort::None),
            budget_tokens: None,
            summary: None,
        });
        let disabled = adapter.output_truncation_error(&request, "max_tokens");
        assert!(disabled.contains("mode=Disabled"));
        assert!(!disabled.contains("omitted/provider-default"));
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

    #[test]
    fn header_order_preserves_custom_auth_and_forces_json_content_type_last() {
        let mut config = config("openai-chat");
        config.credential = Some("default-key".into());
        config.custom_headers = vec![
            ("Authorization".into(), "Custom credential".into()),
            ("Content-Type".into(), "text/plain".into()),
        ];
        let adapter = RuntimeAdapter::new(Client::new(), config);
        let headers = adapter
            .build_headers(
                &[HeaderDirective {
                    name: "content-type".into(),
                    value: "application/problem+json".into(),
                    mode: HeaderMode::Replace,
                }],
                None,
            )
            .expect("headers");

        assert_eq!(headers[AUTHORIZATION], "Custom credential");
        assert_eq!(headers[CONTENT_TYPE], "application/json");
    }

    #[test]
    fn vertex_oauth_has_priority_over_custom_authorization() {
        let mut config = config("vertex-ai");
        config.custom_headers = vec![
            ("Authorization".into(), "Custom credential".into()),
            ("X-Trace".into(), "trace-value".into()),
        ];
        let adapter = RuntimeAdapter::new(Client::new(), config);
        let headers = adapter
            .build_headers(&[], Some("oauth-token"))
            .expect("vertex headers");

        assert_eq!(headers[AUTHORIZATION], "Bearer oauth-token");
        assert_eq!(headers["x-trace"], "trace-value");
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

    #[tokio::test]
    async fn non_stream_model_and_chat_errors_use_the_protocol_decoder() {
        let (base_url, model_server) = serve_once(418, "", r#"{"error":"catalog failed"}"#);
        let mut model_config = config("test-seventh");
        model_config.base_url = base_url;
        let model_error = RuntimeAdapter::new(Client::new(), model_config)
            .list_models()
            .await
            .expect_err("model list error");
        assert_eq!(model_error, "test-seventh HTTP 418: catalog failed");
        model_server.join().expect("model server");

        let (base_url, chat_server) = serve_once(
            422,
            "x-ratelimit-remaining-requests: 0\r\n",
            r#"{"error":"chat failed"}"#,
        );
        let mut chat_config = config("test-seventh");
        chat_config.base_url = base_url;
        let error = RuntimeAdapter::new(Client::new(), chat_config)
            .send_chat_with_meta(&chat_request(false))
            .await
            .expect_err("chat error");
        assert_eq!(error.status, Some(422));
        assert_eq!(error.message, "test-seventh HTTP 422: chat failed");
        assert_eq!(error.rate_limits.request_remaining, Some(0));
        chat_server.join().expect("chat server");
    }

    #[tokio::test]
    async fn compatibility_retry_keeps_raw_error_text_and_replays_once() {
        let (address, requests, server) = serve_sequence(vec![
            (
                400,
                r#"{"error":{"message":"Unsupported parameter: max_tokens"}}"#,
            ),
            (
                200,
                r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}]}"#,
            ),
        ]);
        let client = Client::builder()
            .no_proxy()
            .resolve("api.xiaomimimo.com", address)
            .build()
            .expect("test client");
        let mut runtime_config = config("openai-chat");
        runtime_config.base_url = format!("http://api.xiaomimimo.com:{}/v1", address.port());
        let mut request = chat_request(false);
        request.model = "mimo-v2-pro".into();
        request.custom_parameters = json!({"max_tokens": 8});

        let response = RuntimeAdapter::new(client, runtime_config)
            .send_chat_with_meta(&request)
            .await
            .expect("compatibility retry succeeds");
        assert_eq!(response.response.text, "ok");
        let captured = requests.recv().expect("captured requests");
        server.join().expect("compatibility server");
        assert_eq!(captured.len(), 2);
        let request_body = |raw: &str| {
            serde_json::from_str::<Value>(raw.split_once("\r\n\r\n").expect("HTTP request body").1)
                .expect("JSON request body")
        };
        let original = request_body(&captured[0]);
        let retry = request_body(&captured[1]);
        assert_eq!(original["max_tokens"], 8);
        assert!(original.get("max_completion_tokens").is_none());
        assert_eq!(retry["max_completion_tokens"], 8);
        assert!(retry.get("max_tokens").is_none());

        let error = ProviderChatError {
            status: Some(400),
            message: "protocol-friendly error".into(),
            compatibility_text: Some("Unsupported parameter: max_tokens".into()),
            rate_limits: RateLimitTelemetry::default(),
            kind: ProviderChatErrorKind::HttpStatus,
        };
        assert_eq!(
            negotiation_failure(&error).message,
            "Unsupported parameter: max_tokens"
        );
    }

    #[tokio::test]
    async fn gated_test_protocol_send_acquires_and_settles_once() {
        let (base_url, server) = serve_once(200, "", r#"{"answer":"ok","stop":"done"}"#);
        let mut runtime_config = config("test-seventh");
        runtime_config.base_url = base_url;
        let adapter = RuntimeAdapter::new(Client::new(), runtime_config);
        let prepared = adapter
            .prepare_chat_request(&chat_request(false), 256)
            .expect("prepared test protocol request");
        let gate = RecordingGate::default();
        let context = ChatAttemptContext {
            kind: ChatAttemptKind::Primary,
            logical_attempt: 3,
            compatibility_retry: false,
        };

        let meta = adapter
            .send_prepared_chat_with_gate(&prepared, &gate, context, &CancellationToken::new())
            .await
            .expect("gated test protocol response");
        server.join().expect("test protocol server");

        assert_eq!(meta.response.text, "ok");
        let actual_total = meta
            .response
            .usage
            .as_ref()
            .expect("estimated usage")
            .total_tokens;
        assert_eq!(
            gate.events(),
            vec![
                GateEvent::Acquired {
                    context,
                    cost: prepared.cost,
                },
                GateEvent::Settled {
                    status: Some(200),
                    actual_total_tokens: Some(actual_total),
                },
            ]
        );
    }

    #[test]
    fn prepared_cost_uses_the_final_compact_body_and_counts_completion_once() {
        let adapter = RuntimeAdapter::new(Client::new(), config("anthropic"));
        let mut request = chat_request(false);
        request.model = "claude-sonnet-4-6".into();
        request.web_search = true;
        request.messages = vec![crate::domain::UnifiedMessage {
            role: "user".into(),
            content: vec![crate::domain::UnifiedContent::Text {
                text: "内嵌提示词 + Assistant prompt + 背景与术语表".into(),
            }],
        }];
        request.custom_parameters = json!({
            "max_tokens": 8192,
            "output_config": {
                "format": {
                    "type": "json_schema",
                    "schema": {
                        "type": "object",
                        "properties": {"translatedText": {"type": "string"}}
                    }
                }
            }
        });

        let prepared = adapter
            .prepare_chat_request(&request, 4_096)
            .expect("prepared request");
        let body = prepared.encoded.body.as_ref().expect("request body");
        let compact = serde_json::to_string(body).expect("compact final body");
        let estimated_input_tokens =
            u64::try_from(count_tokens(&compact)).expect("input estimate fits u64");

        assert_eq!(prepared.cost.estimated_input_tokens, estimated_input_tokens);
        assert_eq!(prepared.cost.completion_tokens, 8_192);
        assert_eq!(
            prepared.cost.total_tokens,
            estimated_input_tokens + prepared.cost.completion_tokens
        );
        assert_eq!(body["max_tokens"], 8_192);
        assert!(body.get("max_completion_tokens").is_none());
        assert!(body.pointer("/output_config/format/schema").is_some());
        assert!(body.get("tools").is_some());
        assert!(prepared
            .request
            .custom_parameters
            .get("max_tokens")
            .is_none());
    }

    #[tokio::test]
    async fn gated_compatibility_retry_reestimates_and_reserves_each_wire_send() {
        let (address, requests, server) = serve_sequence(vec![
            (
                400,
                r#"{"error":{"message":"Unsupported parameter: max_tokens"}}"#,
            ),
            (
                200,
                r#"{"choices":[{"message":{"content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":2}}"#,
            ),
        ]);
        let client = Client::builder()
            .no_proxy()
            .resolve("api.xiaomimimo.com", address)
            .build()
            .expect("test client");
        let mut runtime_config = config("openai-chat");
        runtime_config.base_url = format!("http://api.xiaomimimo.com:{}/v1", address.port());
        let adapter = RuntimeAdapter::new(client, runtime_config);
        let mut request = chat_request(false);
        request.model = "mimo-v2-pro".into();
        request.custom_parameters = json!({"max_tokens": 8});
        let completion_budget = adapter
            .validate_and_plan_chat_request(&request, 4)
            .expect("completion budget");
        let encoded = adapter
            .encoded_chat_request(&request)
            .expect("legacy-shaped initial request");
        let cost = request_cost(&encoded, completion_budget.total_output_tokens)
            .expect("initial request cost");
        let prepared = PreparedChatRequest {
            request,
            encoded,
            completion_budget,
            cost,
        };
        let gate = RecordingGate::default();
        let context = ChatAttemptContext {
            kind: ChatAttemptKind::Primary,
            logical_attempt: 1,
            compatibility_retry: false,
        };

        adapter
            .send_prepared_chat_with_gate(&prepared, &gate, context, &CancellationToken::new())
            .await
            .expect("compatibility retry succeeds");
        let captured = requests.recv().expect("captured requests");
        server.join().expect("compatibility server");
        let bodies = captured
            .iter()
            .map(|raw| {
                serde_json::from_str::<Value>(
                    raw.split_once("\r\n\r\n").expect("HTTP request body").1,
                )
                .expect("JSON request body")
            })
            .collect::<Vec<_>>();
        let events = gate.events();
        let acquires = events
            .iter()
            .filter_map(|event| match event {
                GateEvent::Acquired { context, cost } => Some((*context, *cost)),
                GateEvent::Settled { .. } => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(acquires.len(), 2);
        assert!(!acquires[0].0.compatibility_retry);
        assert!(acquires[1].0.compatibility_retry);
        for ((_, cost), body) in acquires.iter().zip(&bodies) {
            let encoded_tokens = u64::try_from(count_tokens(
                &serde_json::to_string(body).expect("compact request body"),
            ))
            .expect("token estimate fits u64");
            assert_eq!(cost.estimated_input_tokens, encoded_tokens);
            assert_eq!(cost.completion_tokens, 8);
            assert_eq!(cost.total_tokens, encoded_tokens + 8);
        }
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, GateEvent::Settled { .. }))
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn gated_http_errors_keep_the_reservation_and_pre_send_cancellation_does_not() {
        let (base_url, server) = serve_once(503, "", r#"{"error":"busy"}"#);
        let mut runtime_config = config("test-seventh");
        runtime_config.base_url = base_url;
        let adapter = RuntimeAdapter::new(Client::new(), runtime_config);
        let prepared = adapter
            .prepare_chat_request(&chat_request(false), 128)
            .expect("prepared request");
        let gate = RecordingGate::default();
        let context = ChatAttemptContext {
            kind: ChatAttemptKind::Primary,
            logical_attempt: 1,
            compatibility_retry: false,
        };
        let error = adapter
            .send_prepared_chat_with_gate(&prepared, &gate, context, &CancellationToken::new())
            .await
            .expect_err("HTTP error");
        server.join().expect("error server");
        assert_eq!(error.status, Some(503));
        assert_eq!(
            gate.events(),
            vec![
                GateEvent::Acquired {
                    context,
                    cost: prepared.cost,
                },
                GateEvent::Settled {
                    status: Some(503),
                    actual_total_tokens: None,
                },
            ]
        );

        let cancelled_gate = RecordingGate::default();
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let error = adapter
            .send_prepared_chat_with_gate(&prepared, &cancelled_gate, context, &cancellation)
            .await
            .expect_err("pre-send cancellation");
        assert_eq!(error.kind, ProviderChatErrorKind::LocalRequest);
        assert!(cancelled_gate.events().is_empty());
    }

    #[tokio::test]
    async fn stream_chat_requires_a_stream_request() {
        let adapter = RuntimeAdapter::new(Client::new(), config("test-seventh"));
        let error = adapter
            .stream_chat(&chat_request(false))
            .await
            .expect_err("non-stream request");
        assert!(error.contains("request.stream"));
    }
}
