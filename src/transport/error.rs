// Project:   hyperi-rustlib
// File:      src/transport/error.rs
// Purpose:   Transport error types
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

use thiserror::Error;

/// Result type for transport operations.
pub type TransportResult<T> = Result<T, TransportError>;

/// Errors that can occur during transport operations.
#[derive(Debug, Error)]
pub enum TransportError {
    /// Configuration error (missing or invalid config).
    #[error("transport config error: {0}")]
    Config(String),

    /// Connection error (network, auth, etc.).
    #[error("transport connection error: {0}")]
    Connection(String),

    /// Send operation failed.
    #[error("transport send error: {0}")]
    Send(String),

    /// Receive operation failed.
    #[error("transport receive error: {0}")]
    Recv(String),

    /// Commit/acknowledge operation failed.
    #[error("transport commit error: {0}")]
    Commit(String),

    /// Transport is closed or shutting down.
    #[error("transport closed")]
    Closed,

    /// Timeout waiting for operation.
    #[error("transport operation timed out")]
    Timeout,

    /// Backpressure - transport cannot accept more messages.
    #[error("transport backpressure")]
    Backpressure,

    /// Internal transport error.
    #[error("transport internal error: {0}")]
    Internal(String),

    /// Admin operation error (topic/partition management).
    #[error("transport admin error: {0}")]
    Admin(String),
}

impl TransportError {
    /// Returns true if this error is recoverable (retry may succeed).
    #[must_use]
    pub fn is_recoverable(&self) -> bool {
        matches!(self, Self::Timeout | Self::Backpressure)
    }

    /// Returns true if this error indicates the transport is unusable.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::Closed | Self::Config(_))
    }
}
