// Project:   hs-rustlib
// File:      src/transport/error.rs
// Purpose:   Transport error types
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

use std::fmt;

/// Result type for transport operations.
pub type TransportResult<T> = Result<T, TransportError>;

/// Errors that can occur during transport operations.
#[derive(Debug)]
pub enum TransportError {
    /// Configuration error (missing or invalid config).
    Config(String),

    /// Connection error (network, auth, etc.).
    Connection(String),

    /// Send operation failed.
    Send(String),

    /// Receive operation failed.
    Recv(String),

    /// Commit/acknowledge operation failed.
    Commit(String),

    /// Transport is closed or shutting down.
    Closed,

    /// Timeout waiting for operation.
    Timeout,

    /// Backpressure - transport cannot accept more messages.
    Backpressure,

    /// Internal transport error.
    Internal(String),
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(msg) => write!(f, "transport config error: {msg}"),
            Self::Connection(msg) => write!(f, "transport connection error: {msg}"),
            Self::Send(msg) => write!(f, "transport send error: {msg}"),
            Self::Recv(msg) => write!(f, "transport receive error: {msg}"),
            Self::Commit(msg) => write!(f, "transport commit error: {msg}"),
            Self::Closed => write!(f, "transport closed"),
            Self::Timeout => write!(f, "transport operation timed out"),
            Self::Backpressure => write!(f, "transport backpressure"),
            Self::Internal(msg) => write!(f, "transport internal error: {msg}"),
        }
    }
}

impl std::error::Error for TransportError {}

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
