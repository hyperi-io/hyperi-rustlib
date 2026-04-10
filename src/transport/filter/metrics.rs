// Project:   hyperi-rustlib
// File:      src/transport/filter/metrics.rs
// Purpose:   Metrics for transport-level message filtering
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Filter metrics — counters per direction and action.
//!
//! Uses the `metrics` crate (no-op if no recorder installed).

use super::config::{FilterAction, FilterDirection};

/// Metrics for transport filter operations.
pub struct FilterMetrics {
    _private: (), // force construction via new()
}

impl FilterMetrics {
    /// Create new filter metrics (registers counters on first use via metrics macros).
    #[must_use]
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Record a filter match event.
    pub fn record(&self, direction: FilterDirection, action: FilterAction) {
        let dir = match direction {
            FilterDirection::In => "in",
            FilterDirection::Out => "out",
        };
        let act = match action {
            FilterAction::Drop => "drop",
            FilterAction::Dlq => "dlq",
        };
        metrics::counter!("transport_filtered_total", "direction" => dir, "action" => act)
            .increment(1);
    }
}

impl Default for FilterMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_record_does_not_panic() {
        let metrics = FilterMetrics::new();
        metrics.record(FilterDirection::In, FilterAction::Drop);
        metrics.record(FilterDirection::Out, FilterAction::Dlq);
        metrics.record(FilterDirection::In, FilterAction::Dlq);
        metrics.record(FilterDirection::Out, FilterAction::Drop);
    }

    #[test]
    fn metrics_default_does_not_panic() {
        let _metrics = FilterMetrics::default();
    }
}
