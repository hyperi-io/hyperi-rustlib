// Project:   hyperi-rustlib
// File:      src/transport/kafka/producer.rs
// Purpose:   High-throughput Kafka producer for PB/day workloads
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! High-throughput Kafka producer optimized for PB/day workloads.
//!
//! # Performance Characteristics
//!
//! - **Batch-first design**: Accumulates messages into large batches (256KB default)
//! - **Non-blocking sends**: Fire-and-forget with delivery callbacks
//! - **High parallelism**: Up to 10 in-flight requests per connection
//! - **LZ4 compression**: Best throughput/ratio tradeoff
//! - **1GB producer queue**: Buffers up to 1M messages
//!
//! # Profiles
//!
//! - **high_throughput**: Maximum throughput, at-least-once delivery
//! - **exactly_once**: Idempotent producer with ordering guarantees
//! - **low_latency**: Minimal batching for real-time use cases
//!
//! # Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::transport::kafka::{KafkaProducer, KafkaConfig, ProducerProfile};
//!
//! // High-throughput producer
//! let config = KafkaConfig::production();
//! let producer = KafkaProducer::new(&config, ProducerProfile::HighThroughput)?;
//!
//! // Send messages (fire-and-forget batching)
//! for msg in messages {
//!     producer.send("events", None, msg.as_bytes())?;
//! }
//!
//! // Flush before shutdown
//! producer.flush(Duration::from_secs(30))?;
//! ```

use super::config::KafkaConfig;
use crate::transport::error::{TransportError, TransportResult};
use rdkafka::config::ClientConfig;
use rdkafka::producer::{BaseRecord, Producer, ThreadedProducer};
use rdkafka::util::Timeout;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Producer profile for different use cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProducerProfile {
    /// Maximum throughput, at-least-once delivery.
    ///
    /// - 256KB batches, 100ms linger
    /// - 1GB producer queue
    /// - 10 in-flight requests
    /// - LZ4 compression
    #[default]
    HighThroughput,

    /// Exactly-once semantics with ordering guarantees.
    ///
    /// - Idempotence enabled
    /// - Max 5 in-flight requests
    /// - Infinite retries (bounded by timeout)
    ExactlyOnce,

    /// Minimal latency for real-time use cases.
    ///
    /// - No batching (linger=0)
    /// - acks=1 for faster response
    /// - Smaller buffers
    LowLatency,

    /// Development/testing settings.
    DevTest,
}

impl std::fmt::Display for ProducerProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HighThroughput => write!(f, "high_throughput"),
            Self::ExactlyOnce => write!(f, "exactly_once"),
            Self::LowLatency => write!(f, "low_latency"),
            Self::DevTest => write!(f, "devtest"),
        }
    }
}

/// High-throughput Kafka producer.
///
/// Uses `ThreadedProducer` for background message delivery with:
/// - Automatic batching and compression
/// - Non-blocking sends
/// - Delivery callbacks (optional)
pub struct KafkaProducer {
    producer: ThreadedProducer<ProducerContext>,
    profile: ProducerProfile,
    // Metrics
    messages_sent: AtomicU64,
    bytes_sent: AtomicU64,
    errors: AtomicU64,
}

/// Producer context for delivery callbacks and metrics.
#[derive(Clone)]
pub struct ProducerContext {
    // Could add metrics collection here
}

impl rdkafka::ClientContext for ProducerContext {}

impl rdkafka::producer::ProducerContext for ProducerContext {
    type DeliveryOpaque = ();

    fn delivery(
        &self,
        _result: &rdkafka::producer::DeliveryResult<'_>,
        _opaque: Self::DeliveryOpaque,
    ) {
        // Delivery callback - could update metrics here
    }
}

impl KafkaProducer {
    /// Create a new high-throughput producer.
    ///
    /// # Arguments
    ///
    /// * `config` - Kafka configuration (brokers, security, etc.)
    /// * `profile` - Producer profile (throughput vs latency vs exactly-once)
    ///
    /// # Errors
    ///
    /// Returns error if producer creation fails.
    pub fn new(config: &KafkaConfig, profile: ProducerProfile) -> TransportResult<Self> {
        let mut client_config = ClientConfig::new();

        // Required settings
        client_config.set("bootstrap.servers", config.brokers.join(","));
        client_config.set("client.id", &config.client_id);

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

        // Apply profile defaults
        let profile_settings = match profile {
            ProducerProfile::HighThroughput => super::config::PRODUCER_HIGH_THROUGHPUT,
            ProducerProfile::ExactlyOnce => super::config::PRODUCER_EXACTLY_ONCE,
            ProducerProfile::LowLatency => super::config::PRODUCER_LOW_LATENCY,
            ProducerProfile::DevTest => super::config::PRODUCER_DEVTEST,
        };

        for (key, value) in profile_settings {
            client_config.set(*key, *value);
        }

        // Apply user overrides (highest priority)
        for (key, value) in &config.librdkafka_overrides {
            client_config.set(key, value);
        }

        let context = ProducerContext {};
        let producer: ThreadedProducer<ProducerContext> = client_config
            .create_with_context(context)
            .map_err(|e| TransportError::Connection(format!("Failed to create producer: {e}")))?;

        Ok(Self {
            producer,
            profile,
            messages_sent: AtomicU64::new(0),
            bytes_sent: AtomicU64::new(0),
            errors: AtomicU64::new(0),
        })
    }

    /// Create a high-throughput producer (convenience method).
    pub fn high_throughput(config: &KafkaConfig) -> TransportResult<Self> {
        Self::new(config, ProducerProfile::HighThroughput)
    }

