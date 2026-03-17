// Project:   hyperi-rustlib
// File:      src/transport/kafka/metrics.rs
// Purpose:   Kafka metrics collection via librdkafka statistics
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Kafka metrics collection via librdkafka statistics callback.
//!
//! Provides a `StatsContext` implementation that collects librdkafka statistics
//! and exposes them through a `KafkaMetrics` snapshot. Matches the Python
//! `hs_pylib.kafka.KafkaMetricsCollector` API.
//!
//! ## Usage
//!
//! Enable statistics by setting `statistics.interval.ms` in the Kafka config:
//!
//! ```rust,ignore
//! use hyperi_rustlib::transport::kafka::{KafkaConfig, KafkaMetrics, StatsContext};
//! use std::sync::Arc;
//!
//! let stats = Arc::new(StatsContext::new());
//! let mut config = KafkaConfig::default();
//! config.extra_config.insert("statistics.interval.ms".to_string(), "5000".to_string());
//!
//! // Use stats.clone() as the context when creating consumer/producer
//! // Then periodically:
//! let metrics = stats.get_metrics();
//! println!("Messages sent: {}", metrics.messages_sent);
//! println!("Consumer lag: {:?}", metrics.partition_lag);
//! ```

use rdkafka::client::ClientContext;
use rdkafka::config::RDKafkaLogLevel;
use rdkafka::error::KafkaError;
use rdkafka::statistics::Statistics;
use std::collections::HashMap;
use std::sync::RwLock;

/// Kafka metrics snapshot.
///
/// Contains aggregated statistics from librdkafka, matching the Python
/// `KafkaMetrics` dataclass structure.
#[derive(Debug, Clone, Default)]
pub struct KafkaMetrics {
    // --- Client-level metrics ---
    /// Total messages sent (produced).
    pub messages_sent: i64,
    /// Total messages received (consumed).
    pub messages_received: i64,
    /// Total bytes sent.
    pub bytes_sent: i64,
    /// Total bytes received.
    pub bytes_received: i64,
    /// Current messages in producer queue.
    pub queue_message_count: u64,
    /// Current bytes in producer queue.
    pub queue_byte_count: u64,

    // --- Per-broker metrics ---
    /// Per-broker statistics keyed by broker name.
    pub brokers: HashMap<String, BrokerMetrics>,

    // --- Per-partition metrics (consumer) ---
    /// Per-partition statistics keyed by (topic, partition).
    pub partition_lag: HashMap<(String, i32), i64>,
    /// Per-partition committed offsets.
    pub partition_committed: HashMap<(String, i32), i64>,
    /// Per-partition high watermarks.
    pub partition_high_watermark: HashMap<(String, i32), i64>,

    // --- Consumer group metrics ---
    /// Consumer group state (e.g., "up", "rebalancing").
    pub consumer_group_state: Option<String>,
    /// Total number of rebalances.
    pub rebalance_count: i64,
    /// Time since last rebalance in milliseconds.
    pub rebalance_age_ms: i64,

    // --- Timestamp ---
    /// Unix timestamp when these stats were collected.
    pub timestamp: i64,
}

/// Per-broker metrics.
#[derive(Debug, Clone, Default)]
pub struct BrokerMetrics {
    /// Broker state ("UP", "DOWN", "INIT", etc.).
    pub state: String,
    /// Average round-trip time in milliseconds.
    pub rtt_avg_ms: f64,
    /// 99th percentile RTT in milliseconds.
    pub rtt_p99_ms: f64,
    /// Total throttle time in milliseconds.
    pub throttle_time_ms: i64,
    /// Messages in output buffer.
    pub outbuf_msg_cnt: i64,
    /// Requests waiting for response.
    pub waitresp_cnt: i64,
    /// Total requests sent.
    pub requests_sent: u64,
    /// Total responses received.
    pub responses_received: u64,
    /// Total request errors.
    pub request_errors: u64,
}

/// Statistics-collecting client context.
///
/// Implements `ClientContext` to receive librdkafka statistics callbacks.
/// Use with consumers or producers that need metrics collection.
///
/// Thread-safe: can be shared across multiple Kafka clients.
#[derive(Debug)]
pub struct StatsContext {
    stats: RwLock<Option<Statistics>>,
    latest_metrics: RwLock<KafkaMetrics>,
}

