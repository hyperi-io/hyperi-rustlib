// Project:   hyperi-rustlib
// File:      src/logger/helpers.rs
// Purpose:   Log spam protection helpers
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Log spam protection helpers.
//!
//! Atomic helper functions for per-site log rate limiting.
//! Use alongside the global `tracing-throttle` layer for defence-in-depth.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Log on state transition only. Returns true if state changed.
///
/// Use for sustained conditions: memory pressure, circuit breaker, disk full.
/// Log the transition, not every check cycle.
///
/// # Example
/// ```
/// use std::sync::atomic::AtomicBool;
/// use hyperi_rustlib::logger::log_state_change;
///
/// static PRESSURE_HIGH: AtomicBool = AtomicBool::new(false);
/// if log_state_change(&PRESSURE_HIGH, true) {
///     // Only logs once when transitioning to high
/// }
/// ```
#[inline]
pub fn log_state_change(flag: &AtomicBool, new_state: bool) -> bool {
    flag.swap(new_state, Ordering::Relaxed) != new_state
}

/// Log every Nth occurrence. Returns true on first call and every `sample_rate`-th call.
///
/// Use for per-message errors in hot paths: send failures, validation errors.
/// Always increment metrics separately — this only controls log emission.
///
/// # Example
/// ```
/// use std::sync::atomic::AtomicU64;
/// use hyperi_rustlib::logger::log_sampled;
///
/// static SEND_ERRORS: AtomicU64 = AtomicU64::new(0);
/// if log_sampled(&SEND_ERRORS, 1000) {
///     // Logs first occurrence, then every 1000th
/// }
/// ```
#[inline]
pub fn log_sampled(counter: &AtomicU64, sample_rate: u64) -> bool {
    let count = counter.fetch_add(1, Ordering::Relaxed) + 1;
    count == 1 || count.is_multiple_of(sample_rate)
}

/// Log at most once per interval. Returns true if enough time has passed.
///
/// Use for tight recv/poll loop errors: UDP recv, Kafka consumer, health checks.
///
/// # Example
/// ```
/// use std::sync::atomic::AtomicU64;
/// use hyperi_rustlib::logger::log_debounced;
///
/// static LAST_WARN: AtomicU64 = AtomicU64::new(0);
/// if log_debounced(&LAST_WARN, 5000) {
///     // Logs at most once per 5 seconds
/// }
/// ```
#[inline]
pub fn log_debounced(last_epoch_ms: &AtomicU64, min_interval_ms: u64) -> bool {
    let now = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX);
    let last = last_epoch_ms.load(Ordering::Relaxed);
    if now.saturating_sub(last) >= min_interval_ms {
        last_epoch_ms.store(now, Ordering::Relaxed);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_change_transitions() {
        let flag = AtomicBool::new(false);
        // false -> true: changed
        assert!(log_state_change(&flag, true));
        // true -> true: no change
        assert!(!log_state_change(&flag, true));
        // true -> false: changed
        assert!(log_state_change(&flag, false));
        // false -> false: no change
        assert!(!log_state_change(&flag, false));
    }

    #[test]
    fn test_sampled_first_and_nth() {
        let counter = AtomicU64::new(0);
        // First call always logs (count=1)
        assert!(log_sampled(&counter, 1000));
        // Calls 2..999 do not log (998 calls)
        for _ in 0..998 {
            assert!(!log_sampled(&counter, 1000));
        }
        // Call 1000: count=1000, 1000 % 1000 == 0 -> logs
        assert!(log_sampled(&counter, 1000));
        // Call 1001: count=1001, 1001 % 1000 == 1 -> does not log
        assert!(!log_sampled(&counter, 1000));
    }

    #[test]
    fn test_sampled_rate_1() {
        let counter = AtomicU64::new(0);
        for _ in 0..10 {
            assert!(log_sampled(&counter, 1));
        }
    }

    #[test]
    fn test_debounced_first_call() {
        let last = AtomicU64::new(0);
        // First call always returns true (last is 0, epoch is large)
        assert!(log_debounced(&last, 5000));
    }

    #[test]
    fn test_debounced_within_interval() {
        let last = AtomicU64::new(0);
        // First call: logs
        assert!(log_debounced(&last, 60_000)); // 60s interval
        // Immediate second call: suppressed (within 60s)
        assert!(!log_debounced(&last, 60_000));
    }
}