    /// Create an exactly-once producer (convenience method).
    pub fn exactly_once(config: &KafkaConfig) -> TransportResult<Self> {
        Self::new(config, ProducerProfile::ExactlyOnce)
    }

    /// Create a low-latency producer (convenience method).
    pub fn low_latency(config: &KafkaConfig) -> TransportResult<Self> {
        Self::new(config, ProducerProfile::LowLatency)
    }

    /// Send a message to a topic.
    ///
    /// This is a non-blocking operation that queues the message for delivery.
    /// Messages are batched and sent in the background.
    ///
    /// # Arguments
    ///
    /// * `topic` - Target topic name
    /// * `key` - Optional message key (for partitioning)
    /// * `payload` - Message payload bytes
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Message queued successfully
    /// * `Err(TransportError::Backpressure)` - Queue full, retry later
    /// * `Err(TransportError::Send(_))` - Send failed
    pub fn send(&self, topic: &str, key: Option<&[u8]>, payload: &[u8]) -> TransportResult<()> {
        let mut record = BaseRecord::to(topic).payload(payload);
        if let Some(k) = key {
            record = record.key(k);
        }

        match self.producer.send(record) {
            Ok(()) => {
                self.messages_sent.fetch_add(1, Ordering::Relaxed);
                self.bytes_sent
                    .fetch_add(payload.len() as u64, Ordering::Relaxed);
                Ok(())
            }
            Err((err, _)) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
                let err_str = err.to_string();
                if err_str.contains("queue full") || err_str.contains("Local: Queue full") {
                    Err(TransportError::Backpressure)
                } else {
                    Err(TransportError::Send(err_str))
                }
            }
        }
    }

    /// Send a message with a string key.
    ///
    /// Convenience method when key is a string.
    pub fn send_keyed(&self, topic: &str, key: &str, payload: &[u8]) -> TransportResult<()> {
        self.send(topic, Some(key.as_bytes()), payload)
    }

    /// Send a batch of messages.
    ///
    /// More efficient than individual sends as it reduces function call overhead.
    /// All messages go to the same topic.
    ///
    /// # Returns
    ///
    /// Number of messages successfully queued. If less than input length,
    /// the producer queue is full - call `poll()` or `flush()` and retry.
    pub fn send_batch(&self, topic: &str, messages: &[(Option<&[u8]>, &[u8])]) -> usize {
        let mut sent = 0;
        for (key, payload) in messages {
            let mut record = BaseRecord::to(topic).payload(*payload);
            if let Some(k) = key {
                record = record.key(*k);
            }

            if self.producer.send(record).is_ok() {
                self.messages_sent.fetch_add(1, Ordering::Relaxed);
                self.bytes_sent
                    .fetch_add(payload.len() as u64, Ordering::Relaxed);
                sent += 1;
            } else {
                self.errors.fetch_add(1, Ordering::Relaxed);
                break; // Queue full
            }
        }
        sent
    }

    /// Poll the producer for delivery callbacks.
    ///
    /// Call this periodically to process delivery reports and free memory.
    /// For high-throughput, call every 100ms or so.
    pub fn poll(&self, timeout: Duration) {
        self.producer.poll(Timeout::After(timeout));
    }

    /// Flush all queued messages.
    ///
    /// Blocks until all messages are delivered or timeout expires.
    /// Call this before shutdown to ensure no message loss.
    ///
    /// # Returns
    ///
    /// Number of messages still in queue (0 if all delivered).
    #[allow(clippy::cast_sign_loss)]
    pub fn flush(&self, timeout: Duration) -> usize {
        let _ = self.producer.flush(Timeout::After(timeout));
        self.producer.in_flight_count().max(0) as usize
    }

    /// Get the number of messages currently in flight.
    #[allow(clippy::cast_sign_loss)]
    pub fn in_flight_count(&self) -> usize {
        self.producer.in_flight_count().max(0) as usize
    }

    /// Get producer metrics.
    #[allow(clippy::cast_sign_loss)]
    pub fn metrics(&self) -> ProducerMetrics {
        ProducerMetrics {
            messages_sent: self.messages_sent.load(Ordering::Relaxed),
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            in_flight: self.producer.in_flight_count().max(0) as u64,
            profile: self.profile,
        }
    }
}

/// Producer metrics snapshot.
#[derive(Debug, Clone)]
pub struct ProducerMetrics {
    /// Total messages sent (queued).
    pub messages_sent: u64,
    /// Total bytes sent.
    pub bytes_sent: u64,
    /// Total errors encountered.
    pub errors: u64,
    /// Messages currently in flight.
    pub in_flight: u64,
    /// Producer profile in use.
    pub profile: ProducerProfile,
}

impl std::fmt::Debug for KafkaProducer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KafkaProducer")
            .field("profile", &self.profile)
            .field("messages_sent", &self.messages_sent.load(Ordering::Relaxed))
            .field("bytes_sent", &self.bytes_sent.load(Ordering::Relaxed))
            .field("errors", &self.errors.load(Ordering::Relaxed))
            .field("in_flight", &self.producer.in_flight_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_producer_profile_display() {
        assert_eq!(
            ProducerProfile::HighThroughput.to_string(),
            "high_throughput"
        );
        assert_eq!(ProducerProfile::ExactlyOnce.to_string(), "exactly_once");
        assert_eq!(ProducerProfile::LowLatency.to_string(), "low_latency");
        assert_eq!(ProducerProfile::DevTest.to_string(), "devtest");
    }

    #[test]
    fn test_producer_profile_default() {
        assert_eq!(ProducerProfile::default(), ProducerProfile::HighThroughput);
    }
}
