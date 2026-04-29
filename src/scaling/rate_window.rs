// Project:   hyperi-rustlib
// File:      src/scaling/rate_window.rs
// Purpose:   Sliding window rate calculator
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Sliding window rate calculator for scaling components.
//!
//! [`RateWindow`] tracks a monotonic counter over time and computes
//! the rate of change per second. Useful for request rate, message rate,
//! error rate, and similar signals.
//!
//! Thread-safe via `parking_lot::RwLock` — reads are fast and non-blocking.

use std::time::{Duration, Instant};

use parking_lot::RwLock;

/// Sliding window rate calculator.
///
/// Tracks (timestamp, counter_value) samples and computes rate per second
/// over the configured window. Old samples are pruned automatically.
///
/// # Example
///
/// ```rust
/// use hyperi_rustlib::scaling::RateWindow;
/// use std::time::Duration;
///
/// let window = RateWindow::new(Duration::from_secs(60));
///
/// // Record counter values over time
/// window.record(100);
/// // ... some time later ...
/// window.record(200);
///
/// let rate = window.rate_per_second();
/// // Rate = (200 - 100) / elapsed_seconds
/// ```
pub struct RateWindow {
    inner: RwLock<WindowInner>,
}

struct WindowInner {
    samples: Vec<(Instant, u64)>,
    window_size: Duration,
}

impl RateWindow {
    /// Create a new rate window with the given duration.
    ///
    /// Samples older than `window_size` are pruned on each `record()` call.
    #[must_use]
    pub fn new(window_size: Duration) -> Self {
        Self {
            inner: RwLock::new(WindowInner {
                samples: Vec::with_capacity(64),
                window_size,
            }),
        }
    }

    /// Create a rate window with the default 60-second window.
    #[must_use]
    pub fn default_window() -> Self {
        Self::new(Duration::from_mins(1))
    }

    /// Record a monotonic counter value at the current time.
    ///
    /// Old samples outside the window are pruned automatically.
    pub fn record(&self, counter_value: u64) {
        let now = Instant::now();
        let mut inner = self.inner.write();
        let cutoff = now.checked_sub(inner.window_size).unwrap_or(now);
        inner.samples.retain(|&(t, _)| t >= cutoff);
        inner.samples.push((now, counter_value));
    }

    /// Record a counter value at a specific instant (for testing).
    #[cfg(test)]
    fn record_at(&self, at: Instant, counter_value: u64) {
        let mut inner = self.inner.write();
        let cutoff = at.checked_sub(inner.window_size).unwrap_or(at);
        inner.samples.retain(|&(t, _)| t >= cutoff);
        inner.samples.push((at, counter_value));
    }

    /// Compute the rate per second over the current window.
    ///
    /// Returns 0.0 if fewer than 2 samples exist or the time span is zero.
    #[must_use]
    pub fn rate_per_second(&self) -> f64 {
        let inner = self.inner.read();
        if inner.samples.len() < 2 {
            return 0.0;
        }

        let first = inner.samples.first().unwrap();
        let last = inner.samples.last().unwrap();

        let duration = last.0.duration_since(first.0).as_secs_f64();
        if duration <= 0.0 {
            return 0.0;
        }

        let delta = last.1.saturating_sub(first.1) as f64;
        delta / duration
    }

    /// Number of samples currently in the window.
    #[must_use]
    pub fn sample_count(&self) -> usize {
        self.inner.read().samples.len()
    }

    /// Clear all samples.
    pub fn clear(&self) {
        self.inner.write().samples.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_window() {
        let w = RateWindow::default_window();
        assert!((w.rate_per_second()).abs() < f64::EPSILON);
        assert_eq!(w.sample_count(), 0);
    }

    #[test]
    fn test_single_sample() {
        let w = RateWindow::default_window();
        w.record(100);
        assert!((w.rate_per_second()).abs() < f64::EPSILON);
        assert_eq!(w.sample_count(), 1);
    }

    #[test]
    fn test_two_samples_rate() {
        let w = RateWindow::new(Duration::from_mins(1));
        let now = Instant::now();
        w.record_at(now, 0);
        w.record_at(now + Duration::from_secs(10), 1000);

        let rate = w.rate_per_second();
        // 1000 events in 10 seconds = 100/s
        assert!((rate - 100.0).abs() < 0.01, "Expected ~100.0, got {rate}");
    }

    #[test]
    fn test_multiple_samples() {
        let w = RateWindow::new(Duration::from_mins(1));
        let now = Instant::now();
        w.record_at(now, 0);
        w.record_at(now + Duration::from_secs(5), 500);
        w.record_at(now + Duration::from_secs(10), 1000);

        let rate = w.rate_per_second();
        // Rate computed from first to last: 1000 / 10 = 100/s
        assert!((rate - 100.0).abs() < 0.01, "Expected ~100.0, got {rate}");
    }

    #[test]
    fn test_window_pruning() {
        let w = RateWindow::new(Duration::from_secs(5));
        let now = Instant::now();

        // Old sample (before window)
        w.record_at(now.checked_sub(Duration::from_secs(10)).unwrap(), 0);
        // Recent samples (within window)
        w.record_at(now.checked_sub(Duration::from_secs(2)).unwrap(), 800);
        w.record_at(now, 1000);

        // Old sample should be pruned, leaving 2
        assert_eq!(w.sample_count(), 2);

        let rate = w.rate_per_second();
        // (1000 - 800) / 2 = 100/s
        assert!((rate - 100.0).abs() < 0.01, "Expected ~100.0, got {rate}");
    }

    #[test]
    fn test_clear() {
        let w = RateWindow::default_window();
        w.record(100);
        w.record(200);
        assert_eq!(w.sample_count(), 2);

        w.clear();
        assert_eq!(w.sample_count(), 0);
        assert!((w.rate_per_second()).abs() < f64::EPSILON);
    }

    #[test]
    fn test_zero_duration() {
        let w = RateWindow::new(Duration::from_mins(1));
        let now = Instant::now();
        // Two samples at the same instant
        w.record_at(now, 0);
        w.record_at(now, 1000);
        // Should return 0.0, not infinity
        assert!((w.rate_per_second()).abs() < f64::EPSILON);
    }

    #[test]
    fn test_counter_wraparound() {
        let w = RateWindow::new(Duration::from_mins(1));
        let now = Instant::now();
        // Counter value decreases (reset/overflow)
        w.record_at(now, 1000);
        w.record_at(now + Duration::from_secs(10), 500);
        // saturating_sub returns 0
        assert!((w.rate_per_second()).abs() < f64::EPSILON);
    }
}
