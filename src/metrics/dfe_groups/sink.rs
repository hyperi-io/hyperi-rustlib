// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED

//! Sink/insert metrics for DFE apps with a downstream.

use metrics::Gauge;

use super::super::MetricsManager;

/// Sink write metrics.
///
/// Tracks write latency, errors, bytes sent, and concurrent insert count.
#[derive(Clone)]
pub struct SinkMetrics {
    pub concurrent_inserts: Gauge,
    namespace: String,
}

impl SinkMetrics {
    #[must_use]
    pub fn new(manager: &MetricsManager) -> Self {
        // Pre-describe metrics that take labels
        let ns = manager.namespace();
        let dur_key = if ns.is_empty() {
            "sink_duration_seconds".to_string()
        } else {
            format!("{ns}_sink_duration_seconds")
        };
        metrics::describe_histogram!(dur_key, metrics::Unit::Seconds, "Sink write latency");

        let err_key = if ns.is_empty() {
            "sink_errors_total".to_string()
        } else {
            format!("{ns}_sink_errors_total")
        };
        metrics::describe_counter!(err_key, "Sink write errors");

        let bytes_key = if ns.is_empty() {
            "bytes_sent_total".to_string()
        } else {
            format!("{ns}_bytes_sent_total")
        };
        metrics::describe_counter!(bytes_key, "Bytes sent to sink");

        Self {
            concurrent_inserts: manager
                .gauge("concurrent_inserts", "In-flight insert/write operations"),
            namespace: ns.to_string(),
        }
    }

    /// Record a sink write with backend label.
    #[inline]
    pub fn record_duration(&self, backend: &str, seconds: f64) {
        let key = if self.namespace.is_empty() {
            "sink_duration_seconds".to_string()
        } else {
            format!("{}_sink_duration_seconds", self.namespace)
        };
        metrics::histogram!(key, "backend" => backend.to_string()).record(seconds);
    }

    /// Record a sink write error with backend label.
    #[inline]
    pub fn record_error(&self, backend: &str) {
        let key = if self.namespace.is_empty() {
            "sink_errors_total".to_string()
        } else {
            format!("{}_sink_errors_total", self.namespace)
        };
        metrics::counter!(key, "backend" => backend.to_string()).increment(1);
    }

    /// Record bytes sent with format label.
    #[inline]
    pub fn record_bytes_sent(&self, format: &str, bytes: u64) {
        let key = if self.namespace.is_empty() {
            "bytes_sent_total".to_string()
        } else {
            format!("{}_bytes_sent_total", self.namespace)
        };
        metrics::counter!(key, "format" => format.to_string()).increment(bytes);
    }

    #[inline]
    pub fn set_concurrent_inserts(&self, count: usize) {
        self.concurrent_inserts.set(count as f64);
    }
}