impl Default for StatsContext {
    fn default() -> Self {
        Self::new()
    }
}

impl StatsContext {
    /// Create a new statistics context.
    #[must_use]
    pub fn new() -> Self {
        Self {
            stats: RwLock::new(None),
            latest_metrics: RwLock::new(KafkaMetrics::default()),
        }
    }

    /// Get the latest metrics snapshot.
    ///
    /// Returns a clone of the most recently collected metrics.
    #[must_use]
    pub fn get_metrics(&self) -> KafkaMetrics {
        self.latest_metrics
            .read()
            .map(|m| m.clone())
            .unwrap_or_default()
    }

    /// Get the raw librdkafka statistics.
    ///
    /// Returns the full `Statistics` struct from the last callback.
    #[must_use]
    pub fn get_raw_stats(&self) -> Option<Statistics> {
        self.stats.read().ok().and_then(|s| s.clone())
    }

    /// Convert raw statistics to our metrics format.
    fn convert_stats(stats: &Statistics) -> KafkaMetrics {
        let mut metrics = KafkaMetrics {
            messages_sent: stats.txmsgs,
            messages_received: stats.rxmsgs,
            bytes_sent: stats.tx_bytes,
            bytes_received: stats.rx_bytes,
            queue_message_count: stats.msg_cnt,
            queue_byte_count: stats.msg_size,
            timestamp: stats.time,
            ..Default::default()
        };

        // Per-broker metrics
        for (name, broker) in &stats.brokers {
            let rtt_avg_ms = broker.rtt.as_ref().map_or(0.0, |w| w.avg as f64 / 1000.0);
            let rtt_p99_ms = broker.rtt.as_ref().map_or(0.0, |w| w.p99 as f64 / 1000.0);
            let throttle_time_ms = broker.throttle.as_ref().map_or(0, |w| w.sum);

            metrics.brokers.insert(
                name.clone(),
                BrokerMetrics {
                    state: broker.state.clone(),
                    rtt_avg_ms,
                    rtt_p99_ms,
                    throttle_time_ms,
                    outbuf_msg_cnt: broker.outbuf_msg_cnt,
                    waitresp_cnt: broker.waitresp_cnt,
                    requests_sent: broker.tx,
                    responses_received: broker.rx,
                    request_errors: broker.txerrs,
                },
            );
        }

        // Per-partition metrics from topics
        for (topic_name, topic) in &stats.topics {
            for (partition_id, partition) in &topic.partitions {
                let key = (topic_name.clone(), *partition_id);

                // Consumer lag
                if partition.consumer_lag >= 0 {
                    metrics
                        .partition_lag
                        .insert(key.clone(), partition.consumer_lag);
                }

                // Committed offset
                if partition.committed_offset >= 0 {
                    metrics
                        .partition_committed
                        .insert(key.clone(), partition.committed_offset);
                }

                // High watermark
                if partition.hi_offset >= 0 {
                    metrics
                        .partition_high_watermark
                        .insert(key, partition.hi_offset);
                }
            }
        }

        // Consumer group metrics
        if let Some(ref cgrp) = stats.cgrp {
            metrics.consumer_group_state = Some(cgrp.state.clone());
            metrics.rebalance_count = cgrp.rebalance_cnt;
            metrics.rebalance_age_ms = cgrp.rebalance_age;
        }

        metrics
    }
}

impl ClientContext for StatsContext {
    fn stats(&self, statistics: Statistics) {
        // Convert and store metrics
        let metrics = Self::convert_stats(&statistics);

        if let Ok(mut lock) = self.latest_metrics.write() {
            *lock = metrics;
        }

        // Also store raw stats
        if let Ok(mut lock) = self.stats.write() {
            *lock = Some(statistics);
        }
    }

