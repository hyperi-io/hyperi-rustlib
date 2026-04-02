// Project:   hyperi-rustlib
// File:      src/metrics/dfe.rs
// Purpose:   Standard DFE metric definitions with transport labels
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Standard DFE metrics.
//!
//! Pre-defined metric set for DFE pipeline components (receiver, loader, engine).
//! Call [`DfeMetrics::register`] **after** creating a
//! [`MetricsManager`](super::MetricsManager) — the manager must exist so that
//! platform metrics are automatically captured in the manifest registry.
//!
//! All methods are `#[inline]` and designed for hot-path use.
//!
//! ## Example
//!
//! ```rust,no_run
//! use hyperi_rustlib::metrics::{MetricsManager, DfeMetrics};
//!
//! let mgr = MetricsManager::new("myapp");
//! let dfe = DfeMetrics::register(&mgr);
//!
//! dfe.transport_sent("kafka", 100);
//! dfe.records_received(500);
//! dfe.scaling_pressure(42.0);
//! ```

use super::manifest::{MetricDescriptor, MetricType};

/// Standard DFE metric set.
///
/// Provides labelled counters, gauges, and histograms covering transport,
/// pipeline, records, scaling, spool, and security concerns.
///
/// Construct via [`DfeMetrics::register`] — this describes all metrics with
/// the global recorder AND pushes descriptors into the manifest registry.
pub struct DfeMetrics {
    /// Prevent external construction.
    _private: (),
}

impl DfeMetrics {
    /// Register all DFE metric descriptions with the global recorder and
    /// manifest registry.
    ///
    /// Call this **once** after creating a [`MetricsManager`](super::MetricsManager).
    /// The returned handle is cheaply clonable (it's zero-sized — all recording
    /// goes through the global `metrics!` macros).
    ///
    /// **Breaking change (v1.22):** Now takes `&MetricsManager` to ensure
    /// platform metrics are tightly coupled with the manifest registry.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn register(manager: &super::MetricsManager) -> Self {
        let reg = manager.registry();

        // --- Transport ---
        metrics::describe_counter!(
            "dfe_transport_sent_total",
            "Messages successfully sent to transport"
        );
        metrics::describe_counter!(
            "dfe_transport_send_errors_total",
            "Messages that failed to send"
        );
        metrics::describe_counter!(
            "dfe_transport_backpressured_total",
            "Messages delayed due to backpressure"
        );
        metrics::describe_counter!(
            "dfe_transport_refused_total",
            "Messages refused by transport (circuit open, capacity)"
        );
        metrics::describe_gauge!(
            "dfe_transport_healthy",
            "Transport health (1=healthy, 0=unhealthy)"
        );
        metrics::describe_gauge!(
            "dfe_transport_queue_size",
            "Current number of messages in transport queue"
        );
        metrics::describe_gauge!(
            "dfe_transport_queue_capacity",
            "Maximum transport queue capacity"
        );
        metrics::describe_gauge!(
            "dfe_transport_inflight",
            "Messages currently in-flight (sent but not acked)"
        );
        metrics::describe_histogram!(
            "dfe_transport_send_duration_seconds",
            metrics::Unit::Seconds,
            "Time to send a batch to transport"
        );

        // Push transport descriptors into manifest registry
        for (name, desc, mt) in [
            (
                "dfe_transport_sent_total",
                "Messages successfully sent to transport",
                MetricType::Counter,
            ),
            (
                "dfe_transport_send_errors_total",
                "Messages that failed to send",
                MetricType::Counter,
            ),
            (
                "dfe_transport_backpressured_total",
                "Messages delayed due to backpressure",
                MetricType::Counter,
            ),
            (
                "dfe_transport_refused_total",
                "Messages refused by transport (circuit open, capacity)",
                MetricType::Counter,
            ),
            (
                "dfe_transport_healthy",
                "Transport health (1=healthy, 0=unhealthy)",
                MetricType::Gauge,
            ),
            (
                "dfe_transport_queue_size",
                "Current number of messages in transport queue",
                MetricType::Gauge,
            ),
            (
                "dfe_transport_queue_capacity",
                "Maximum transport queue capacity",
                MetricType::Gauge,
            ),
            (
                "dfe_transport_inflight",
                "Messages currently in-flight (sent but not acked)",
                MetricType::Gauge,
            ),
        ] {
            reg.push(MetricDescriptor {
                name: name.into(),
                metric_type: mt,
                description: desc.into(),
                unit: String::new(),
                labels: vec!["transport".into()],
                group: "platform".into(),
                buckets: None,
                use_cases: vec![],
                dashboard_hint: None,
            });
        }
        reg.push(MetricDescriptor {
            name: "dfe_transport_send_duration_seconds".into(),
            metric_type: MetricType::Histogram,
            description: "Time to send a batch to transport".into(),
            unit: "seconds".into(),
            labels: vec!["transport".into()],
            group: "platform".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });

