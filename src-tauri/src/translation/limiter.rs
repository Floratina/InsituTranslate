use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

use crate::providers::{
    ChatAttemptContext, ChatAttemptGate, ChatAttemptOutcome, RateLimitTelemetry, RequestCost,
};

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
    state: Mutex<HeaderQuotaState>,
}

struct HeaderQuotaState {
    telemetry: Option<RateLimitTelemetry>,
    request_reset_at: Option<Instant>,
    token_reset_at: Option<Instant>,
    retry_at: Option<Instant>,
    generation: u64,
    next_sequence: u64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct HeaderQuotaReservation {
    generation: u64,
    sequence: u64,
    reserved_tokens: u64,
}

impl HeaderQuotaPolicy {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            state: Mutex::new(HeaderQuotaState {
                telemetry: None,
                request_reset_at: None,
                token_reset_at: None,
                retry_at: None,
                generation: 0,
                next_sequence: 0,
            }),
        }
    }

    fn refresh_expired_windows(state: &mut HeaderQuotaState, now: Instant) {
        if state.request_reset_at.is_some_and(|reset| reset <= now) {
            if let Some(telemetry) = state.telemetry.as_mut() {
                telemetry.request_remaining = telemetry.request_limit;
                telemetry.request_reset_ms = None;
            }
            state.request_reset_at = None;
            state.generation = state.generation.wrapping_add(1);
        }
        if state.token_reset_at.is_some_and(|reset| reset <= now) {
            if let Some(telemetry) = state.telemetry.as_mut() {
                telemetry.token_remaining = telemetry.token_limit;
                telemetry.token_reset_ms = None;
            }
            state.token_reset_at = None;
            state.generation = state.generation.wrapping_add(1);
        }
        if state.retry_at.is_some_and(|retry| retry <= now) {
            state.retry_at = None;
        }
    }

    fn reset_deadline(
        remaining: Option<u64>,
        reset_ms: Option<u64>,
        now: Instant,
    ) -> Option<Instant> {
        reset_ms
            .map(|delay| now + Duration::from_millis(delay))
            .or_else(|| remaining.is_some().then(|| now + Duration::from_secs(60)))
    }

    fn delay_until(deadline: Option<Instant>, now: Instant) -> Option<Duration> {
        deadline
            .filter(|deadline| *deadline > now)
            .map(|deadline| deadline.duration_since(now).min(Duration::from_secs(60)))
    }

    async fn reserve_or_delay(
        &self,
        estimated_tokens: u64,
    ) -> Result<HeaderQuotaReservation, Duration> {
        if !self.enabled {
            return Ok(HeaderQuotaReservation {
                generation: 0,
                sequence: 0,
                reserved_tokens: estimated_tokens,
            });
        }
        let now = Instant::now();
        let mut state = self.state.lock().await;
        Self::refresh_expired_windows(&mut state, now);
        let mut delay = Self::delay_until(state.retry_at, now);
        if let Some(telemetry) = state.telemetry.as_ref() {
            if telemetry.request_remaining == Some(0) {
                delay = delay.max(Self::delay_until(state.request_reset_at, now));
            }
            if telemetry
                .token_remaining
                .is_some_and(|remaining| remaining < estimated_tokens)
            {
                delay = delay.max(Self::delay_until(state.token_reset_at, now));
            }
        }
        if let Some(delay) = delay {
            return Err(delay.max(Duration::from_millis(25)));
        }

        let generation = state.generation;
        let sequence = state.next_sequence;
        state.next_sequence = state.next_sequence.wrapping_add(1);
        if let Some(telemetry) = state.telemetry.as_mut() {
            if let Some(remaining) = telemetry.request_remaining.as_mut() {
                *remaining = remaining.saturating_sub(1);
            }
            if let Some(remaining) = telemetry.token_remaining.as_mut() {
                *remaining = remaining.saturating_sub(estimated_tokens);
            }
        }
        Ok(HeaderQuotaReservation {
            generation,
            sequence,
            reserved_tokens: estimated_tokens,
        })
    }

    pub(super) async fn acquire(
        &self,
        estimated_tokens: u64,
        cancellation: &CancellationToken,
    ) -> Result<HeaderQuotaReservation, String> {
        loop {
            if cancellation.is_cancelled() {
                return Err("Request cancelled while waiting for provider quota".into());
            }
            match self.reserve_or_delay(estimated_tokens).await {
                Ok(reservation) => return Ok(reservation),
                Err(delay) => {
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = cancellation.cancelled() => {
                            return Err("Request cancelled while waiting for provider quota".into());
                        }
                    }
                }
            }
        }
    }

    pub(super) async fn settle(
        &self,
        reservation: HeaderQuotaReservation,
        actual_tokens: Option<u64>,
        telemetry: &RateLimitTelemetry,
    ) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        let now = Instant::now();
        let mut state = self.state.lock().await;
        Self::refresh_expired_windows(&mut state, now);

        if reservation.generation != state.generation {
            return Ok(());
        }

        if !telemetry.has_quota_headers() {
            let token_limit = state.telemetry.as_ref().and_then(|value| value.token_limit);
            if let (Some(actual_tokens), Some(current)) = (
                actual_tokens,
                state
                    .telemetry
                    .as_mut()
                    .and_then(|value| value.token_remaining.as_mut()),
            ) {
                if actual_tokens < reservation.reserved_tokens {
                    *current = current
                        .checked_add(reservation.reserved_tokens - actual_tokens)
                        .ok_or_else(|| {
                            "Header quota token reconciliation overflowed".to_string()
                        })?;
                    if let Some(limit) = token_limit {
                        *current = (*current).min(limit);
                    }
                } else {
                    *current = current.saturating_sub(actual_tokens - reservation.reserved_tokens);
                }
            }
        }

        if telemetry.has_quota_headers() || telemetry.retry_after_ms.is_some() {
            let request_reset_at =
                Self::reset_deadline(telemetry.request_remaining, telemetry.request_reset_ms, now);
            let token_reset_at =
                Self::reset_deadline(telemetry.token_remaining, telemetry.token_reset_ms, now);
            let retry_at = telemetry
                .retry_after_ms
                .map(|delay| now + Duration::from_millis(delay));
            match state.telemetry.as_mut() {
                Some(current) => {
                    current.request_limit = telemetry.request_limit.or(current.request_limit);
                    current.token_limit = telemetry.token_limit.or(current.token_limit);
                    current.request_remaining =
                        min_optional(current.request_remaining, telemetry.request_remaining);
                    current.token_remaining =
                        min_optional(current.token_remaining, telemetry.token_remaining);
                    current.retry_after_ms = telemetry.retry_after_ms.or(current.retry_after_ms);
                    current.source = telemetry.source.clone().or_else(|| current.source.clone());
                    state.request_reset_at =
                        later_deadline(state.request_reset_at, request_reset_at);
                    state.token_reset_at = later_deadline(state.token_reset_at, token_reset_at);
                    state.retry_at = later_deadline(state.retry_at, retry_at);
                }
                None => {
                    state.telemetry = Some(telemetry.clone());
                    state.request_reset_at = request_reset_at;
                    state.token_reset_at = token_reset_at;
                    state.retry_at = retry_at;
                }
            }
        }
        let _ = reservation.sequence;
        Ok(())
    }
}

