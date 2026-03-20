// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED

//! Kafka consumer metrics for DFE apps.

use metrics::{Counter, Gauge, Histogram};

use super::super::MetricsManager;

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
    pub fn new(manager: &MetricsManager) -> Self {
        Self {
            partitions_assigned: manager.gauge(
                "consumer_partitions_assigned",
                "Current assigned partition count",
            ),
            rebalance: manager.counter(
                "consumer_rebalance_total",
                "Consumer group rebalance events",
            ),
            poll_duration: manager.histogram(
                "consumer_poll_duration_seconds",
                "Time per Kafka poll/recv call",
            ),
            offsets_committed: manager.counter(
                "offsets_committed_total",
                "Kafka offsets committed after successful processing",
            ),
            namespace: manager.namespace().to_string(),
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
