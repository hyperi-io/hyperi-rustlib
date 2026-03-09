// Project:   hyperi-rustlib
// File:      src/transport/kafka/mod.rs
// Purpose:   High-throughput Kafka transport for PB/day workloads
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # Kafka Transport
//!
//! High-throughput Kafka transport optimized for PB/day batch processing.
//! Uses rdkafka (librdkafka wrapper) with batch-first design.
//!
//! ## Performance Characteristics
//!
//! - **Batch-first**: Designed for 10K+ messages per batch
//! - **Zero-copy where possible**: Minimizes allocations in hot path
//! - **Lock-free topic cache**: Pre-populated, no per-message locking
//! - **Non-blocking batch drain**: Uses zero-timeout poll to drain internal queue
//! - **At-least-once delivery**: Manual commit after processing
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::transport::{KafkaTransport, KafkaConfig, Transport};
//!
//! let config = KafkaConfig {
//!     brokers: vec!["kafka:9092".to_string()],
//!     group: "dfe-loader".to_string(),
//!     topics: vec!["events".to_string()],
//!     ..Default::default()
//! };
//!
//! let transport = KafkaTransport::new(&config).await?;
//!
//! // Batch processing loop
//! loop {
//!     // Poll for up to 10K messages
//!     let batch = transport.recv(10_000).await?;
//!     if batch.is_empty() {
//!         continue;
//!     }
//!
//!     // Process entire batch
//!     process_batch(&batch);
//!
//!     // Commit AFTER successful processing (at-least-once)
//!     let tokens: Vec<_> = batch.iter().map(|m| m.token.clone()).collect();
//!     transport.commit(&tokens).await?;
//! }
//! ```

mod admin;
mod config;
mod metrics;
mod producer;
mod token;

pub use admin::{KafkaAdmin, TopicInfo};
#[allow(deprecated)]
pub use config::{
    merge_with_overrides, KafkaConfig, KafkaProfile, DEVTEST_PROFILE,
    HIGH_THROUGHPUT_CONSUMER_DEFAULTS, LOW_LATENCY_CONSUMER_DEFAULTS, PRODUCER_DEFAULTS,
    PRODUCER_DEVTEST, PRODUCER_EXACTLY_ONCE, PRODUCER_HIGH_THROUGHPUT, PRODUCER_LOW_LATENCY,
    PRODUCTION_PROFILE,
};
pub use metrics::{
    healthy_broker_count, total_consumer_lag, BrokerMetrics, KafkaMetrics, StatsContext,
};
pub use producer::{KafkaProducer, ProducerMetrics, ProducerProfile};
pub use token::KafkaToken;

use super::error::{TransportError, TransportResult};
use super::traits::Transport;
use super::types::{Message, PayloadFormat, SendResult};
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, CommitMode, Consumer, DefaultConsumerContext};
use rdkafka::message::Message as KafkaMessage;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::topic_partition_list::{Offset, TopicPartitionList};
use rdkafka::util::Timeout;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// High-throughput tuning defaults.
///
/// These are optimized for PB/day batch workloads.
pub mod tuning {
    /// Default batch size for recv() - 10K messages.
    pub const DEFAULT_BATCH_SIZE: usize = 10_000;

    /// Maximum time to spend draining the internal queue (ms).
    /// After this, return what we have to maintain responsiveness.
    pub const MAX_DRAIN_MS: u64 = 100;

    /// Poll timeout when queue is empty - triggers network fetch.
    pub const POLL_TIMEOUT_MS: u64 = 50;

    /// Pre-allocated message vector capacity.
    pub const INITIAL_BATCH_CAPACITY: usize = 10_000;
}

/// High-throughput Kafka transport using rdkafka.
///
/// Optimized for batch-oriented consumption at PB/day scale:
/// - Uses `BaseConsumer` for direct poll control
/// - Pre-populates topic cache to eliminate per-message locking
/// - Drains internal queue with zero-timeout polls
/// - Minimizes allocations in hot path
pub struct KafkaTransport {
    consumer: BaseConsumer<DefaultConsumerContext>,
    producer: FutureProducer,
    /// Pre-populated topic cache - populated on construction, read-only after.
    /// Key optimization: no locks in the hot path.
    topic_cache: HashMap<String, Arc<str>>,
    closed: AtomicBool,
    /// Topics we're subscribed to (for cache warming).
    subscribed_topics: Vec<String>,
}

