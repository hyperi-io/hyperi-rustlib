// Project:   hyperi-rustlib
// File:      src/transport/traits.rs
// Purpose:   Transport trait definitions (sender + receiver split)
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

use super::error::TransportResult;
use super::filter::FilteredDlqEntry;
use super::types::{Message, SendResult};
use std::fmt::{Debug, Display};
use std::future::Future;

/// Transport-specific token for commit/acknowledgment.
///
/// Each transport implementation provides its own token type that
/// captures the information needed to acknowledge message processing.
pub trait CommitToken: Clone + Send + Sync + Debug + Display + 'static {
    /// Get a string representation for logging/debugging.
    fn as_str(&self) -> String {
        format!("{self}")
    }
}

/// Result of a [`TransportReceiver::recv`] call.
///
/// Carries the messages that passed inbound filtering AND any entries those
/// filters routed to DLQ, so a caller cannot accidentally lose dead-letters by
/// forgetting a separate drain step (the previous two-call
/// `recv()` + `take_filtered_dlq_entries()` contract). The caller routes
/// `dlq_entries` onward via its own DLQ handle.
#[derive(Debug)]
pub struct RecvBatch<T: CommitToken> {
    /// Messages that passed all inbound filters (or had no filter match).
    pub messages: Vec<Message<T>>,
    /// Entries matched by `action: dlq` inbound filters. Caller routes to DLQ.
    pub dlq_entries: Vec<FilteredDlqEntry>,
}

impl<T: CommitToken> RecvBatch<T> {
    /// An empty batch (no messages, no DLQ entries).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            messages: Vec::new(),
            dlq_entries: Vec::new(),
        }
    }

    /// A batch of messages with no DLQ entries (e.g. filters disabled).
    #[must_use]
    pub fn from_messages(messages: Vec<Message<T>>) -> Self {
        Self {
            messages,
            dlq_entries: Vec::new(),
        }
    }

    /// Whether the batch has no messages (DLQ entries may still be present).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Number of passing messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }
}

/// Common transport operations shared by senders and receivers.
///
/// Every transport implementation provides these lifecycle and
/// introspection methods regardless of direction.
pub trait TransportBase: Send + Sync {
    /// Shutdown the transport gracefully.
    fn close(&self) -> impl Future<Output = TransportResult<()>> + Send;

    /// Check if the transport is healthy and connected.
    fn is_healthy(&self) -> bool;

    /// Get transport name for logging/metrics.
    fn name(&self) -> &'static str;
}

/// Send-side transport.
///
/// Extends `TransportBase` with send capability. The factory returns
/// `AnySender` (enum dispatch) for runtime transport selection.
///
/// All implementations auto-emit `dfe_transport_*` Prometheus metrics
/// when a `MetricsManager` recorder is installed.
pub trait TransportSender: TransportBase {
    /// Send raw bytes to a key/destination.
    ///
    /// The `key` semantics depend on the transport:
    /// - Kafka: topic name
    /// - gRPC: metadata routing key
    /// - HTTP: URL path suffix or ignored
    /// - File: filename suffix or ignored
    /// - Redis: stream name
    /// - Pipe: ignored (single stdout)
    fn send(&self, key: &str, payload: bytes::Bytes) -> impl Future<Output = SendResult> + Send;
}

/// Receive-side transport -- generic over commit token type.
///
/// Extends `TransportBase` with receive and commit capability.
/// Input stages (receiver, fetcher) use concrete implementations
/// directly for type-safe token handling.
pub trait TransportReceiver: TransportBase {
    /// The token type for this transport.
    type Token: CommitToken;

    /// Receive up to `max` messages.
    ///
    /// Returns immediately with available messages (may be fewer than `max`).
    /// Returns an empty batch if no messages are available.
    ///
    /// **Filter behaviour:** if the transport has inbound filters configured,
    /// `recv()` removes messages matching `action: drop` filters and returns
    /// messages matching `action: dlq` filters in [`RecvBatch`]`.dlq_entries`
    /// alongside the passing [`RecvBatch`]`.messages`. Route the DLQ entries via
    /// your own DLQ handle -- they cannot be silently lost.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let batch = transport.recv(100).await?.messages;
    /// for entry in batch.dlq_entries {
    ///     dlq.send(DlqEntry::new("filter", entry.reason, entry.payload)).await?;
    /// }
    /// for msg in batch.messages { /* process */ }
    /// ```
    fn recv(
        &self,
        max: usize,
    ) -> impl Future<Output = TransportResult<RecvBatch<Self::Token>>> + Send;

    /// Commit/acknowledge processed messages.
    ///
    /// - Kafka: commits consumer offsets
    /// - gRPC: no-op (no persistence)
    /// - Redis: XACK
    /// - File: advances read position
    /// - Memory: advances internal sequence
    fn commit(&self, tokens: &[Self::Token]) -> impl Future<Output = TransportResult<()>> + Send;
}

/// Combined transport -- implements both send and receive.
///
/// Convenience trait for transports that support bidirectional communication.
/// Most concrete implementations (Kafka, gRPC, Memory, Redis, File, Pipe)
/// implement this. Automatically implemented via blanket impl.
pub trait Transport: TransportSender + TransportReceiver {}

/// Blanket impl: anything that implements both traits is a Transport.
impl<T: TransportSender + TransportReceiver> Transport for T {}

/// Load a transport config from the cascade under a fixed key.
///
/// Consolidates the byte-identical `from_cascade()` bodies that each transport
/// config previously repeated (try the cascade under a key, register the
/// section, fall back to `Default`). Implementors only name their key; the
/// loading logic lives here once. Without the `config` feature the default
/// method just returns `Default::default()`.
pub trait FromCascade: Default + serde::Serialize + serde::de::DeserializeOwned + 'static {
    /// Load `Self` from the config cascade under `key`, registering the section
    /// in the global registry; falls back to `Default` if the cascade is
    /// unavailable or the key is absent/invalid.
    #[must_use]
    fn from_cascade_key(key: &str) -> Self {
        #[cfg(feature = "config")]
        {
            if let Some(cfg) = crate::config::try_get()
                && let Ok(value) = cfg.unmarshal_key_registered::<Self>(key)
            {
                return value;
            }
        }
        // Without `config`, or on any cascade miss, use defaults.
        #[cfg(not(feature = "config"))]
        let _ = key;
        Self::default()
    }
}
