// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED

//! Buffer metrics for apps with batching (receiver, loader, archiver).

use metrics::{Counter, Gauge, Histogram};

use super::super::MetricsManager;

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
    pub fn new(manager: &MetricsManager) -> Self {
        Self {
            buffer_bytes: manager.gauge("buffer_bytes", "Current buffer size in bytes"),
            buffer_records: manager.gauge("buffer_records", "Current buffered record count"),
            buffer_flush: manager.counter("buffer_flush_total", "Buffer flush operations"),
            buffer_flush_duration: manager
                .histogram("buffer_flush_duration_seconds", "Buffer flush latency"),
            namespace: manager.namespace().to_string(),
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
