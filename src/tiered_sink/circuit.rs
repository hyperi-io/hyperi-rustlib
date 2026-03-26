// Project:   hyperi-rustlib
// File:      src/tiered_sink/circuit.rs
// Purpose:   Circuit breaker for sink health tracking
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Circuit breaker for sink health tracking.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::RwLock;

/// Circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Circuit is closed - requests flow through normally.
    Closed,
    /// Circuit is open - requests are rejected, sink is known unhealthy.
    Open,
    /// Circuit is half-open - one probe request allowed to test recovery.
    HalfOpen,
}

/// Circuit breaker for protecting against unhealthy sinks.
///
/// The circuit breaker tracks consecutive failures and opens when
/// a threshold is reached. After a timeout, it allows a single probe
/// request to test if the sink has recovered.
pub struct CircuitBreaker {
    state: RwLock<CircuitState>,
    consecutive_failures: AtomicU32,
    failure_threshold: u32,
    reset_timeout: Duration,
    last_failure_time: AtomicU64, // epoch millis
    /// Atomic mirror of circuit state for sync health check access.
    /// 0 = Closed, 1 = Open, 2 = HalfOpen.
    health_state: Arc<AtomicU8>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker.
    ///
    /// - `failure_threshold`: Number of consecutive failures before opening
    /// - `reset_timeout`: Time to wait before allowing a probe request
    #[must_use]
    pub fn new(failure_threshold: u32, reset_timeout: Duration) -> Self {
        let health_state = Arc::new(AtomicU8::new(0)); // 0 = Closed

        #[cfg(feature = "health")]
        {
            let hs = Arc::clone(&health_state);
            crate::health::HealthRegistry::register("circuit_breaker", move || {
                match hs.load(Ordering::Relaxed) {
                    0 => crate::health::HealthStatus::Healthy,   // Closed
                    2 => crate::health::HealthStatus::Degraded,  // HalfOpen
                    _ => crate::health::HealthStatus::Unhealthy, // Open
                }
            });
        }

        Self {
            state: RwLock::new(CircuitState::Closed),
            consecutive_failures: AtomicU32::new(0),
            failure_threshold,
            reset_timeout,
            last_failure_time: AtomicU64::new(0),
            health_state,
        }
    }

    /// Sync the atomic health state mirror with the current circuit state.
    fn sync_health_state(&self, state: CircuitState) {
        let val = match state {
            CircuitState::Closed => 0,
            CircuitState::Open => 1,
            CircuitState::HalfOpen => 2,
        };
        self.health_state.store(val, Ordering::Relaxed);
    }

    /// Get current circuit state.
    pub async fn state(&self) -> CircuitState {
        let mut state = self.state.write().await;

        // Check if we should transition from Open to HalfOpen
        if *state == CircuitState::Open {
            let last_failure = self.last_failure_time.load(Ordering::SeqCst);
            let now = current_epoch_millis();
            let elapsed = Duration::from_millis(now.saturating_sub(last_failure));

            if elapsed >= self.reset_timeout {
                *state = CircuitState::HalfOpen;
                self.sync_health_state(*state);
            }
        }

        *state
    }

    /// Check if requests should be allowed through.
    pub async fn is_closed(&self) -> bool {
        self.state().await == CircuitState::Closed
    }

    /// Check if circuit is open (requests should be rejected).
    pub async fn is_open(&self) -> bool {
        self.state().await == CircuitState::Open
    }

    /// Record a successful request.
    pub async fn record_success(&self) {
        let mut state = self.state.write().await;
        self.consecutive_failures.store(0, Ordering::SeqCst);
        *state = CircuitState::Closed;
        self.sync_health_state(*state);
    }

    /// Record a failed request.
    pub async fn record_failure(&self) {
        let failures = self.consecutive_failures.fetch_add(1, Ordering::SeqCst) + 1;
        self.last_failure_time
            .store(current_epoch_millis(), Ordering::SeqCst);

        if failures >= self.failure_threshold {
            let mut state = self.state.write().await;
            *state = CircuitState::Open;
            self.sync_health_state(*state);
        }
    }

    /// Get the number of consecutive failures.
    #[must_use]
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures.load(Ordering::SeqCst)
    }

    /// Reset the circuit breaker to closed state.
    pub async fn reset(&self) {
        self.consecutive_failures.store(0, Ordering::SeqCst);
        let mut state = self.state.write().await;
        *state = CircuitState::Closed;
        self.sync_health_state(*state);
    }
}

impl std::fmt::Debug for CircuitBreaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CircuitBreaker")
            .field("failure_threshold", &self.failure_threshold)
            .field("reset_timeout", &self.reset_timeout)
            .field("consecutive_failures", &self.consecutive_failures())
            .finish_non_exhaustive()
    }
}

fn current_epoch_millis() -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_initial_state_is_closed() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(30));
        assert_eq!(cb.state().await, CircuitState::Closed);
        assert!(cb.is_closed().await);
    }

    #[tokio::test]
    async fn test_opens_after_threshold() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(30));

        cb.record_failure().await;
        assert!(cb.is_closed().await);

        cb.record_failure().await;
        assert!(cb.is_closed().await);

        cb.record_failure().await;
        assert!(cb.is_open().await);
        assert_eq!(cb.consecutive_failures(), 3);
    }

    #[tokio::test]
    async fn test_success_resets_failures() {
        let cb = CircuitBreaker::new(3, Duration::from_secs(30));

        cb.record_failure().await;
        cb.record_failure().await;
        assert_eq!(cb.consecutive_failures(), 2);

        cb.record_success().await;
        assert_eq!(cb.consecutive_failures(), 0);
        assert!(cb.is_closed().await);
    }

    #[tokio::test]
    async fn test_half_open_after_timeout() {
        let cb = CircuitBreaker::new(1, Duration::from_millis(50));

        cb.record_failure().await;
        assert!(cb.is_open().await);

        // Wait for reset timeout
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(cb.state().await, CircuitState::HalfOpen);
    }

    #[tokio::test]
    async fn test_half_open_success_closes() {
        let cb = CircuitBreaker::new(1, Duration::from_millis(10));

        cb.record_failure().await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(cb.state().await, CircuitState::HalfOpen);

        cb.record_success().await;
        assert!(cb.is_closed().await);
    }

    #[tokio::test]
    async fn test_half_open_failure_reopens() {
        let cb = CircuitBreaker::new(1, Duration::from_millis(10));

        cb.record_failure().await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(cb.state().await, CircuitState::HalfOpen);

        cb.record_failure().await;
        assert!(cb.is_open().await);
    }

    #[tokio::test]
    async fn test_reset() {
        let cb = CircuitBreaker::new(1, Duration::from_secs(30));

        cb.record_failure().await;
        assert!(cb.is_open().await);

        cb.reset().await;
        assert!(cb.is_closed().await);
        assert_eq!(cb.consecutive_failures(), 0);
    }
}
