// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED

//! Buffer metrics for apps with batching (receiver, loader, archiver).

use metrics::{Counter, Gauge, Histogram};

use super::super::MetricsManager;
use super::super::manifest::{MetricDescriptor, MetricType};

/// Default histogram buckets for buffer flush duration.
const BUFFER_FLUSH_BUCKETS: &[f64] = &[0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0];

/// Buffer metrics for DFE apps with batching.
///
/// Tracks buffer depth, flush operations, and flush trigger reasons.
#[derive(Clone)]
pub struct BufferMetrics {
    pub buffer_bytes: Gauge,
    pub buffer_records: Gauge,
    pub buffer_flush: Counter,
    pub buffer_flush_duration: Histogram,
    namespace: String,
}

impl BufferMetrics {
    #[must_use]
    pub fn new(manager: &MetricsManager) -> Self {
        let ns = manager.namespace();

        // buffer_flush_trigger_total — label-based, register descriptor manually
        let trigger_key = if ns.is_empty() {
            "buffer_flush_trigger_total".to_string()
        } else {
            format!("{ns}_buffer_flush_trigger_total")
        };
        metrics::describe_counter!(trigger_key.clone(), "Buffer flush trigger reason");
        manager.registry().push(MetricDescriptor {
            name: trigger_key,
            metric_type: MetricType::Counter,
            description: "Buffer flush trigger reason".into(),
            unit: String::new(),
            labels: vec!["trigger".into()],
            group: "buffer".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });

        Self {
            buffer_bytes: manager.gauge_with_labels(
                "buffer_bytes",
                "Current buffer size in bytes",
                &[],
                "buffer",
            ),
            buffer_records: manager.gauge_with_labels(
                "buffer_records",
                "Current buffered record count",
                &[],
                "buffer",
            ),
            buffer_flush: manager.counter_with_labels(
                "buffer_flush_total",
                "Buffer flush operations",
                &[],
                "buffer",
            ),
            buffer_flush_duration: manager.histogram_with_labels(
                "buffer_flush_duration_seconds",
                "Buffer flush latency",
                &[],
                "buffer",
                Some(BUFFER_FLUSH_BUCKETS),
            ),
            namespace: ns.to_string(),
        }
    }

    #[inline]
    pub fn set_buffer(&self, bytes: usize, records: usize) {
        self.buffer_bytes.set(bytes as f64);
        self.buffer_records.set(records as f64);
    }

    /// Record a flush with its duration and trigger reason.
    ///
    /// `trigger` should be one of: `size`, `age`, `eviction`, `records`.
    #[inline]
    pub fn record_flush(&self, duration_secs: f64, trigger: &str) {
        self.buffer_flush.increment(1);
        self.buffer_flush_duration.record(duration_secs);
        let key = if self.namespace.is_empty() {
            "buffer_flush_trigger_total".to_string()
        } else {
            format!("{}_buffer_flush_trigger_total", self.namespace)
        };
        metrics::counter!(key, "trigger" => trigger.to_string()).increment(1);
    }
}