fn min_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    }
}

fn later_deadline(left: Option<Instant>, right: Option<Instant>) -> Option<Instant> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (left, right) => left.or(right),
    }
}

pub(super) struct ManualRateLimiter {
    max_requests: u64,
    max_tokens: u64,
    state: Mutex<ManualRateLimiterState>,
}

struct ManualRateLimiterState {
    window_started: Instant,
    generation: u64,
    requests: u64,
    tokens: u64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ManualRateLimitReservation {
    generation: u64,
    reserved_tokens: u64,
}

impl ManualRateLimiter {
    pub(super) fn new(max_requests: u64, max_tokens: u64) -> Self {
        Self {
            max_requests: max_requests.max(1),
            max_tokens: max_tokens.max(1),
            state: Mutex::new(ManualRateLimiterState {
                window_started: Instant::now(),
                generation: 0,
                requests: 0,
                tokens: 0,
            }),
        }
    }

    async fn reserve_or_delay(
        &self,
        estimated_tokens: u64,
    ) -> Result<ManualRateLimitReservation, Duration> {
        let mut state = self.state.lock().await;
        if state.window_started.elapsed() >= Duration::from_secs(60) {
            state.window_started = Instant::now();
            state.generation = state.generation.wrapping_add(1);
            state.requests = 0;
            state.tokens = 0;
        }
        let reserved_total = state.tokens.checked_add(estimated_tokens);
        if state.requests < self.max_requests
            && reserved_total.is_some_and(|tokens| tokens <= self.max_tokens)
        {
            state.requests += 1;
            state.tokens = reserved_total.expect("checked above");
            Ok(ManualRateLimitReservation {
                generation: state.generation,
                reserved_tokens: estimated_tokens,
            })
        } else {
            Err(Duration::from_secs(60)
                .saturating_sub(state.window_started.elapsed())
                .max(Duration::from_millis(25)))
        }
    }

