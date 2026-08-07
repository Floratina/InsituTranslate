use reqwest::header::HeaderMap;
use serde_json::Value;

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

pub(super) fn rate_limits_from_headers(headers: &HeaderMap) -> RateLimitTelemetry {
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

pub(super) fn merge_retry_after_from_error_body(rate_limits: &mut RateLimitTelemetry, text: &str) {
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
}
