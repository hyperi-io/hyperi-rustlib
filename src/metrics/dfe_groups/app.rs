// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED

//! Mandatory app-level metrics for every DFE application.

use metrics::{Counter, Gauge};

use super::super::MetricsManager;
use super::super::manifest::{MetricDescriptor, MetricType};

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
        manager.set_build_info(version, commit);

        // Info metric for service discovery
        let ns = manager.namespace();
        let info_name = if ns.is_empty() {
            "info".to_string()
        } else {
            format!("{ns}_info")
        };
        metrics::describe_gauge!(info_name.clone(), "Application info for service discovery");
        metrics::gauge!(
            info_name.clone(),
            "version" => version.to_string(),
            "commit" => commit.to_string()
        )
        .set(1.0);
        manager.registry().push(MetricDescriptor {
            name: info_name,
            metric_type: MetricType::Gauge,
            description: "Application info for service discovery".into(),
            unit: String::new(),
            labels: vec!["version".into(), "commit".into()],
            group: "app".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });

        // Start time
        let start_time = manager.gauge_with_labels(
            "start_time_seconds",
            "Unix timestamp of process start",
            &[],
            "app",
        );
        start_time.set(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0),
        );

        // config_reloads_total — label-based, register descriptor manually
        let config_key = if ns.is_empty() {
            "config_reloads_total".to_string()
        } else {
            format!("{ns}_config_reloads_total")
        };
        metrics::describe_counter!(config_key.clone(), "Config reload attempts");
        manager.registry().push(MetricDescriptor {
            name: config_key.clone(),
            metric_type: MetricType::Counter,
            description: "Config reload attempts".into(),
            unit: String::new(),
            labels: vec!["result".into()],
            group: "app".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });

        Self {
            records_received: manager.counter_with_labels(
                "records_received_total",
                "Records received from source",
                &[],
                "app",
            ),
            records_processed: manager.counter_with_labels(
                "records_processed_total",
                "Records successfully processed",
                &[],
                "app",
            ),
            records_error: manager.counter_with_labels(
                "records_error_total",
                "Records that failed processing",
                &[],
                "app",
            ),
            bytes_received: manager.counter_with_labels(
                "bytes_received_total",
                "Bytes received from source",
                &[],
                "app",
            ),
            bytes_written: manager.counter_with_labels(
                "bytes_written_total",
                "Bytes written to sink",
                &[],
                "app",
            ),
            memory_used_bytes: manager.gauge_with_labels(
                "memory_used_bytes",
                "Current memory usage (cgroup-aware)",
                &[],
                "app",
            ),
            memory_limit_bytes: manager.gauge_with_labels(
                "memory_limit_bytes",
                "Effective memory limit",
                &[],
                "app",
            ),
            config_reloads_success: metrics::counter!(config_key.clone(), "result" => "success"),
            config_reloads_error: metrics::counter!(config_key, "result" => "error"),
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
