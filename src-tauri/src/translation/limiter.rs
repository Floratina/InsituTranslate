use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

use crate::adapters::RateLimitTelemetry;

fn rate_limit_status(telemetry: &RateLimitTelemetry, window: usize) -> Option<String> {
    if telemetry.has_quota_headers() {
        Some(format!(
            "{}: requests {}/{}, tokens {}/{}, window {}",
            telemetry.source.as_deref().unwrap_or("headers"),
            telemetry
                .request_remaining
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            telemetry
                .request_limit
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            telemetry
                .token_remaining
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            telemetry
                .token_limit
                .map(|value| value.to_string())
                .unwrap_or_else(|| "-".into()),
            window
        ))
    } else {
        Some(format!("aimd: window {window}"))
    }
}

pub(super) async fn current_rate_limit_status(
    telemetry: &RateLimitTelemetry,
    limiter: &AdaptiveLimiter,
    manual_limiter: &Option<Arc<ManualRateLimiter>>,
) -> Option<String> {
    match manual_limiter {
        Some(manual_limiter) => Some(manual_limiter.status().await),
        None => rate_limit_status(telemetry, limiter.window().await),
    }
}

pub(super) struct HeaderQuotaPolicy {
    enabled: bool,
    state: Mutex<Option<RateLimitTelemetry>>,
}

impl HeaderQuotaPolicy {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            state: Mutex::new(None),
        }
    }

    pub(super) async fn wait_duration(&self, estimated_tokens: u64) -> Option<Duration> {
        if !self.enabled {
            return None;
        }
        let sleep_ms = {
            let state = self.state.lock().await;
            state.as_ref().and_then(|telemetry| {
                let mut delay = telemetry.retry_after_ms;
                if telemetry
                    .request_remaining
                    .is_some_and(|remaining| remaining <= 1)
                {
                    delay = delay.max(telemetry.request_reset_ms);
                }
                if telemetry
                    .token_remaining
                    .is_some_and(|remaining| remaining <= estimated_tokens + 128)
                {
                    delay = delay.max(telemetry.token_reset_ms);
                }
                delay
            })
        };
        sleep_ms
            .filter(|value| *value > 0)
            .map(|delay| Duration::from_millis(delay.min(60_000)))
    }

    pub(super) async fn before_request(
        &self,
        estimated_tokens: u64,
        cancellation: &CancellationToken,
    ) -> bool {
        if cancellation.is_cancelled() {
            return false;
        }
        if let Some(delay) = self.wait_duration(estimated_tokens).await {
            tokio::select! {
                _ = tokio::time::sleep(delay) => true,
                _ = cancellation.cancelled() => false,
            }
        } else {
            true
        }
    }

    pub(super) async fn update(&self, telemetry: &RateLimitTelemetry) {
        if !self.enabled {
            return;
        }
        if telemetry.has_quota_headers() || telemetry.retry_after_ms.is_some() {
            *self.state.lock().await = Some(telemetry.clone());
        }
    }
}

pub(super) struct ManualRateLimiter {
    max_requests: u64,
    max_tokens: u64,
    state: Mutex<ManualRateLimiterState>,
}

struct ManualRateLimiterState {
    window_started: Instant,
    requests: u64,
    tokens: u64,
}

impl ManualRateLimiter {
    pub(super) fn new(max_requests: u64, max_tokens: u64) -> Self {
        Self {
            max_requests: max_requests.max(1),
            max_tokens: max_tokens.max(1),
            state: Mutex::new(ManualRateLimiterState {
                window_started: Instant::now(),
                requests: 0,
                tokens: 0,
            }),
        }
    }

    pub(super) async fn reserve_or_delay(&self, estimated_tokens: u64) -> Option<Duration> {
        let estimated_tokens = estimated_tokens.min(self.max_tokens);
        let mut state = self.state.lock().await;
        if state.window_started.elapsed() >= Duration::from_secs(60) {
            state.window_started = Instant::now();
            state.requests = 0;
            state.tokens = 0;
        }
        if state.requests < self.max_requests && state.tokens + estimated_tokens <= self.max_tokens
        {
            state.requests += 1;
            state.tokens += estimated_tokens;
            None
        } else {
            Some(
                Duration::from_secs(60)
                    .saturating_sub(state.window_started.elapsed())
                    .max(Duration::from_millis(25)),
            )
        }
    }

    pub(super) async fn before_request(
        &self,
        estimated_tokens: u64,
        cancellation: &CancellationToken,
    ) -> bool {
        loop {
            if cancellation.is_cancelled() {
                return false;
            }
            match self.reserve_or_delay(estimated_tokens).await {
                Some(delay) => {
                    let completed = tokio::select! {
                        _ = tokio::time::sleep(delay) => true,
                        _ = cancellation.cancelled() => false,
                    };
                    if !completed {
                        return false;
                    }
                }
                None => return true,
            }
        }
    }