impl KafkaTransport {
    /// Create a new high-throughput Kafka transport.
    ///
    /// The transport is optimized for batch consumption at PB/day scale.
    /// Configuration defaults are tuned for high throughput:
    /// - `fetch.max.bytes`: 50MB (controls network batch size)
    /// - `enable.auto.commit`: false (manual commit for at-least-once)
    ///
    /// # Errors
    ///
    /// Returns error if Kafka client creation fails.
    pub async fn new(config: &KafkaConfig) -> TransportResult<Self> {
        let mut client_config = ClientConfig::new();

        // Required settings
        client_config.set("bootstrap.servers", config.brokers.join(","));
        client_config.set("group.id", &config.group);
        client_config.set("enable.auto.commit", config.enable_auto_commit.to_string());
        client_config.set(
            "auto.commit.interval.ms",
            config.auto_commit_interval_ms.to_string(),
        );
        client_config.set("session.timeout.ms", config.session_timeout_ms.to_string());
        client_config.set(
            "heartbeat.interval.ms",
            config.heartbeat_interval_ms.to_string(),
        );
        client_config.set(
            "max.poll.interval.ms",
            config.max_poll_interval_ms.to_string(),
        );
        client_config.set("fetch.min.bytes", config.fetch_min_bytes.to_string());
        client_config.set("fetch.max.bytes", config.fetch_max_bytes.to_string());
        client_config.set(
            "max.partition.fetch.bytes",
            config.max_partition_fetch_bytes.to_string(),
        );
        client_config.set("auto.offset.reset", &config.auto_offset_reset);
        client_config.set(
            "enable.partition.eof",
            config.enable_partition_eof.to_string(),
        );

        // Apply profile defaults (these can be overridden by librdkafka_overrides)
        let rdkafka_config = config.build_librdkafka_config();
        for (key, value) in &rdkafka_config {
            client_config.set(key, value);
        }

        // Security settings
        client_config.set("security.protocol", &config.security_protocol);
        if let Some(ref mechanism) = config.sasl_mechanism {
            client_config.set("sasl.mechanism", mechanism);
        }
        if let Some(ref username) = config.sasl_username {
            client_config.set("sasl.username", username);
        }
        if let Some(ref password) = config.sasl_password {
            client_config.set("sasl.password", password);
        }

        // TLS settings
        if let Some(ref ca) = config.ssl_ca_location {
            client_config.set("ssl.ca.location", ca);
        }
        if let Some(ref cert) = config.ssl_certificate_location {
            client_config.set("ssl.certificate.location", cert);
        }
        if let Some(ref key) = config.ssl_key_location {
            client_config.set("ssl.key.location", key);
        }
        if config.ssl_skip_verify {
            client_config.set("enable.ssl.certificate.verification", "false");
        }

        // Client ID
        client_config.set("client.id", &config.client_id);

        // Create consumer using BaseConsumer for direct poll control
        let consumer: BaseConsumer<DefaultConsumerContext> = client_config
            .create()
            .map_err(|e| TransportError::Connection(format!("Failed to create consumer: {e}")))?;

        // Subscribe to topics
        let subscribed_topics = config.topics.clone();
        if !subscribed_topics.is_empty() {
            let topics: Vec<&str> = subscribed_topics.iter().map(String::as_str).collect();
            consumer
                .subscribe(&topics)
                .map_err(|e| TransportError::Connection(format!("Failed to subscribe: {e}")))?;
        }

        // Pre-populate topic cache - eliminates locks in hot path
        let mut topic_cache = HashMap::with_capacity(subscribed_topics.len());
        for topic in &subscribed_topics {
            topic_cache.insert(topic.clone(), Arc::from(topic.as_str()));
        }

        // Create producer (for send operations)
        let producer: FutureProducer = client_config
            .create()
            .map_err(|e| TransportError::Connection(format!("Failed to create producer: {e}")))?;

        Ok(Self {
            consumer,
            producer,
            topic_cache,
            closed: AtomicBool::new(false),
            subscribed_topics,
        })
    }

