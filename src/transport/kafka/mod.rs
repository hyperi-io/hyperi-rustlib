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
pub mod topic_resolver;

pub use admin::{KafkaAdmin, TopicInfo};
#[allow(deprecated)]
pub use config::{
    DEVTEST_PROFILE, HIGH_THROUGHPUT_CONSUMER_DEFAULTS, KafkaConfig, KafkaProfile,
    LOW_LATENCY_CONSUMER_DEFAULTS, PRODUCER_DEFAULTS, PRODUCER_DEVTEST, PRODUCER_EXACTLY_ONCE,
    PRODUCER_HIGH_THROUGHPUT, PRODUCER_LOW_LATENCY, PRODUCTION_PROFILE, SuppressionRule,
    merge_with_overrides,
};
pub use metrics::{
    BrokerMetrics, KafkaMetrics, StatsContext, healthy_broker_count, total_consumer_lag,
};
pub use producer::{KafkaProducer, ProducerMetrics, ProducerProfile};
pub use token::KafkaToken;
pub use topic_resolver::{TopicRefreshHandle, TopicResolver};

use super::error::{TransportError, TransportResult};
use super::traits::{TransportBase, TransportReceiver, TransportSender};
use super::types::{Message, PayloadFormat, SendResult};
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, CommitMode, Consumer};
use rdkafka::message::Message as KafkaMessage;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::topic_partition_list::{Offset, TopicPartitionList};
use rdkafka::util::Timeout;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    consumer: BaseConsumer<StatsContext>,
    producer: FutureProducer<StatsContext>,
    /// Pre-populated topic cache - populated on construction, read-only after.
    /// Key optimization: no locks in the hot path.
    topic_cache: HashMap<String, Arc<str>>,
    closed: AtomicBool,
    /// Shared healthy flag — read by health registry closure, written by close().
    healthy: Arc<AtomicBool>,
    /// Topics we're subscribed to (for cache warming and Debug).
    /// Behind RwLock so recv() can update after topic refresh re-subscribe.
    subscribed_topics: parking_lot::RwLock<Vec<String>>,
    /// Shutdown token — cancelled on close() to stop background tasks.
    shutdown_token: tokio_util::sync::CancellationToken,
    /// Periodic topic refresh handle (auto-discovery mode only).
    /// Checked on each recv() call to detect new/removed topics.
    /// Uses parking_lot::Mutex (no poisoning, faster uncontended) since this
    /// is on the recv() hot path.
    topic_refresh: Option<parking_lot::Mutex<TopicRefreshHandle>>,
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
            client_config.set("sasl.password", password.expose());
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

        // Ensure statistics callbacks fire (all profiles already set this, but
        // guarantee it as a fallback for manual configs).
        if client_config.get("statistics.interval.ms").is_none() {
            client_config.set("statistics.interval.ms", "5000");
        }

        // StatsContext receives librdkafka statistics callbacks and auto-emits
        // rdkafka_* Prometheus metrics when a recorder is installed.
        // Consumer and producer each get their own context instance.

        // Create consumer with StatsContext for metrics collection
        let consumer: BaseConsumer<StatsContext> = client_config
            .create_with_context(StatsContext::new())
            .map_err(|e| TransportError::Connection(format!("Failed to create consumer: {e}")))?;

        // Resolve effective topics:
        // - Explicit list → subscribe to those
        // - Empty + auto_discover → auto-discover from broker
        // - Empty + !auto_discover → no subscription (producer-only)
        let (effective_topics, topic_refresh, shutdown_token) =
            if config.topics.is_empty() && config.auto_discover {
                tracing::info!("Topics empty — auto-discovering from broker");
                let resolver = topic_resolver::TopicResolver::new(config)?;
                let discovered = resolver.resolve()?;
                if discovered.is_empty() {
                    return Err(TransportError::Config(
                        "Auto-discovery found no matching topics".into(),
                    ));
                }

                let token = tokio_util::sync::CancellationToken::new();
                let refresh = if config.topic_refresh_secs > 0 {
                    let refresh_resolver = topic_resolver::TopicResolver::new(config)?;
                    let handle = refresh_resolver.start_refresh_loop(
                        Duration::from_secs(config.topic_refresh_secs),
                        token.clone(),
                    );
                    tracing::info!(
                        interval_secs = config.topic_refresh_secs,
                        "Started periodic topic refresh"
                    );
                    Some(parking_lot::Mutex::new(handle))
                } else {
                    None
                };

                (discovered, refresh, token)
            } else {
                (
                    config.topics.clone(),
                    None,
                    tokio_util::sync::CancellationToken::new(),
                )
            };

        // Subscribe to topics
        let subscribed_topics = effective_topics;
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

        // Create producer with StatsContext for metrics collection
        let producer: FutureProducer<StatsContext> = client_config
            .create_with_context(StatsContext::new())
            .map_err(|e| TransportError::Connection(format!("Failed to create producer: {e}")))?;

        let healthy = Arc::new(AtomicBool::new(true));

        #[cfg(feature = "health")]
        {
            let h = Arc::clone(&healthy);
            crate::health::HealthRegistry::register("transport:kafka", move || {
                if h.load(Ordering::Relaxed) {
                    crate::health::HealthStatus::Healthy
                } else {
                    crate::health::HealthStatus::Unhealthy
                }
            });
        }

        Ok(Self {
            consumer,
            producer,
            topic_cache,
            closed: AtomicBool::new(false),
            healthy,
            subscribed_topics: parking_lot::RwLock::new(subscribed_topics),
            shutdown_token,
            topic_refresh,
        })
    }

    /// Get the consumer's metrics snapshot.
    ///
    /// Returns statistics collected via librdkafka callbacks. Includes
    /// broker RTT, consumer lag, rebalance count, etc.
    #[must_use]
    pub fn stats(&self) -> KafkaMetrics {
        self.consumer.context().get_metrics()
    }
}

