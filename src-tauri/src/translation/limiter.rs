use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, Notify};

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

    pub(super) async fn before_request(&self, estimated_tokens: u64) {
        if !self.enabled {
            return;
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
        if let Some(delay) = sleep_ms.filter(|value| *value > 0) {
            tokio::time::sleep(Duration::from_millis(delay.min(60_000))).await;
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

    pub(super) async fn before_request(&self, estimated_tokens: u64) {
        let estimated_tokens = estimated_tokens.min(self.max_tokens);
        loop {
            let delay = {
                let mut state = self.state.lock().await;
                if state.window_started.elapsed() >= Duration::from_secs(60) {
                    state.window_started = Instant::now();
                    state.requests = 0;
                    state.tokens = 0;
                }
                if state.requests < self.max_requests
                    && state.tokens + estimated_tokens <= self.max_tokens
                {
                    state.requests += 1;
                    state.tokens += estimated_tokens;
                    None
                } else {
                    Some(Duration::from_secs(60).saturating_sub(state.window_started.elapsed()))
                }
            };
            match delay {
                Some(delay) => tokio::time::sleep(delay.max(Duration::from_millis(25))).await,
                None => return,
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
        interrupted: &AtomicBool,
    ) -> Option<AdaptivePermit> {
        loop {
            if interrupted.load(Ordering::SeqCst) {
                return None;
            }
            let window = self.window().await;
            if self.in_flight.load(Ordering::SeqCst) < window {
                self.in_flight.fetch_add(1, Ordering::SeqCst);
                return Some(AdaptivePermit {
                    limiter: self.clone(),
                });
            }
            self.notify.notified().await;
        }
    }

    pub(super) async fn on_result(&self, has_headers: bool, success: bool, rate_limited: bool) {
        if !self.adaptive {
            return;
        }
        let mut state = self.state.lock().await;
        if has_headers {
            state.header_mode = true;
            state.window = self.max;
            state.success_streak = 0;
        } else if rate_limited {
            state.header_mode = false;
            state.window = (state.window / 2).max(1);
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