    /// Create transport with custom context for metrics collection.
    ///
    /// Use this when you need statistics collection via `StatsContext`.
    /// Remember to set `statistics.interval.ms` in `extra_config`.
    pub fn new_with_context<C>(config: &KafkaConfig, _context: C) -> TransportResult<Self>
    where
        C: rdkafka::consumer::ConsumerContext + 'static,
    {
        // For now, delegate to the standard constructor
        // Full context support would require generic parameter on struct
        // which complicates the Transport trait implementation
        tokio::runtime::Handle::current().block_on(Self::new(config))
    }
}

impl Transport for KafkaTransport {
    type Token = KafkaToken;

    async fn send(&self, key: &str, payload: &[u8]) -> SendResult {
        if self.closed.load(Ordering::Relaxed) {
            return SendResult::Fatal(TransportError::Closed);
        }

        let record: FutureRecord<'_, str, [u8]> = FutureRecord::to(key).payload(payload);

        match self
            .producer
            .send(record, Timeout::After(Duration::from_secs(5)))
            .await
        {
            Ok(_) => SendResult::Ok,
            Err((err, _)) => {
                let err_str = err.to_string();
                if err_str.contains("queue full") || err_str.contains("Local: Queue full") {
                    SendResult::Backpressured
                } else {
                    SendResult::Fatal(TransportError::Send(err_str))
                }
            }
        }
    }

    /// Receive a batch of messages.
    ///
    /// This is optimized for high-throughput batch processing:
    /// - Uses zero-timeout polls to drain librdkafka's internal queue
    /// - Returns up to `max` messages per call
    /// - Pre-populates topic cache to avoid allocations
    ///
    /// For PB/day workloads, call with `max = 10_000` or higher.
    async fn recv(&self, max: usize) -> TransportResult<Vec<Message<Self::Token>>> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(TransportError::Closed);
        }

        let timeout = Duration::from_millis(tuning::POLL_TIMEOUT_MS);
        let max_msgs = max;

        // Clone topic cache for use - this is a shallow clone (Arc pointers)
        let mut local_cache = self.topic_cache.clone();

        // Poll synchronously - BaseConsumer::poll is thread-safe
        let mut messages = Vec::with_capacity(max_msgs.min(tuning::INITIAL_BATCH_CAPACITY));
        let drain_deadline =
            std::time::Instant::now() + Duration::from_millis(tuning::MAX_DRAIN_MS);

        // Phase 1: Initial blocking poll (triggers network fetch if queue empty)
        if let Some(result) = self.consumer.poll(timeout) {
            match result {
                Ok(msg) => {
                    let topic_str = msg.topic();
                    let topic: Arc<str> = get_or_insert_topic(&mut local_cache, topic_str);
                    let payload = msg.payload().map_or_else(Vec::new, |p| p.to_vec());
                    let partition = msg.partition();
                    let offset = msg.offset();
                    let timestamp_ms = msg.timestamp().to_millis();

                    messages.push(Message {
                        key: Some(topic.clone()),
                        payload,
                        token: KafkaToken::new(topic, partition, offset),
                        timestamp_ms,
                        format: PayloadFormat::Auto,
                    });
                }
                Err(e) => {
                    return Err(TransportError::Recv(e.to_string()));
                }
            }
        } else {
            return Ok(messages);
        }

        // Phase 2: Drain queue with zero-timeout polls
        // This is where the batch magic happens - librdkafka has already
        // fetched a batch from the network, we just drain it fast.
        while messages.len() < max_msgs {
            if std::time::Instant::now() >= drain_deadline {
                break;
            }

            match self.consumer.poll(Duration::ZERO) {
                Some(Ok(msg)) => {
                    let topic_str = msg.topic();
                    let topic: Arc<str> = get_or_insert_topic(&mut local_cache, topic_str);
                    let payload = msg.payload().map_or_else(Vec::new, |p| p.to_vec());
                    let partition = msg.partition();
                    let offset = msg.offset();
                    let timestamp_ms = msg.timestamp().to_millis();

                    messages.push(Message {
                        key: Some(topic.clone()),
                        payload,
                        token: KafkaToken::new(topic, partition, offset),
                        timestamp_ms,
                        format: PayloadFormat::Auto,
                    });
                }
                Some(Err(e)) => {
                    if messages.is_empty() {
                        return Err(TransportError::Recv(e.to_string()));
                    }
                    break;
                }
                None => break,
            }
        }

        Ok(messages)
    }

    /// Commit offsets for processed messages.
    ///
    /// Uses async commit for better throughput. The commit is batched
    /// by partition - only the highest offset per partition is committed.
    async fn commit(&self, tokens: &[Self::Token]) -> TransportResult<()> {
        if tokens.is_empty() {
            return Ok(());
        }

        // Build topic-partition-offset list
        // For each partition, commit the highest offset + 1 (next to be read)
        let mut tpl = TopicPartitionList::new();
        let mut partition_offsets: HashMap<(&str, i32), i64> =
            HashMap::with_capacity(tokens.len() / 100);

        for token in tokens {
            let key = (token.topic.as_ref(), token.partition);
            partition_offsets
                .entry(key)
                .and_modify(|current| {
                    if token.offset > *current {
                        *current = token.offset;
                    }
                })
                .or_insert(token.offset);
        }

        for ((topic, partition), offset) in partition_offsets {
            tpl.add_partition_offset(topic, partition, Offset::Offset(offset + 1))
                .map_err(|e| TransportError::Commit(format!("Failed to build TPL: {e}")))?;
        }

        // Async commit for better throughput
        self.consumer
            .commit(&tpl, CommitMode::Async)
            .map_err(|e| TransportError::Commit(e.to_string()))?;

        Ok(())
    }

    async fn close(&self) -> TransportResult<()> {
        self.closed.store(true, Ordering::Relaxed);
        // rdkafka handles cleanup on drop
        Ok(())
    }

    fn is_healthy(&self) -> bool {
        !self.closed.load(Ordering::Relaxed)
    }

    fn name(&self) -> &'static str {
        "kafka"
    }
}