    pub(super) async fn acquire(
        &self,
        estimated_tokens: u64,
        cancellation: &CancellationToken,
    ) -> Result<ManualRateLimitReservation, String> {
        if estimated_tokens > self.max_tokens {
            return Err(format!(
                "Estimated request cost ({estimated_tokens} tokens) exceeds the configured manual TPM limit ({} tokens).",
                self.max_tokens
            ));
        }
        loop {
            if cancellation.is_cancelled() {
                return Err("Request cancelled while waiting for manual rate limit".into());
            }
            match self.reserve_or_delay(estimated_tokens).await {
                Err(delay) => {
                    let completed = tokio::select! {
                        _ = tokio::time::sleep(delay) => true,
                        _ = cancellation.cancelled() => false,
                    };
                    if !completed {
                        return Err("Request cancelled while waiting for manual rate limit".into());
                    }
                }
                Ok(reservation) => return Ok(reservation),
            }
        }
    }

    pub(super) async fn settle(
        &self,
        reservation: ManualRateLimitReservation,
        actual_tokens: Option<u64>,
    ) -> Result<(), String> {
        let Some(actual_tokens) = actual_tokens else {
            return Ok(());
        };
        let mut state = self.state.lock().await;
        if state.window_started.elapsed() >= Duration::from_secs(60)
            || reservation.generation != state.generation
        {
            return Ok(());
        }
        if actual_tokens < reservation.reserved_tokens {
            state.tokens = state
                .tokens
                .saturating_sub(reservation.reserved_tokens - actual_tokens);
        } else {
            state.tokens = state
                .tokens
                .checked_add(actual_tokens - reservation.reserved_tokens)
                .ok_or_else(|| "Manual TPM reconciliation overflowed".to_string())?;
        }
        Ok(())
    }

    async fn cancel(&self, reservation: ManualRateLimitReservation) {
        let mut state = self.state.lock().await;
        if reservation.generation == state.generation
            && state.window_started.elapsed() < Duration::from_secs(60)
        {
            state.requests = state.requests.saturating_sub(1);
            state.tokens = state.tokens.saturating_sub(reservation.reserved_tokens);
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
        assert!(limiter.reserve_or_delay(100).await.is_ok());
        let delay = limiter
            .reserve_or_delay(100)
            .await
            .err()
            .expect("second request should wait for the current window");
        assert!(delay > Duration::from_millis(0));
        assert!(delay <= Duration::from_secs(60));
    }

    #[tokio::test]
    async fn header_quota_exposes_capped_wait_duration() {
        let policy = HeaderQuotaPolicy::new(true);
        let reservation = policy
            .acquire(100, &CancellationToken::new())
            .await
            .expect("initial reservation");
        policy
            .settle(
                reservation,
                None,
                &RateLimitTelemetry {
                    request_remaining: Some(0),
                    request_reset_ms: Some(90_000),
                    ..RateLimitTelemetry::default()
                },
            )
            .await
            .expect("header quota settlement");
        assert!(policy.reserve_or_delay(100).await.is_err());
    }

    #[tokio::test]
    async fn manual_limiter_wait_is_cancelled_promptly() {
        let limiter = Arc::new(ManualRateLimiter::new(1, 1_000));
        let cancellation = CancellationToken::new();
        assert!(limiter.acquire(100, &cancellation).await.is_ok());
        let cancel = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
        });
        let completed = tokio::time::timeout(
            Duration::from_millis(250),
            limiter.acquire(100, &cancellation),
        )
        .await
        .expect("manual limiter cancellation should not wait for the minute window");
        assert!(completed.is_err());
    }

    #[tokio::test]
    async fn manual_limiter_rejects_pre_cancelled_request() {
        let limiter = ManualRateLimiter::new(60, 60_000);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(limiter.acquire(100, &cancellation).await.is_err());
    }

