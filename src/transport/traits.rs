// Project:   hyperi-rustlib
// File:      src/transport/traits.rs
// Purpose:   Transport trait definitions (sender + receiver split)
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use super::error::TransportResult;
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
}

/// Combined transport — implements both send and receive.
///
/// Convenience trait for transports that support bidirectional communication.
/// Most concrete implementations (Kafka, gRPC, Memory, Redis, File, Pipe)
/// implement this. Automatically implemented via blanket impl.
pub trait Transport: TransportSender + TransportReceiver {}

/// Blanket impl: anything that implements both traits is a Transport.
impl<T: TransportSender + TransportReceiver> Transport for T {}