/// Get or insert topic Arc into cache.
///
/// Inline helper for hot path - avoids method call overhead.
#[inline]
fn get_or_insert_topic(cache: &mut HashMap<String, Arc<str>>, topic: &str) -> Arc<str> {
    if let Some(arc) = cache.get(topic) {
        return arc.clone();
    }
    let arc: Arc<str> = Arc::from(topic);
    cache.insert(topic.to_string(), arc.clone());
    arc
}

impl std::fmt::Debug for KafkaTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KafkaTransport")
            .field("subscribed_topics", &self.subscribed_topics)
            .field("closed", &self.closed.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tuning_constants() {
        assert_eq!(tuning::DEFAULT_BATCH_SIZE, 10_000);
        assert_eq!(tuning::MAX_DRAIN_MS, 100);
        assert_eq!(tuning::POLL_TIMEOUT_MS, 50);
    }

    #[test]
    fn test_get_or_insert_topic_cached() {
        let mut cache = HashMap::new();
        cache.insert("events".to_string(), Arc::from("events"));

        let arc1 = get_or_insert_topic(&mut cache, "events");
        let arc2 = get_or_insert_topic(&mut cache, "events");

        // Should return same Arc (pointer equality)
        assert!(Arc::ptr_eq(&arc1, &arc2));
    }

    #[test]
    fn test_get_or_insert_topic_new() {
        let mut cache = HashMap::new();

        let arc = get_or_insert_topic(&mut cache, "new-topic");
        assert_eq!(&*arc, "new-topic");
        assert!(cache.contains_key("new-topic"));
    }

    #[test]
    fn test_kafka_config_defaults() {
        let config = KafkaConfig::default();
        assert_eq!(config.fetch_max_bytes, 52_428_800); // 50MB
        assert!(!config.enable_auto_commit); // Manual commit
    }
}
