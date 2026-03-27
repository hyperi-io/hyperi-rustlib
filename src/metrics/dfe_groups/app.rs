// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED

//! Mandatory app-level metrics for every DFE application.

use metrics::{Counter, Gauge};

use super::super::MetricsManager;

/// Mandatory metrics for every DFE application.
///
/// Registers `info`, `start_time_seconds`, record counters, byte counters,
/// memory gauges, and config reload counter — all prefixed with the
/// `MetricsManager` namespace.
#[derive(Clone)]
pub struct AppMetrics {
    pub records_received: Counter,
    pub records_processed: Counter,
    pub records_error: Counter,
    pub bytes_received: Counter,
    pub bytes_written: Counter,
    pub memory_used_bytes: Gauge,
    pub memory_limit_bytes: Gauge,
    pub config_reloads_success: Counter,
    pub config_reloads_error: Counter,
}

impl AppMetrics {
    /// Create and register app metrics.
    ///
    /// `version` and `commit` are emitted as labels on the `info` gauge.
    #[must_use]
    pub fn new(manager: &MetricsManager, version: &str, commit: &str) -> Self {
        // Info metric for service discovery
        let ns = manager.namespace();
        let info_name = if ns.is_empty() {
            "info".to_string()
        } else {
            format!("{ns}_info")
        };
        metrics::describe_gauge!(info_name.clone(), "Application info for service discovery");
        metrics::gauge!(
            info_name,
            "version" => version.to_string(),
            "commit" => commit.to_string()
        )
        .set(1.0);

        // Start time
        let start_time = manager.gauge("start_time_seconds", "Unix timestamp of process start");
        start_time.set(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0),
        );

        Self {
            records_received: manager
                .counter("records_received_total", "Records received from source"),
            records_processed: manager
                .counter("records_processed_total", "Records successfully processed"),
            records_error: manager.counter("records_error_total", "Records that failed processing"),
            bytes_received: manager.counter("bytes_received_total", "Bytes received from source"),
            bytes_written: manager.counter("bytes_written_total", "Bytes written to sink"),
            memory_used_bytes: manager
                .gauge("memory_used_bytes", "Current memory usage (cgroup-aware)"),
            memory_limit_bytes: manager.gauge("memory_limit_bytes", "Effective memory limit"),
            config_reloads_success: {
                let key = if ns.is_empty() {
                    "config_reloads_total".to_string()
                } else {
                    format!("{ns}_config_reloads_total")
                };
                metrics::describe_counter!(key.clone(), "Config reload attempts");
                metrics::counter!(key, "result" => "success")
            },
            config_reloads_error: {
                let key = if ns.is_empty() {
                    "config_reloads_total".to_string()
                } else {
                    format!("{ns}_config_reloads_total")
                };
                metrics::counter!(key, "result" => "error")
            },
        }
    }

    #[inline]
    pub fn record_received(&self, count: u64) {
        self.records_received.increment(count);
    }

    #[inline]
    pub fn record_processed(&self, count: u64) {
        self.records_processed.increment(count);
    }

    #[inline]
    pub fn record_error(&self, count: u64) {
        self.records_error.increment(count);
    }

    #[inline]
    pub fn record_bytes_received(&self, bytes: u64) {
        self.bytes_received.increment(bytes);
    }

    #[inline]
    pub fn record_bytes_written(&self, bytes: u64) {
        self.bytes_written.increment(bytes);
    }

    #[inline]
    pub fn set_memory(&self, used: u64, limit: u64) {
        self.memory_used_bytes.set(used as f64);
        self.memory_limit_bytes.set(limit as f64);
    }

    #[inline]
    pub fn record_config_reload(&self, success: bool) {
        if success {
            self.config_reloads_success.increment(1);
        } else {
            self.config_reloads_error.increment(1);
        }
    }
}
