// Project:   hyperi-rustlib
// File:      src/transport/traits.rs
// Purpose:   Transport trait definitions
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use super::error::TransportResult;
use super::types::{Message, SendResult};
use async_trait::async_trait;
use std::fmt::{Debug, Display};

/// Transport-specific token for commit/acknowledgment.
///
/// Each transport implementation provides its own token type that
/// captures the information needed to acknowledge message processing.
///
/// Implementors must be `Clone`, `Send`, `Sync`, and `Debug`.
pub trait CommitToken: Clone + Send + Sync + Debug + Display + 'static {
    /// Get a string representation for logging/debugging.
    fn as_str(&self) -> String {
        format!("{self}")
    }
}

/// Transport-agnostic message delivery.
///
/// All implementations deliver raw bytes (JSON or MsgPack) without
/// any envelope or framing. Transport metadata is captured in tokens.
///
/// # Type Parameter
///
/// The `Token` associated type allows each transport to have its own
/// commit token type (e.g., `KafkaToken`, `ZenohToken`, `MemoryToken`).
#[async_trait]
pub trait Transport: Send + Sync {
    /// The token type for this transport.
    type Token: CommitToken;

    /// Send raw bytes to a key/topic.
    ///
    /// Returns `SendResult::Ok` on success, `SendResult::Backpressured`
    /// if the transport cannot accept more messages, or `SendResult::Fatal`
    /// on unrecoverable errors.
    async fn send(&self, key: &str, payload: &[u8]) -> SendResult;

    /// Receive up to `max` messages.
    ///
    /// Returns immediately with available messages (may be fewer than `max`).
    /// Returns empty vec if no messages are available.
    async fn recv(&self, max: usize) -> TransportResult<Vec<Message<Self::Token>>>;

    /// Commit/acknowledge processed messages.
    ///
    /// For Kafka: commits consumer offsets.
    /// For Zenoh: no-op (no persistence).
    /// For Memory: advances internal sequence.
    async fn commit(&self, tokens: &[Self::Token]) -> TransportResult<()>;

    /// Shutdown gracefully.
    ///
    /// Flushes pending messages and closes connections.
    async fn close(&self) -> TransportResult<()>;

    /// Check if the transport is healthy and connected.
    fn is_healthy(&self) -> bool;

    /// Get transport name for logging/metrics.
    fn name(&self) -> &'static str;
}
