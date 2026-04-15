// Project:   hyperi-rustlib
// File:      src/metrics/dfe_groups/backpressure.rs
// Purpose:   DFE backpressure metrics group
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Backpressure metrics.

use metrics::Counter;

use super::super::MetricsManager;

/// Backpressure event tracking.
///
/// Counts backpressure activations and cumulative pause duration.
#[derive(Clone)]
pub struct BackpressureMetrics {
    pub events: Counter,
    pub duration: Counter,
}

impl BackpressureMetrics {
    #[must_use]
    pub fn new(manager: &MetricsManager) -> Self {
        Self {
            events: manager.counter_with_labels(
                "backpressure_events_total",
                "Backpressure activation events",
                &[],
                "backpressure",
            ),
            duration: manager.counter_with_labels(
                "backpressure_duration_seconds_total",
                "Cumulative time paused by backpressure",
                &[],
                "backpressure",
            ),
        }
    }

    #[inline]
    pub fn record_event(&self) {
        self.events.increment(1);
    }

    /// Record backpressure pause duration in seconds.
    #[inline]
    pub fn record_duration(&self, seconds: f64) {
        // Duration is always non-negative and fits in u64 for practical values.
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        self.duration.increment(seconds as u64);
    }
}
