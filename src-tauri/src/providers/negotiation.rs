use serde::Serialize;

use crate::providers::compatibility::{patch_for_error, CompatibilityChange, CompatibilityContext};
use crate::providers::{EncodedRequest, HttpMethod};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NegotiationFailureKind {
    HttpStatus,
    Authentication,
    RateLimit,
    Transport,
    #[allow(dead_code)]
    UnknownProtocol,
    InvalidResponse,
    LocalRequest,
}

pub struct NegotiationFailure<'a> {
    pub status: Option<u16>,
    pub kind: NegotiationFailureKind,
    pub message: &'a str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NegotiationAudit {
    pub rule_id: String,
    pub original_status: u16,
    pub changes: Vec<CompatibilityChange>,
    pub retry_status: Option<u16>,
    pub retry_succeeded: Option<bool>,
}

impl NegotiationAudit {
    pub fn record_success(&mut self, status: u16) {
        self.retry_status = Some(status);
        self.retry_succeeded = Some(true);
    }

    pub fn record_failure(&mut self, status: Option<u16>) {
        self.retry_status = status;
        self.retry_succeeded = Some(false);
    }
}

pub struct NegotiationPlan {
    pub request: EncodedRequest,
    pub audit: NegotiationAudit,
}

pub fn plan_retry(
    wire_family: Option<&str>,
    base_url: &str,
    model_id: &str,
    encoded: &EncodedRequest,
    failure: NegotiationFailure<'_>,
    attempt: u8,
    replay_safe: bool,
) -> Option<NegotiationPlan> {
    if attempt != 0
        || !replay_safe
        || encoded.method != HttpMethod::Post
        || failure.kind != NegotiationFailureKind::HttpStatus
    {
        return None;
    }
    let wire_family = wire_family?;
    let status = failure.status?;
    let body = encoded.body.as_ref()?;
    let patch = patch_for_error(
        CompatibilityContext {
            wire_family,
            base_url,
            model_id,
        },
        status,
        failure.message,
        body,
    )?;
    let mut request = encoded.clone();
    request.body = Some(patch.body);
    Some(NegotiationPlan {
        request,
        audit: NegotiationAudit {
            rule_id: patch.rule_id.to_string(),
            original_status: status,
            changes: patch.changes,
            retry_status: None,
            retry_succeeded: None,
        },
    })
}

pub fn write_audit(audit: &NegotiationAudit) {
    match serde_json::to_string(audit) {
        Ok(serialized) => eprintln!("Provider compatibility negotiation: {serialized}"),
        Err(error) => eprintln!("Unable to serialize provider negotiation audit: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::providers::codec::HttpMethod;

    fn encoded() -> EncodedRequest {
        EncodedRequest {
            method: HttpMethod::Post,
            url: "https://api.xiaomimimo.com/v1/chat/completions".into(),
            headers: Vec::new(),
            body: Some(json!({"model": "mimo-v2-pro", "max_tokens": 1024})),
        }
    }

    fn failure(kind: NegotiationFailureKind, status: Option<u16>) -> NegotiationFailure<'static> {
        NegotiationFailure {
            status,
            kind,
            message: "Unsupported parameter: max_tokens",
        }
    }

    #[test]
    fn allows_exactly_one_safe_retry() {
        assert!(plan_retry(
            Some("openai-chat"),
            "https://api.xiaomimimo.com/v1",
            "mimo-v2-pro",
            &encoded(),
            failure(NegotiationFailureKind::HttpStatus, Some(400)),
            0,
            true,
        )
        .is_some());
        assert!(plan_retry(
            Some("openai-chat"),
            "https://api.xiaomimimo.com/v1",
            "mimo-v2-pro",
            &encoded(),
            failure(NegotiationFailureKind::HttpStatus, Some(400)),
            1,
            true,
        )
        .is_none());
    }

    #[test]
    fn excludes_non_replayable_and_non_protocol_failures() {
        for kind in [
            NegotiationFailureKind::Authentication,
            NegotiationFailureKind::RateLimit,
            NegotiationFailureKind::Transport,
            NegotiationFailureKind::UnknownProtocol,
            NegotiationFailureKind::InvalidResponse,
            NegotiationFailureKind::LocalRequest,
        ] {
            assert!(plan_retry(
                Some("openai-chat"),
                "https://api.xiaomimimo.com/v1",
                "mimo-v2-pro",
                &encoded(),
                failure(kind, Some(400)),
                0,
                true,
            )
            .is_none());
        }
        assert!(plan_retry(
            None,
            "https://api.xiaomimimo.com/v1",
            "mimo-v2-pro",
            &encoded(),
            failure(NegotiationFailureKind::UnknownProtocol, None),
            0,
            true,
        )
        .is_none());
        assert!(plan_retry(
            Some("openai-chat"),
            "https://api.xiaomimimo.com/v1",
            "mimo-v2-pro",
            &encoded(),
            failure(NegotiationFailureKind::HttpStatus, Some(400)),
            0,
            false,
        )
        .is_none());
    }
}