        // --- Pipeline ---
        metrics::describe_gauge!(
            "dfe_pipeline_ready",
            "Pipeline readiness (1=ready, 0=not ready)"
        );
        metrics::describe_counter!(
            "dfe_pipeline_stall_seconds_total",
            "Cumulative seconds the pipeline was stalled"
        );

        reg.push(MetricDescriptor {
            name: "dfe_pipeline_ready".into(),
            metric_type: MetricType::Gauge,
            description: "Pipeline readiness (1=ready, 0=not ready)".into(),
            unit: String::new(),
            labels: vec![],
            group: "platform".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });
        reg.push(MetricDescriptor {
            name: "dfe_pipeline_stall_seconds_total".into(),
            metric_type: MetricType::Counter,
            description: "Cumulative seconds the pipeline was stalled".into(),
            unit: "seconds".into(),
            labels: vec![],
            group: "platform".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });

        // --- Records ---
        metrics::describe_counter!(
            "dfe_records_received_total",
            "Records received from all sources"
        );
        metrics::describe_counter!(
            "dfe_records_delivered_total",
            "Records successfully delivered to sink"
        );
        metrics::describe_counter!(
            "dfe_records_filtered_total",
            "Records dropped by filter/routing rules"
        );
        metrics::describe_counter!("dfe_records_dlq_total", "Records sent to dead letter queue");

        for (name, desc) in [
            (
                "dfe_records_received_total",
                "Records received from all sources",
            ),
            (
                "dfe_records_delivered_total",
                "Records successfully delivered to sink",
            ),
            (
                "dfe_records_filtered_total",
                "Records dropped by filter/routing rules",
            ),
            ("dfe_records_dlq_total", "Records sent to dead letter queue"),
        ] {
            reg.push(MetricDescriptor {
                name: name.into(),
                metric_type: MetricType::Counter,
                description: desc.into(),
                unit: String::new(),
                labels: vec![],
                group: "platform".into(),
                buckets: None,
                use_cases: vec![],
                dashboard_hint: None,
            });
        }

        // --- Scaling ---
        metrics::describe_gauge!(
            "dfe_scaling_pressure",
            "Normalised scaling pressure (0-100)"
        );
        metrics::describe_gauge!(
            "dfe_scaling_circuit_open",
            "Circuit breaker state (1=open, 0=closed)"
        );
        metrics::describe_gauge!(
            "dfe_scaling_memory_pressure",
            "Memory pressure ratio (0.0-1.0)"
        );

        for (name, desc) in [
            (
                "dfe_scaling_pressure",
                "Normalised scaling pressure (0-100)",
            ),
            (
                "dfe_scaling_circuit_open",
                "Circuit breaker state (1=open, 0=closed)",
            ),
            (
                "dfe_scaling_memory_pressure",
                "Memory pressure ratio (0.0-1.0)",
            ),
        ] {
            reg.push(MetricDescriptor {
                name: name.into(),
                metric_type: MetricType::Gauge,
                description: desc.into(),
                unit: String::new(),
                labels: vec![],
                group: "platform".into(),
                buckets: None,
                use_cases: vec![],
                dashboard_hint: None,
            });
        }

        // --- Spool ---
        metrics::describe_gauge!("dfe_spool_bytes", "Current spool size in bytes");
        metrics::describe_gauge!("dfe_spool_messages", "Current spool message count");
        metrics::describe_gauge!(
            "dfe_spool_disk_available",
            "Available disk space for spool in bytes"
        );

        for (name, desc) in [
            ("dfe_spool_bytes", "Current spool size in bytes"),
            ("dfe_spool_messages", "Current spool message count"),
            (
                "dfe_spool_disk_available",
                "Available disk space for spool in bytes",
            ),
        ] {
            reg.push(MetricDescriptor {
                name: name.into(),
                metric_type: MetricType::Gauge,
                description: desc.into(),
                unit: String::new(),
                labels: vec![],
                group: "platform".into(),
                buckets: None,
                use_cases: vec![],
                dashboard_hint: None,
            });
        }

