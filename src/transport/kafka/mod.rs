// Project:   hs-rustlib
// File:      src/transport/kafka/mod.rs
// Purpose:   Kafka transport implementation using rdkafka
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! # Kafka Transport
//!
//! Production-grade Kafka transport with at-least-once delivery semantics.
//! Uses rdkafka (librdkafka wrapper) for the underlying Kafka client.
//!
//! ## Features
//!
//! - Consumer group support with offset tracking
//! - SASL/SCRAM and SSL authentication
//! - Manual commit for at-least-once delivery
//! - Arc<str> topic optimization for reduced allocations
//!
//! ## Example
//!
//! ```rust,ignore
//! use hs_rustlib::transport::{KafkaTransport, KafkaConfig, Transport};
//!
//! let config = KafkaConfig {
//!     brokers: vec!["localhost:9092".to_string()],
//!     group: "my-consumer-group".to_string(),
//!     topics: vec!["events".to_string()],
//!     ..Default::default()
//! };
//!
//! let transport = KafkaTransport::new(&config).await?;
//!
//! loop {
//!     let messages = transport.recv(100).await?;
//!     for msg in &messages {
//!         // Process message...
//!     }
//!
//!     // Commit after successful processing
//!     let tokens: Vec<_> = messages.iter().map(|m| m.token.clone()).collect();
//!     transport.commit(&tokens).await?;
//! }
//! ```

mod config;
mod token;

pub use config::KafkaConfig;
pub use token::KafkaToken;

use super::error::{TransportError, TransportResult};
use super::traits::Transport;
use super::types::{Message, PayloadFormat, SendResult};
use async_trait::async_trait;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{CommitMode, Consumer, StreamConsumer};
use rdkafka::message::{BorrowedMessage, Message as KafkaMessage};
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::topic_partition_list::{Offset, TopicPartitionList};
use rdkafka::util::Timeout;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Kafka transport using rdkafka.
///
/// Provides at-least-once delivery with consumer group offset tracking.
pub struct KafkaTransport {
    consumer: StreamConsumer,
    producer: FutureProducer,
    topic_cache: RwLock<HashMap<String, Arc<str>>>,
    closed: AtomicBool,
}

impl KafkaTransport {
    /// Create a new Kafka transport.
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

        // Extra config (for advanced librdkafka options)
        for (key, value) in &config.extra_config {
            client_config.set(key, value);
        }

        // Create consumer
        let consumer: StreamConsumer = client_config
            .create()
            .map_err(|e| TransportError::Connection(format!("Failed to create consumer: {e}")))?;

        // Subscribe to topics
        if !config.topics.is_empty() {
            let topics: Vec<&str> = config.topics.iter().map(String::as_str).collect();
            consumer
                .subscribe(&topics)
                .map_err(|e| TransportError::Connection(format!("Failed to subscribe: {e}")))?;
        }

        // Create producer (for send operations)
        let producer: FutureProducer = client_config
            .create()
            .map_err(|e| TransportError::Connection(format!("Failed to create producer: {e}")))?;

        Ok(Self {
            consumer,
            producer,
            topic_cache: RwLock::new(HashMap::new()),
            closed: AtomicBool::new(false),
        })
    }

    /// Get or create cached Arc<str> for topic name.
    async fn get_topic_arc(&self, topic: &str) -> Arc<str> {
        // Fast path: read lock
        {
            let cache = self.topic_cache.read().await;
            if let Some(arc) = cache.get(topic) {
                return arc.clone();
            }
        }

        // Slow path: write lock
        let mut cache = self.topic_cache.write().await;
        cache
            .entry(topic.to_string())
            .or_insert_with(|| Arc::from(topic))
            .clone()
    }

    /// Convert rdkafka message to our Message type.
    async fn convert_message(&self, msg: &BorrowedMessage<'_>) -> Message<KafkaToken> {
        let topic = self.get_topic_arc(msg.topic()).await;
        let payload = msg.payload().map(|p| p.to_vec()).unwrap_or_default();
        let format = PayloadFormat::detect(&payload);

        Message {
            key: Some(topic.clone()),
            payload,
            token: KafkaToken::new(topic, msg.partition(), msg.offset()),
            timestamp_ms: msg.timestamp().to_millis(),
            format,
        }
    }

}

#[async_trait]
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
                // Check if it's a retriable error
                let err_str = err.to_string();
                if err_str.contains("queue full") || err_str.contains("Local: Queue full") {
                    SendResult::Backpressured
                } else {
                    SendResult::Fatal(TransportError::Send(err_str))
                }
            }
        }
    }

    async fn recv(&self, max: usize) -> TransportResult<Vec<Message<Self::Token>>> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(TransportError::Closed);
        }

        let mut messages = Vec::with_capacity(max.min(1000));
        let timeout = Duration::from_millis(100);

        for _ in 0..max {
            match tokio::time::timeout(timeout, self.consumer.recv()).await {
                Ok(Ok(msg)) => {
                    messages.push(self.convert_message(&msg).await);
                }
                Ok(Err(e)) => {
                    // Kafka error
                    if messages.is_empty() {
                        return Err(TransportError::Recv(e.to_string()));
                    }
                    // Return what we have
                    break;
                }
                Err(_) => {
                    // Timeout - return what we have
                    break;
                }
            }
        }

        Ok(messages)
    }

    async fn commit(&self, tokens: &[Self::Token]) -> TransportResult<()> {
        if tokens.is_empty() {
            return Ok(());
        }

        // Build topic-partition-offset list
        // For each partition, commit the highest offset + 1 (next to be read)
        let mut tpl = TopicPartitionList::new();
        let mut partition_offsets: HashMap<(Arc<str>, i32), i64> = HashMap::new();

        for token in tokens {
            let key = (token.topic.clone(), token.partition);
            let current = partition_offsets.get(&key).copied().unwrap_or(i64::MIN);
            if token.offset > current {
                partition_offsets.insert(key, token.offset);
            }
        }

        for ((topic, partition), offset) in partition_offsets {
            tpl.add_partition_offset(&topic, partition, Offset::Offset(offset + 1))
                .map_err(|e| TransportError::Commit(format!("Failed to build TPL: {e}")))?;
        }

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