impl TransportBase for KafkaTransport {
    async fn close(&self) -> TransportResult<()> {
        self.closed.store(true, Ordering::Relaxed);
        self.healthy.store(false, Ordering::Relaxed);
        self.shutdown_token.cancel();
        // rdkafka handles cleanup on drop
        Ok(())
    }

    fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    fn name(&self) -> &'static str {
        "kafka"
    }
}

impl TransportSender for KafkaTransport {
    async fn send(&self, key: &str, payload: &[u8]) -> SendResult {
        if self.closed.load(Ordering::Relaxed) {
            return SendResult::Fatal(TransportError::Closed);
        }

        let record: FutureRecord<'_, str, [u8]> = FutureRecord::to(key).payload(payload);

        // Inject W3C traceparent into Kafka message headers for distributed tracing
        #[cfg(feature = "otel")]
        let record = if let Some(tp) = super::propagation::current_traceparent() {
            let headers = rdkafka::message::OwnedHeaders::new().insert(rdkafka::message::Header {
                key: super::propagation::TRACEPARENT_HEADER,
                value: Some(tp.as_str()),
            });
            record.headers(headers)
        } else {
            record
        };

        #[cfg(feature = "metrics")]
        let start = std::time::Instant::now();

        let result = match self
            .producer
            .send(record, Timeout::After(Duration::from_secs(5)))
            .await
        {
            Ok(_) => {
                #[cfg(feature = "metrics")]
                ::metrics::counter!("dfe_transport_sent_total", "transport" => "kafka")
                    .increment(1);
                SendResult::Ok
            }
            Err((err, _)) => {
                let err_str = err.to_string();
                if err_str.contains("queue full") || err_str.contains("Local: Queue full") {
                    #[cfg(feature = "metrics")]
                    ::metrics::counter!(
                        "dfe_transport_backpressured_total",
                        "transport" => "kafka"
                    )
                    .increment(1);
                    SendResult::Backpressured
                } else {
                    #[cfg(feature = "metrics")]
                    ::metrics::counter!(
                        "dfe_transport_send_errors_total",
                        "transport" => "kafka"
                    )
                    .increment(1);
                    SendResult::Fatal(TransportError::Send(err_str))
                }
            }
        };

        #[cfg(feature = "metrics")]
        ::metrics::histogram!(
            "dfe_transport_send_duration_seconds",
            "transport" => "kafka"
        )
        .record(start.elapsed().as_secs_f64());

        result
    }
}

impl TransportReceiver for KafkaTransport {
    type Token = KafkaToken;

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

        // Check for topic changes from the background refresh loop
        if let Some(ref refresh) = self.topic_refresh
            && let Some(new_topics) = refresh.lock().check_changed()
        {
            let topics: Vec<&str> = new_topics.iter().map(String::as_str).collect();
            match self.consumer.subscribe(&topics) {
                Ok(()) => {
                    tracing::info!(?new_topics, "Re-subscribed after topic refresh");
                    *self.subscribed_topics.write() = new_topics;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to re-subscribe after topic refresh");
                }
            }
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
                    // Extract W3C traceparent from Kafka headers (first message only,
                    // to associate the batch span with the upstream trace)
                    #[cfg(feature = "otel")]
                    if let Some(headers) = msg.headers() {
                        use rdkafka::message::Headers;
                        for idx in 0..headers.count() {
                            if let Some(Ok(header)) = headers.try_get_as::<[u8]>(idx)
                                && header.key == super::propagation::TRACEPARENT_HEADER
                            {
                                if let Some(value) = header.value
                                    && let Ok(tp) = std::str::from_utf8(value)
                                    && super::propagation::is_valid_traceparent(tp)
                                {
                                    tracing::Span::current().record("traceparent", tp);
                                }
                                break;
                            }
                        }
                    }

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
            .field("subscribed_topics", &*self.subscribed_topics.read())
            .field("closed", &self.closed.load(Ordering::Relaxed))
            .field("healthy", &self.healthy.load(Ordering::Relaxed))
            .field("topic_refresh_active", &self.topic_refresh.is_some())
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

    #[tokio::test]
    async fn test_topic_refresh_check_changed_detects_updates() {
        // Simulate the watch channel that TopicRefreshHandle uses internally
        let (tx, rx) = tokio::sync::watch::channel(vec!["events_load".to_string()]);

        let mut handle = topic_resolver::TopicRefreshHandle::new_for_test(rx);

        // Initially no change (first check sees initial value as "no change")
        assert!(handle.check_changed().is_none());

        // Send new topics
        tx.send(vec!["events_load".to_string(), "logs_load".to_string()])
            .unwrap();

        // Now check_changed should return the new list
        let changed = handle.check_changed();
        assert!(changed.is_some());
        let topics = changed.unwrap();
        assert_eq!(topics.len(), 2);
        assert!(topics.contains(&"logs_load".to_string()));

        // Second check with no new changes should return None
        assert!(handle.check_changed().is_none());
    }

    #[test]
    fn test_subscribed_topics_rwlock_update() {
        // Verify the RwLock pattern used in recv() for subscribed_topics
        let topics = parking_lot::RwLock::new(vec!["events_load".to_string()]);

        // Read path (Debug, metrics)
        assert_eq!(topics.read().len(), 1);

        // Write path (after topic refresh re-subscribe)
        *topics.write() = vec!["events_load".to_string(), "logs_load".to_string()];
        assert_eq!(topics.read().len(), 2);
        assert_eq!(topics.read()[1], "logs_load");
    }
}