    #[tokio::test]
    async fn header_quota_wait_is_cancelled_promptly() {
        let policy = HeaderQuotaPolicy::new(true);
        let reservation = policy
            .acquire(100, &CancellationToken::new())
            .await
            .expect("initial reservation");
        policy
            .settle(
                reservation,
                None,
                &RateLimitTelemetry {
                    retry_after_ms: Some(60_000),
                    ..RateLimitTelemetry::default()
                },
            )
            .await
            .expect("header quota settlement");
        let cancellation = CancellationToken::new();
        let cancel = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel.cancel();
        });
        let completed = tokio::time::timeout(
            Duration::from_millis(250),
            policy.acquire(100, &cancellation),
        )
        .await
        .expect("header quota cancellation should not wait for retry-after");
        assert!(completed.is_err());
    }

    #[tokio::test]
    async fn header_quota_rejects_pre_cancelled_request() {
        let policy = HeaderQuotaPolicy::new(false);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(policy.acquire(100, &cancellation).await.is_err());
    }

    #[tokio::test]
    async fn manual_limiter_rejects_oversized_requests_without_reserving() {
        let limiter = ManualRateLimiter::new(60, 1_000);
        let error = limiter
            .acquire(1_001, &CancellationToken::new())
            .await
            .expect_err("oversized request");
        assert!(error.contains("exceeds"));
        assert_eq!(limiter.state.lock().await.requests, 0);
    }

    #[tokio::test]
    async fn manual_limiter_reconciles_actual_usage() {
        let limiter = ManualRateLimiter::new(60, 1_000);
        let reservation = limiter
            .acquire(500, &CancellationToken::new())
            .await
            .expect("reservation");
        limiter
            .settle(reservation, Some(200))
            .await
            .expect("settlement");
        assert_eq!(limiter.state.lock().await.tokens, 200);
    }

    #[tokio::test]
    async fn manual_limiter_reconciles_larger_usage_and_ignores_old_windows() {
        let limiter = ManualRateLimiter::new(60, 1_000);
        let old = limiter
            .acquire(200, &CancellationToken::new())
            .await
            .expect("old reservation");
        limiter
            .settle(old, Some(500))
            .await
            .expect("larger settlement");
        assert_eq!(limiter.state.lock().await.tokens, 500);

        let old_window = limiter
            .acquire(100, &CancellationToken::new())
            .await
            .expect("reservation before rollover");
        limiter.state.lock().await.window_started = Instant::now() - Duration::from_secs(61);
        let _new_window = limiter
            .acquire(300, &CancellationToken::new())
            .await
            .expect("new-window reservation");
        limiter
            .settle(old_window, Some(1))
            .await
            .expect("stale settlement is ignored");
        let state = limiter.state.lock().await;
        assert_eq!(state.requests, 1);
        assert_eq!(state.tokens, 300);
    }

    #[tokio::test]
    async fn header_quota_atomically_reserves_the_last_request() {
        let policy = Arc::new(HeaderQuotaPolicy::new(true));
        let initial = policy
            .acquire(10, &CancellationToken::new())
            .await
            .expect("initial unknown-quota reservation");
        policy
            .settle(
                initial,
                Some(10),
                &RateLimitTelemetry {
                    request_limit: Some(1),
                    request_remaining: Some(1),
                    request_reset_ms: Some(1_000),
                    ..RateLimitTelemetry::default()
                },
            )
            .await
            .expect("establish header quota");

        let left = {
            let policy = policy.clone();
            tokio::spawn(async move { policy.reserve_or_delay(10).await })
        };
        let right = {
            let policy = policy.clone();
            tokio::spawn(async move { policy.reserve_or_delay(10).await })
        };
        let results = [
            left.await.expect("left task"),
            right.await.expect("right task"),
        ];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    }

    #[tokio::test]
    async fn header_quota_reconciles_tokens_and_keeps_conservative_out_of_order_data() {
        let policy = HeaderQuotaPolicy::new(true);
        let first = policy
            .acquire(10, &CancellationToken::new())
            .await
            .expect("first reservation");
        let second = policy
            .acquire(10, &CancellationToken::new())
            .await
            .expect("second reservation");
        policy
            .settle(
                second,
                Some(10),
                &RateLimitTelemetry {
                    request_limit: Some(10),
                    request_remaining: Some(8),
                    token_limit: Some(1_000),
                    token_remaining: Some(900),
                    request_reset_ms: Some(60_000),
                    token_reset_ms: Some(60_000),
                    ..RateLimitTelemetry::default()
                },
            )
            .await
            .expect("newer response");
        policy
            .settle(
                first,
                Some(10),
                &RateLimitTelemetry {
                    request_limit: Some(10),
                    request_remaining: Some(9),
                    token_limit: Some(1_000),
                    token_remaining: Some(950),
                    request_reset_ms: Some(60_000),
                    token_reset_ms: Some(60_000),
                    ..RateLimitTelemetry::default()
                },
            )
            .await
            .expect("older response");
        assert_eq!(
            policy
                .state
                .lock()
                .await
                .telemetry
                .as_ref()
                .and_then(|telemetry| telemetry.request_remaining),
            Some(8)
        );

        let estimated = policy
            .acquire(500, &CancellationToken::new())
            .await
            .expect("estimated token reservation");
        policy
            .settle(estimated, Some(200), &RateLimitTelemetry::default())
            .await
            .expect("refund unused estimate");
        assert_eq!(
            policy
                .state
                .lock()
                .await
                .telemetry
                .as_ref()
                .and_then(|telemetry| telemetry.token_remaining),
            Some(700)
        );
    }

    #[tokio::test]
    async fn header_quota_ignores_responses_from_an_expired_window() {
        let policy = HeaderQuotaPolicy::new(true);
        let old = policy
            .acquire(10, &CancellationToken::new())
            .await
            .expect("old-window reservation");
        {
            let mut state = policy.state.lock().await;
            state.telemetry = Some(RateLimitTelemetry {
                request_limit: Some(10),
                request_remaining: Some(0),
                ..RateLimitTelemetry::default()
            });
            state.request_reset_at = Some(Instant::now() - Duration::from_millis(1));
        }
        let _new = policy
            .reserve_or_delay(10)
            .await
            .expect("new-window reservation");
        policy
            .settle(
                old,
                None,
                &RateLimitTelemetry {
                    request_limit: Some(10),
                    request_remaining: Some(0),
                    request_reset_ms: Some(60_000),
                    ..RateLimitTelemetry::default()
                },
            )
            .await
            .expect("stale response is ignored");
        assert_eq!(
            policy
                .state
                .lock()
                .await
                .telemetry
                .as_ref()
                .and_then(|telemetry| telemetry.request_remaining),
            Some(9)
        );
    }
}