        // --- Security ---
        metrics::describe_counter!(
            "dfe_auth_failures_total",
            "Authentication failures by reason"
        );
        metrics::describe_counter!(
            "dfe_validation_failures_total",
            "Validation failures by reason"
        );

        reg.push(MetricDescriptor {
            name: "dfe_auth_failures_total".into(),
            metric_type: MetricType::Counter,
            description: "Authentication failures by reason".into(),
            unit: String::new(),
            labels: vec!["reason".into()],
            group: "platform".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });
        reg.push(MetricDescriptor {
            name: "dfe_validation_failures_total".into(),
            metric_type: MetricType::Counter,
            description: "Validation failures by reason".into(),
            unit: String::new(),
            labels: vec!["reason".into()],
            group: "platform".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });

        Self { _private: () }
    }

    // ── Transport ────────────────────────────────────────────────────

    /// Record messages successfully sent to a transport.
    #[inline]
    pub fn transport_sent(&self, transport: &str, count: u64) {
        metrics::counter!("dfe_transport_sent_total", "transport" => transport.to_string())
            .increment(count);
    }

    /// Record send errors for a transport.
    #[inline]
    pub fn transport_send_errors(&self, transport: &str, count: u64) {
        metrics::counter!("dfe_transport_send_errors_total", "transport" => transport.to_string())
            .increment(count);
    }

    /// Record backpressure events for a transport.
    #[inline]
    pub fn transport_backpressured(&self, transport: &str, count: u64) {
        metrics::counter!("dfe_transport_backpressured_total", "transport" => transport.to_string())
            .increment(count);
    }

    /// Record refused messages for a transport.
    #[inline]
    pub fn transport_refused(&self, transport: &str, count: u64) {
        metrics::counter!("dfe_transport_refused_total", "transport" => transport.to_string())
            .increment(count);
    }

    /// Set transport health status.
    #[inline]
    pub fn transport_healthy(&self, transport: &str, healthy: bool) {
        metrics::gauge!("dfe_transport_healthy", "transport" => transport.to_string())
            .set(if healthy { 1.0 } else { 0.0 });
    }

    /// Set current transport queue size.
    #[inline]
    pub fn transport_queue_size(&self, transport: &str, size: f64) {
        metrics::gauge!("dfe_transport_queue_size", "transport" => transport.to_string()).set(size);
    }

    /// Set transport queue capacity.
    #[inline]
    pub fn transport_queue_capacity(&self, transport: &str, capacity: f64) {
        metrics::gauge!("dfe_transport_queue_capacity", "transport" => transport.to_string())
            .set(capacity);
    }

    /// Set in-flight message count for a transport.
    #[inline]
    pub fn transport_inflight(&self, transport: &str, count: f64) {
        metrics::gauge!("dfe_transport_inflight", "transport" => transport.to_string()).set(count);
    }

    /// Record batch send duration for a transport.
    #[inline]
    pub fn transport_send_duration(&self, transport: &str, seconds: f64) {
        metrics::histogram!(
            "dfe_transport_send_duration_seconds",
            "transport" => transport.to_string()
        )
        .record(seconds);
    }

    // ── Pipeline ─────────────────────────────────────────────────────

    /// Set pipeline readiness state.
    #[inline]
    pub fn pipeline_ready(&self, ready: bool) {
        metrics::gauge!("dfe_pipeline_ready").set(if ready { 1.0 } else { 0.0 });
    }

    /// Add stall duration to the cumulative stall counter (whole seconds).
    #[inline]
    pub fn pipeline_stall(&self, seconds: u64) {
        metrics::counter!("dfe_pipeline_stall_seconds_total").increment(seconds);
    }

    // ── Records ──────────────────────────────────────────────────────

    /// Record incoming records.
    #[inline]
    pub fn records_received(&self, count: u64) {
        metrics::counter!("dfe_records_received_total").increment(count);
    }

    /// Record successfully delivered records.
    #[inline]
    pub fn records_delivered(&self, count: u64) {
        metrics::counter!("dfe_records_delivered_total").increment(count);
    }

    /// Record filtered/dropped records.
    #[inline]
    pub fn records_filtered(&self, count: u64) {
        metrics::counter!("dfe_records_filtered_total").increment(count);
    }

    /// Record records sent to dead letter queue.
    #[inline]
    pub fn records_dlq(&self, count: u64) {
        metrics::counter!("dfe_records_dlq_total").increment(count);
    }

    // ── Scaling ──────────────────────────────────────────────────────

    /// Set normalised scaling pressure (0-100).
    #[inline]
    pub fn scaling_pressure(&self, pressure: f64) {
        metrics::gauge!("dfe_scaling_pressure").set(pressure);
    }

    /// Set circuit breaker state.
    #[inline]
    pub fn scaling_circuit_open(&self, open: bool) {
        metrics::gauge!("dfe_scaling_circuit_open").set(if open { 1.0 } else { 0.0 });
    }

    /// Set memory pressure ratio (0.0-1.0).
    #[inline]
    pub fn scaling_memory_pressure(&self, ratio: f64) {
        metrics::gauge!("dfe_scaling_memory_pressure").set(ratio);
    }

    // ── Spool ────────────────────────────────────────────────────────

    /// Set current spool size in bytes.
    #[inline]
    pub fn spool_bytes(&self, bytes: f64) {
        metrics::gauge!("dfe_spool_bytes").set(bytes);
    }

    /// Set current spool message count.
    #[inline]
    pub fn spool_messages(&self, count: f64) {
        metrics::gauge!("dfe_spool_messages").set(count);
    }

    /// Set available disk space for spool.
    #[inline]
    pub fn spool_disk_available(&self, bytes: f64) {
        metrics::gauge!("dfe_spool_disk_available").set(bytes);
    }

    // ── Security ─────────────────────────────────────────────────────

    /// Record authentication failure.
    #[inline]
    pub fn auth_failure(&self, reason: &str) {
        metrics::counter!("dfe_auth_failures_total", "reason" => reason.to_string()).increment(1);
    }

    /// Record validation failure.
    #[inline]
    pub fn validation_failure(&self, reason: &str) {
        metrics::counter!("dfe_validation_failures_total", "reason" => reason.to_string())
            .increment(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_does_not_panic() {
        let mgr = super::super::MetricsManager::new_for_test("test_app");
        let _dfe = DfeMetrics::register(&mgr);
    }

    #[tokio::test]
    async fn test_register_populates_registry() {
        let mgr = super::super::MetricsManager::new_for_test("test_app");
        let _dfe = DfeMetrics::register(&mgr);
        let manifest = mgr.registry().manifest();
        let names: Vec<&str> = manifest.metrics.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"dfe_transport_sent_total"));
        assert!(names.contains(&"dfe_pipeline_ready"));
        assert!(names.contains(&"dfe_records_received_total"));
        assert!(names.contains(&"dfe_scaling_pressure"));
        assert!(names.contains(&"dfe_spool_bytes"));
        assert!(names.contains(&"dfe_auth_failures_total"));
        // All should be group=platform
        for m in &manifest.metrics {
            assert_eq!(m.group, "platform");
        }
        // Transport metrics should have "transport" label
        let sent = manifest
            .metrics
            .iter()
            .find(|m| m.name == "dfe_transport_sent_total")
            .unwrap();
        assert_eq!(sent.labels, vec!["transport"]);
        // Security metrics should have "reason" label
        let auth = manifest
            .metrics
            .iter()
            .find(|m| m.name == "dfe_auth_failures_total")
            .unwrap();
        assert_eq!(auth.labels, vec!["reason"]);
    }

    #[tokio::test]
    async fn test_methods_callable_without_recorder() {
        let mgr = super::super::MetricsManager::new("test_app");
        let dfe = DfeMetrics::register(&mgr);

        dfe.transport_sent("kafka", 1);
        dfe.transport_send_errors("kafka", 1);
        dfe.transport_backpressured("kafka", 1);
        dfe.transport_refused("kafka", 1);
        dfe.transport_healthy("kafka", true);
        dfe.transport_queue_size("kafka", 100.0);
        dfe.transport_queue_capacity("kafka", 1000.0);
        dfe.transport_inflight("kafka", 50.0);
        dfe.transport_send_duration("kafka", 0.042);

        dfe.pipeline_ready(true);
        dfe.pipeline_stall(1);

        dfe.records_received(100);
        dfe.records_delivered(99);
        dfe.records_filtered(1);
        dfe.records_dlq(0);

        dfe.scaling_pressure(42.0);
        dfe.scaling_circuit_open(false);
        dfe.scaling_memory_pressure(0.65);

        dfe.spool_bytes(1024.0);
        dfe.spool_messages(10.0);
        dfe.spool_disk_available(1_000_000.0);

        dfe.auth_failure("invalid_token");
        dfe.validation_failure("missing_field");
    }
}