    pub(super) async fn status(&self) -> String {
        let state = self.state.lock().await;
        format!(
            "manual: requests {}/{}, tokens {}/{} per minute",
            state.requests, self.max_requests, state.tokens, self.max_tokens
        )
    }
}

pub(super) struct AdaptiveLimiter {
    max: usize,
    adaptive: bool,
    in_flight: AtomicUsize,
    state: Mutex<AdaptiveLimiterState>,
    notify: Notify,
}

struct AdaptiveLimiterState {
    window: usize,
    success_streak: usize,
    header_mode: bool,
}

pub(super) struct AdaptivePermit {
    limiter: Arc<AdaptiveLimiter>,
}

impl Drop for AdaptivePermit {
    fn drop(&mut self) {
        self.limiter.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.limiter.notify.notify_waiters();
    }
}

impl AdaptiveLimiter {
    pub(super) fn new(max: usize, adaptive: bool) -> Self {
        let max = max.max(1);
        Self {
            max,
            adaptive,
            in_flight: AtomicUsize::new(0),
            state: Mutex::new(AdaptiveLimiterState {
                window: if adaptive { 1 } else { max },
                success_streak: 0,
                header_mode: false,
            }),
            notify: Notify::new(),
        }
    }

    pub(super) async fn acquire(
        self: &Arc<Self>,
        cancellation: &CancellationToken,
    ) -> Option<AdaptivePermit> {
        loop {
            if cancellation.is_cancelled() {
                return None;
            }
            let window = self.window().await;
            if self.in_flight.load(Ordering::SeqCst) < window {
                self.in_flight.fetch_add(1, Ordering::SeqCst);
                return Some(AdaptivePermit {
                    limiter: self.clone(),
                });
            }
            tokio::select! {
                _ = self.notify.notified() => {}
                _ = cancellation.cancelled() => return None,
            }
        }
    }

    pub(super) async fn on_result(&self, has_headers: bool, success: bool, rate_limited: bool) {
        if !self.adaptive {
            return;
        }
        let mut state = self.state.lock().await;
        if rate_limited {
            state.header_mode = false;
            state.window = (state.window / 2).max(1);
            state.success_streak = 0;
        } else if has_headers {
            state.header_mode = true;
            state.window = self.max;
            state.success_streak = 0;
        } else if success && !state.header_mode {
            state.success_streak += 1;
            if state.success_streak >= state.window {
                state.window = (state.window + 1).min(self.max);
                state.success_streak = 0;
            }
        }
        self.notify.notify_waiters();
    }

    pub(super) async fn window(&self) -> usize {
        self.state.lock().await.window
    }

    pub(super) fn notify_waiters(&self) {
        self.notify.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn manual_limiter_exposes_wait_before_sleeping() {
        let limiter = ManualRateLimiter::new(1, 1_000);
        assert!(limiter.reserve_or_delay(100).await.is_none());
        let delay = limiter
            .reserve_or_delay(100)
            .await
            .expect("second request should wait for the current window");
        assert!(delay > Duration::from_millis(0));
        assert!(delay <= Duration::from_secs(60));
    }

    #[tokio::test]
    async fn header_quota_exposes_capped_wait_duration() {
        let policy = HeaderQuotaPolicy::new(true);
        policy
            .update(&RateLimitTelemetry {
                request_remaining: Some(0),
                request_reset_ms: Some(90_000),
                ..RateLimitTelemetry::default()
            })
            .await;
        assert_eq!(
            policy.wait_duration(100).await,
            Some(Duration::from_secs(60))
        );
    }

    #[tokio::test]
    async fn manual_limiter_wait_is_cancelled_promptly() {
        let limiter = Arc::new(ManualRateLimiter::new(1, 1_000));
        let cancellation = CancellationToken::new();
        assert!(limiter.before_request(100, &cancellation).await);
        let cancel = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
        });
        let completed = tokio::time::timeout(
            Duration::from_millis(250),
            limiter.before_request(100, &cancellation),
        )
        .await
        .expect("manual limiter cancellation should not wait for the minute window");
        assert!(!completed);
    }

    #[tokio::test]
    async fn manual_limiter_rejects_pre_cancelled_request() {
        let limiter = ManualRateLimiter::new(60, 60_000);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(!limiter.before_request(100, &cancellation).await);
    }

    #[tokio::test]
    async fn header_quota_wait_is_cancelled_promptly() {
        let policy = HeaderQuotaPolicy::new(true);
        policy
            .update(&RateLimitTelemetry {
                retry_after_ms: Some(60_000),
                ..RateLimitTelemetry::default()
            })
            .await;
        let cancellation = CancellationToken::new();
        let cancel = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
        });
        let completed = tokio::time::timeout(
            Duration::from_millis(250),
            policy.before_request(100, &cancellation),
        )
        .await
        .expect("header quota cancellation should not wait for retry-after");
        assert!(!completed);
    }

    #[tokio::test]
    async fn header_quota_rejects_pre_cancelled_request() {
        let policy = HeaderQuotaPolicy::new(false);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(!policy.before_request(100, &cancellation).await);
    }
}
