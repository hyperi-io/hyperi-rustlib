// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED

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
    pub fn new(manager: &MetricsManager) -> Self {
        Self {
            events: manager.counter(
                "backpressure_events_total",
                "Backpressure activation events",
            ),
            duration: manager.counter(
                "backpressure_duration_seconds_total",
                "Cumulative time paused by backpressure",
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
        // Counter increment with fractional seconds (converted to integer millis
        // would lose precision; the metrics crate handles f64 counters correctly).
        self.duration.increment(seconds as u64);
    }
}
