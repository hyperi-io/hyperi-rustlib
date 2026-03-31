// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED

//! Kafka consumer metrics for DFE apps.

use metrics::{Counter, Gauge, Histogram};

use super::super::MetricsManager;
use super::super::manifest::{MetricDescriptor, MetricType};

/// Kafka consumer metrics.
///
/// Tracks consumer lag, partition assignments, rebalances, poll latency,
/// and offset commits.
#[derive(Clone)]
pub struct ConsumerMetrics {
    pub partitions_assigned: Gauge,
    pub rebalance: Counter,
    pub poll_duration: Histogram,
    pub offsets_committed: Counter,
    namespace: String,
}

impl ConsumerMetrics {
    #[must_use]
    pub fn new(manager: &MetricsManager) -> Self {
        let ns = manager.namespace();

        // consumer_lag — label-based, register descriptor manually
        let lag_key = if ns.is_empty() {
            "consumer_lag".to_string()
        } else {
            format!("{ns}_consumer_lag")
        };
        metrics::describe_gauge!(lag_key.clone(), "Kafka consumer lag per topic/partition");
        manager.registry().push(MetricDescriptor {
            name: lag_key,
            metric_type: MetricType::Gauge,
            description: "Kafka consumer lag per topic/partition".into(),
            unit: String::new(),
            labels: vec!["topic".into(), "partition".into()],
            group: "consumer".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });

        Self {
            partitions_assigned: manager.gauge_with_labels(
                "consumer_partitions_assigned",
                "Current assigned partition count",
                &[],
                "consumer",
            ),
            rebalance: manager.counter_with_labels(
                "consumer_rebalance_total",
                "Consumer group rebalance events",
                &[],
                "consumer",
            ),
            poll_duration: manager.histogram_with_labels(
                "consumer_poll_duration_seconds",
                "Time per Kafka poll/recv call",
                &[],
                "consumer",
                None,
            ),
            offsets_committed: manager.counter_with_labels(
                "offsets_committed_total",
                "Kafka offsets committed after successful processing",
                &[],
                "consumer",
            ),
            namespace: ns.to_string(),
        }
    }

    /// Set consumer lag for a specific topic/partition.
    #[inline]
    pub fn set_lag(&self, topic: &str, partition: i32, lag: i64) {
        let key = if self.namespace.is_empty() {
            "consumer_lag".to_string()
        } else {
            format!("{}_consumer_lag", self.namespace)
        };
        #[allow(clippy::cast_precision_loss)]
        metrics::gauge!(
            key,
            "topic" => topic.to_string(),
            "partition" => partition.to_string()
        )
        .set(lag as f64);
    }

    #[inline]
    pub fn set_partitions_assigned(&self, count: usize) {
        self.partitions_assigned.set(count as f64);
    }

    #[inline]
    pub fn record_rebalance(&self) {
        self.rebalance.increment(1);
    }

    #[inline]
    pub fn record_poll_duration(&self, seconds: f64) {
        self.poll_duration.record(seconds);
    }

    #[inline]
    pub fn record_offsets_committed(&self, count: u64) {
        self.offsets_committed.increment(count);
    }
}
