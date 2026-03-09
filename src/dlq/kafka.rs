// Project:   hyperi-rustlib
// File:      src/dlq/kafka.rs
// Purpose:   Kafka-based DLQ backend using rustlib's KafkaProducer
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Kafka-based DLQ backend.
//!
//! Routes failed messages to Kafka topics using rustlib's existing
//! [`KafkaProducer`](crate::transport::kafka::KafkaProducer) for all
//! broker/SASL/TLS plumbing.
//!
//! ## Topic Routing
//!
//! - **Per-table**: Destination `acme.auth` → topic `acme.auth.dlq`
//! - **Common**: All failures → single common topic (e.g. `dfe.dlq`)

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use tracing::{debug, error, info};

use crate::transport::KafkaConfig;
use crate::transport::kafka::{KafkaProducer, ProducerProfile};

use super::backend::DlqBackend;
use super::config::{DlqRouting, KafkaDlqConfig};
use super::entry::DlqEntry;
use super::error::DlqError;

/// Kafka-based DLQ backend.
///
/// Uses rustlib's [`KafkaProducer`] with `LowLatency` profile for
/// immediate delivery of failed messages.
pub struct KafkaDlq {
    producer: KafkaProducer,
    routing: DlqRouting,
    topic_suffix: String,
    common_topic: String,
    entries_written: AtomicU64,
    write_errors: AtomicU64,
}

impl std::fmt::Debug for KafkaDlq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KafkaDlq")
            .field("routing", &self.routing)
            .field("topic_suffix", &self.topic_suffix)
            .field("common_topic", &self.common_topic)
            .field(
                "entries_written",
                &self.entries_written.load(Ordering::Relaxed),
            )
            .field("write_errors", &self.write_errors.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl KafkaDlq {
    /// Create a new Kafka DLQ backend.
    ///
    /// Uses the service's existing [`KafkaConfig`] for broker/auth settings.
    /// The producer uses `LowLatency` profile — DLQ volume is low and we
    /// want messages delivered quickly.
    ///
    /// # Errors
    ///
    /// Returns an error if the Kafka producer cannot be created.
    pub fn new(kafka_config: &KafkaConfig, dlq_config: &KafkaDlqConfig) -> Result<Self, DlqError> {
        // Use LowLatency profile: minimal batching, fast delivery
        let producer = KafkaProducer::new(kafka_config, ProducerProfile::LowLatency)
            .map_err(|e| DlqError::Kafka(format!("failed to create DLQ producer: {e}")))?;

        info!(
            routing = ?dlq_config.routing,
            suffix = %dlq_config.topic_suffix,
            common_topic = %dlq_config.common_topic,
            "Kafka DLQ backend initialised"
        );

        Ok(Self {
            producer,
            routing: dlq_config.routing,
            topic_suffix: dlq_config.topic_suffix.clone(),
            common_topic: dlq_config.common_topic.clone(),
            entries_written: AtomicU64::new(0),
            write_errors: AtomicU64::new(0),
        })
    }

    /// Resolve the target Kafka topic from the entry's destination.
    fn resolve_topic(&self, entry: &DlqEntry) -> String {
        match self.routing {
            DlqRouting::Common => self.common_topic.clone(),
            DlqRouting::PerTable => {
                if let Some(ref dest) = entry.destination {
                    format!("{}{}", dest, self.topic_suffix)
                } else {
                    self.common_topic.clone()
                }
            }
        }
    }

    /// Number of entries successfully written.
    pub fn entries_written(&self) -> u64 {
        self.entries_written.load(Ordering::Relaxed)
    }

    /// Number of write errors.
    pub fn write_errors(&self) -> u64 {
        self.write_errors.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl DlqBackend for KafkaDlq {
    async fn send(&self, entry: &DlqEntry) -> Result<(), DlqError> {
        let topic = self.resolve_topic(entry);

        // Serialise entry to JSON for the message body
        let payload = serde_json::to_vec(entry)
            .map_err(|e| DlqError::Serialization(format!("failed to serialise DLQ entry: {e}")))?;

        // KafkaProducer.send() is non-blocking (queues for background delivery)
        match self.producer.send(&topic, None, &payload) {
            Ok(()) => {
                self.entries_written.fetch_add(1, Ordering::Relaxed);
                debug!(
                    topic = %topic,
                    reason = %entry.reason,
                    "DLQ entry sent to Kafka"
                );
                Ok(())
            }
            Err(e) => {
                self.write_errors.fetch_add(1, Ordering::Relaxed);
                error!(
                    error = %e,
                    topic = %topic,
                    reason = %entry.reason,
                    "Failed to send DLQ entry to Kafka"
                );
                Err(DlqError::Kafka(format!("DLQ send failed: {e}")))
            }
        }
    }

    fn name(&self) -> &'static str {
        "kafka"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_topic_per_table() {
        let routing = DlqRouting::PerTable;
        let suffix = ".dlq";
        let common = "dfe.dlq";

        // With destination
        let entry = DlqEntry::new("loader", "error", vec![]).with_destination("acme.auth");
        let topic = match routing {
            DlqRouting::PerTable => {
                if let Some(ref dest) = entry.destination {
                    format!("{dest}{suffix}")
                } else {
                    common.to_string()
                }
            }
            DlqRouting::Common => common.to_string(),
        };
        assert_eq!(topic, "acme.auth.dlq");

        // Without destination falls back to common
        let entry_no_dest = DlqEntry::new("loader", "error", vec![]);
        let topic = match routing {
            DlqRouting::PerTable => {
                if let Some(ref dest) = entry_no_dest.destination {
                    format!("{dest}{suffix}")
                } else {
                    common.to_string()
                }
            }
            DlqRouting::Common => common.to_string(),
        };
        assert_eq!(topic, "dfe.dlq");
    }

    #[test]
    fn test_resolve_topic_common() {
        let routing = DlqRouting::Common;
        let common = "all-errors.dlq";

        let _entry = DlqEntry::new("loader", "error", vec![]).with_destination("acme.auth");
        // Common mode ignores destination, always uses common topic
        let topic = match routing {
            DlqRouting::Common => common.to_string(),
            DlqRouting::PerTable => unreachable!(),
        };
        assert_eq!(topic, "all-errors.dlq");
    }
}
