// Project:   hyperi-rustlib
// File:      src/dlq/error.rs
// Purpose:   DLQ error types
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Error types for the DLQ module.

use thiserror::Error;

/// Errors from DLQ operations.
#[derive(Debug, Error)]
pub enum DlqError {
    /// File backend I/O error.
    #[error("file DLQ I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialisation error.
    #[error("DLQ serialisation error: {0}")]
    Serialization(String),

    /// File backend error (non-I/O).
    #[error("file DLQ error: {0}")]
    File(String),

    /// Kafka backend error.
    #[cfg(feature = "dlq-kafka")]
    #[error("kafka DLQ error: {0}")]
    Kafka(String),

    /// Generic backend error (HTTP, Redis, etc.).
    #[error("DLQ backend error: {0}")]
    BackendError(String),

    /// All configured backends failed.
    #[error("all DLQ backends failed: {0}")]
    AllBackendsFailed(String),

    /// DLQ is disabled or not configured.
    #[error("DLQ not configured")]
    NotConfigured,
}
