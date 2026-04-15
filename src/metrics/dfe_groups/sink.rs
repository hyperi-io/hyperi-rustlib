// Project:   hyperi-rustlib
// File:      src/metrics/dfe_groups/sink.rs
// Purpose:   DFE sink metrics group
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Sink/insert metrics for DFE apps with a downstream.

use metrics::Gauge;

use super::super::MetricsManager;
use super::super::manifest::{MetricDescriptor, MetricType};

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
        let ns = manager.namespace();

        // sink_duration_seconds — label-based, register descriptor manually
        let dur_key = if ns.is_empty() {
            "sink_duration_seconds".to_string()
        } else {
            format!("{ns}_sink_duration_seconds")
        };
        metrics::describe_histogram!(
            dur_key.clone(),
            metrics::Unit::Seconds,
            "Sink write latency"
        );
        manager.registry().push(MetricDescriptor {
            name: dur_key,
            metric_type: MetricType::Histogram,
            description: "Sink write latency".into(),
            unit: "seconds".into(),
            labels: vec!["backend".into()],
            group: "sink".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });

        // sink_errors_total — label-based
        let err_key = if ns.is_empty() {
            "sink_errors_total".to_string()
        } else {
            format!("{ns}_sink_errors_total")
        };
        metrics::describe_counter!(err_key.clone(), "Sink write errors");
        manager.registry().push(MetricDescriptor {
            name: err_key,
            metric_type: MetricType::Counter,
            description: "Sink write errors".into(),
            unit: String::new(),
            labels: vec!["backend".into()],
            group: "sink".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });

        // bytes_sent_total — label-based
        let bytes_key = if ns.is_empty() {
            "bytes_sent_total".to_string()
        } else {
            format!("{ns}_bytes_sent_total")
        };
        metrics::describe_counter!(bytes_key.clone(), "Bytes sent to sink");
        manager.registry().push(MetricDescriptor {
            name: bytes_key,
            metric_type: MetricType::Counter,
            description: "Bytes sent to sink".into(),
            unit: String::new(),
            labels: vec!["format".into()],
            group: "sink".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });

        Self {
            concurrent_inserts: manager.gauge_with_labels(
                "concurrent_inserts",
                "In-flight insert/write operations",
                &[],
                "sink",
            ),
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
