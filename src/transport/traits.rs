// Project:   hyperi-rustlib
// File:      src/transport/traits.rs
// Purpose:   Transport trait definitions (sender + receiver split)
// Language:  Rust
//
// License:   FSL-1.1-ALv2
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
    fn send(&self, key: &str, payload: &[u8]) -> impl Future<Output = SendResult> + Send;
}

/// Receive-side transport — generic over commit token type.
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
    /// Returns empty vec if no messages are available.
    ///
    /// **Filter behaviour:** if the transport has inbound filters configured,
    /// `recv()` removes messages that match `action: drop` filters and stages
    /// messages matching `action: dlq` filters into an internal queue. Use
    /// [`take_filtered_dlq_entries`](Self::take_filtered_dlq_entries) after
    /// each `recv()` call to retrieve the staged DLQ entries and route them
    /// via your DLQ handle.
    fn recv(
        &self,
        max: usize,
    ) -> impl Future<Output = TransportResult<Vec<Message<Self::Token>>>> + Send;

    /// Commit/acknowledge processed messages.
    ///
    /// - Kafka: commits consumer offsets
    /// - gRPC: no-op (no persistence)
    /// - Redis: XACK
    /// - File: advances read position
    /// - Memory: advances internal sequence
    fn commit(&self, tokens: &[Self::Token]) -> impl Future<Output = TransportResult<()>> + Send;

    /// Drain DLQ entries staged by inbound filtering.
    ///
    /// When a transport's inbound filters classify messages as `action: dlq`,
    /// the messages are removed from the `recv()` result and staged in an
    /// internal queue. Call this method after each `recv()` to drain the
    /// staged entries and route them to your DLQ.
    ///
    /// Default implementation returns an empty vec — transports without
    /// filter support don't need to override this.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let messages = transport.recv(100).await?;
    /// for entry in transport.take_filtered_dlq_entries() {
    ///     dlq.send(DlqEntry::new("filter", entry.reason, entry.payload)).await?;
    /// }
    /// // Process passing messages...
    /// ```
    fn take_filtered_dlq_entries(&self) -> Vec<FilteredDlqEntry> {
        Vec::new()
    }
}

/// Combined transport — implements both send and receive.
///
/// Convenience trait for transports that support bidirectional communication.
/// Most concrete implementations (Kafka, gRPC, Memory, Redis, File, Pipe)
/// implement this. Automatically implemented via blanket impl.
pub trait Transport: TransportSender + TransportReceiver {}

/// Blanket impl: anything that implements both traits is a Transport.
impl<T: TransportSender + TransportReceiver> Transport for T {}
