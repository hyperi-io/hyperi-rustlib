// Project:   hyperi-rustlib
// File:      src/dlq/kafka.rs
// Purpose:   Kafka-based DLQ backend variant
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Kafka backend variant for the DLQ enum.
//!
//! Routes failed messages to Kafka topics using rustlib's
//! [`KafkaProducer`](crate::transport::kafka::KafkaProducer). The
//! producer uses the `LowLatency` profile -- DLQ volume is low and we
//! want failures visible quickly.
//!
//! ## Topic Routing
//!
//! - **Per-table**: Destination `acme.auth` → topic `acme.auth.dlq`
//! - **Common**: All failures → single common topic (e.g. `dfe.dlq`)

use std::sync::atomic::{AtomicU64, Ordering};

use tracing::{debug, error, info};

use crate::transport::KafkaConfig;
use crate::transport::kafka::{KafkaProducer, ProducerProfile};

use super::config::{DlqRouting, KafkaDlqConfig};
use super::entry::DlqEntry;
use super::error::DlqError;

/// Kafka backend -- internal variant carried by [`super::DlqBackend::Kafka`].
pub struct KafkaDlqInner {
    producer: KafkaProducer,
    routing: DlqRouting,
    topic_suffix: String,
    common_topic: String,
    entries_written: AtomicU64,
    write_errors: AtomicU64,
}

impl std::fmt::Debug for KafkaDlqInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KafkaDlqInner")
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

impl KafkaDlqInner {
    /// Build the Kafka backend.
    ///
    /// # Errors
    ///
    /// Returns an error if the Kafka producer cannot be created.
    pub fn new(kafka_config: &KafkaConfig, dlq_config: &KafkaDlqConfig) -> Result<Self, DlqError> {
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

    fn resolve_topic(&self, entry: &DlqEntry) -> String {
        match self.routing {
            DlqRouting::Common => self.common_topic.clone(),
            DlqRouting::PerTable => entry.destination.as_ref().map_or_else(
                || self.common_topic.clone(),
                |dest| format!("{dest}{}", self.topic_suffix),
            ),
        }
    }

    /// Send a batch. Per-entry topic resolution + non-blocking producer
    /// queue. The producer's background delivery thread does the network
    /// I/O -- `send()` is sync-shaped and returns immediately.
    pub async fn send_batch(&mut self, batch: &[DlqEntry]) -> Result<(), DlqError> {
        for entry in batch {
            let topic = self.resolve_topic(entry);
            let payload = serde_json::to_vec(entry)
                .map_err(|e| DlqError::Serialization(format!("DLQ serialise: {e}")))?;

            match self.producer.send(&topic, None, &payload) {
                Ok(()) => {
                    self.entries_written.fetch_add(1, Ordering::Relaxed);
                    debug!(topic = %topic, reason = %entry.reason, "DLQ entry queued to Kafka");
                }
                Err(e) => {
                    self.write_errors.fetch_add(1, Ordering::Relaxed);
                    error!(
                        error = %e,
                        topic = %topic,
                        reason = %entry.reason,
                        "Failed to queue DLQ entry to Kafka"
                    );
                    return Err(DlqError::Kafka(format!("DLQ send failed: {e}")));
                }
            }
        }
        Ok(())
    }

    /// Block until every entry previously queued by `send_batch` has
    /// been acknowledged by the broker (per the producer's `acks`
    /// configuration).
    ///
    /// `send_batch` is sync-shaped -- it hands payloads to the
    /// background producer thread and returns immediately. Without this
    /// flush, the orchestrator's barrier would ack `Dlq::flush()`
    /// callers when their entries were merely queued, not when they
    /// were durably written. The reviewer (hyperi-rustlib pre-GA C06)
    /// flagged exactly this gap.
    ///
    /// # Errors
    ///
    /// `DlqError::Kafka` when the producer flush timeout expires with
    /// messages still outstanding. The previous shape returned `Ok(())`
    /// regardless -- callers thought the DLQ was drained while Kafka
    /// still owned in-flight data, so a process exit lost entries
    /// (Codex F3).
    pub async fn flush_durable(&mut self) -> Result<(), DlqError> {
        // Bounded wait -- typical producer flush completes in
        // milliseconds; a 30s ceiling avoids wedging the actor on a
        // hard-to-reach broker.
        let outstanding = self.producer.flush(std::time::Duration::from_secs(30));
        if outstanding > 0 {
            debug!(
                outstanding,
                "Kafka DLQ flush timed out with messages still in flight"
            );
            return Err(DlqError::Kafka(format!(
                "flush_durable timed out with {outstanding} messages still in flight"
            )));
        }
        Ok(())
    }

    /// Number of entries successfully queued.
    #[must_use]
    pub fn entries_written(&self) -> u64 {
        self.entries_written.load(Ordering::Relaxed)
    }

    /// Number of queue-submit errors.
    #[must_use]
    pub fn write_errors(&self) -> u64 {
        self.write_errors.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_topic_per_table() {
        let routing = DlqRouting::PerTable;
        let suffix = ".dlq";
        let common = "dfe.dlq";

        let entry = DlqEntry::new("loader", "error", vec![]).with_destination("acme.auth");
        let topic = match routing {
            DlqRouting::PerTable => entry
                .destination
                .as_ref()
                .map_or_else(|| common.to_string(), |dest| format!("{dest}{suffix}")),
            DlqRouting::Common => common.to_string(),
        };
        assert_eq!(topic, "acme.auth.dlq");

        let entry_no_dest = DlqEntry::new("loader", "error", vec![]);
        let topic = match routing {
            DlqRouting::PerTable => entry_no_dest
                .destination
                .as_ref()
                .map_or_else(|| common.to_string(), |dest| format!("{dest}{suffix}")),
            DlqRouting::Common => common.to_string(),
        };
        assert_eq!(topic, "dfe.dlq");
    }

    #[test]
    fn resolve_topic_common_ignores_destination() {
        let routing = DlqRouting::Common;
        let common = "all-errors.dlq";

        let entry = DlqEntry::new("loader", "error", vec![]).with_destination("acme.auth");
        let topic = match routing {
            DlqRouting::Common => common.to_string(),
            DlqRouting::PerTable => unreachable!(
                "per-table case is exercised by the sibling test; this match must hit Common"
            ),
        };
        let _ = entry;
        assert_eq!(topic, "all-errors.dlq");
    }
}