    fn log(&self, level: RDKafkaLogLevel, fac: &str, log_message: &str) {
        // Forward to tracing if available
        match level {
            RDKafkaLogLevel::Emerg
            | RDKafkaLogLevel::Alert
            | RDKafkaLogLevel::Critical
            | RDKafkaLogLevel::Error => {
                #[cfg(feature = "logger")]
                tracing::error!(target: "librdkafka", facility = fac, "{}", log_message);
                #[cfg(not(feature = "logger"))]
                eprintln!("ERROR librdkafka: {} {}", fac, log_message);
            }
            RDKafkaLogLevel::Warning => {
                #[cfg(feature = "logger")]
                tracing::warn!(target: "librdkafka", facility = fac, "{}", log_message);
                #[cfg(not(feature = "logger"))]
                eprintln!("WARN librdkafka: {} {}", fac, log_message);
            }
            RDKafkaLogLevel::Notice | RDKafkaLogLevel::Info => {
                // rdkafka INFO/Notice is too verbose for application-level INFO
                // (statistics JSON every statistics.interval.ms, connection lifecycle, etc.)
                #[cfg(feature = "logger")]
                tracing::debug!(target: "librdkafka", facility = fac, "{}", log_message);
                #[cfg(not(feature = "logger"))]
                {}
            }
            RDKafkaLogLevel::Debug => {
                #[cfg(feature = "logger")]
                tracing::debug!(target: "librdkafka", facility = fac, "{}", log_message);
                #[cfg(not(feature = "logger"))]
                {}
            }
        }
    }

    fn error(&self, error: KafkaError, reason: &str) {
        #[cfg(feature = "logger")]
        tracing::error!(target: "librdkafka", error = %error, "{}", reason);
        #[cfg(not(feature = "logger"))]
        eprintln!("ERROR librdkafka: {}: {}", error, reason);
    }
}

// StatsContext can be used as a ConsumerContext
impl rdkafka::consumer::ConsumerContext for StatsContext {}

/// Calculate total consumer lag across all partitions.
///
/// Helper function to sum lag from a `KafkaMetrics` snapshot.
#[must_use]
pub fn total_consumer_lag(metrics: &KafkaMetrics) -> i64 {
    metrics.partition_lag.values().sum()
}

/// Get brokers in "UP" state.
#[must_use]
pub fn healthy_broker_count(metrics: &KafkaMetrics) -> usize {
    metrics.brokers.values().filter(|b| b.state == "UP").count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stats_context_creation() {
        let ctx = StatsContext::new();
        let metrics = ctx.get_metrics();
        assert_eq!(metrics.messages_sent, 0);
        assert_eq!(metrics.messages_received, 0);
    }

    #[test]
    fn test_kafka_metrics_default() {
        let metrics = KafkaMetrics::default();
        assert_eq!(metrics.messages_sent, 0);
        assert!(metrics.brokers.is_empty());
        assert!(metrics.partition_lag.is_empty());
    }

    #[test]
    fn test_broker_metrics_default() {
        let metrics = BrokerMetrics::default();
        assert_eq!(metrics.state, "");
        assert!(metrics.rtt_avg_ms.abs() < f64::EPSILON);
    }

    #[test]
    fn test_total_consumer_lag() {
        let mut metrics = KafkaMetrics::default();
        metrics.partition_lag.insert(("topic".to_string(), 0), 100);
        metrics.partition_lag.insert(("topic".to_string(), 1), 200);
        metrics.partition_lag.insert(("topic".to_string(), 2), 50);

        assert_eq!(total_consumer_lag(&metrics), 350);
    }

    #[test]
    fn test_healthy_broker_count() {
        let mut metrics = KafkaMetrics::default();
        metrics.brokers.insert(
            "broker1".to_string(),
            BrokerMetrics {
                state: "UP".to_string(),
                ..Default::default()
            },
        );
        metrics.brokers.insert(
            "broker2".to_string(),
            BrokerMetrics {
                state: "DOWN".to_string(),
                ..Default::default()
            },
        );
        metrics.brokers.insert(
            "broker3".to_string(),
            BrokerMetrics {
                state: "UP".to_string(),
                ..Default::default()
            },
        );

        assert_eq!(healthy_broker_count(&metrics), 2);
    }

    #[test]
    fn test_stats_context_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<StatsContext>();
    }
}