pub(super) struct TaskChatAttemptGate {
    quota: Arc<HeaderQuotaPolicy>,
    manual: Option<Arc<ManualRateLimiter>>,
}

pub(super) struct TaskChatAttemptReservation {
    header: HeaderQuotaReservation,
    manual: Option<ManualRateLimitReservation>,
}

impl TaskChatAttemptGate {
    pub(super) fn new(
        quota: Arc<HeaderQuotaPolicy>,
        manual: Option<Arc<ManualRateLimiter>>,
    ) -> Self {
        Self { quota, manual }
    }
}

impl ChatAttemptGate for TaskChatAttemptGate {
    type Reservation = TaskChatAttemptReservation;

    async fn acquire(
        &self,
        _context: ChatAttemptContext,
        cost: RequestCost,
        cancellation: &CancellationToken,
    ) -> Result<Self::Reservation, String> {
        let manual = match self.manual.as_ref() {
            Some(limiter) => Some(limiter.acquire(cost.total_tokens, cancellation).await?),
            None => None,
        };
        let header = match self.quota.acquire(cost.total_tokens, cancellation).await {
            Ok(reservation) => reservation,
            Err(error) => {
                if let (Some(limiter), Some(reservation)) = (self.manual.as_ref(), manual) {
                    limiter.cancel(reservation).await;
                }
                return Err(error);
            }
        };
        Ok(TaskChatAttemptReservation { header, manual })
    }

    async fn settle(
        &self,
        reservation: Self::Reservation,
        outcome: ChatAttemptOutcome,
    ) -> Result<(), String> {
        let _status = outcome.status;
        let mut errors = Vec::new();
        if let (Some(limiter), Some(manual)) = (self.manual.as_ref(), reservation.manual) {
            if let Err(error) = limiter.settle(manual, outcome.actual_total_tokens).await {
                errors.push(error);
            }
        }
        if let Err(error) = self
            .quota
            .settle(
                reservation.header,
                outcome.actual_total_tokens,
                &outcome.rate_limits,
            )
            .await
        {
            errors.push(error);
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}
