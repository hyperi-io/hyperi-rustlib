// Project:   hyperi-rustlib
// File:      src/spool/error.rs
// Purpose:   Spool error types
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Spool error types.

use std::io;
use thiserror::Error;

/// Errors that can occur during spool operations.
#[derive(Debug, Error)]
pub enum SpoolError {
    /// Failed to open or create the queue file.
    #[error("failed to open spool at {path}: {message}")]
    Open { path: String, message: String },

    /// Queue file operation error.
    #[error("spool queue error: {0}")]
    Queue(String),

    /// I/O error during queue operations.
    #[error("spool I/O error: {0}")]
    Io(#[from] io::Error),

    /// Queue has reached its maximum item count.
    #[error("spool is full: maximum {max} items reached")]
    MaxItemsReached { max: usize },

    /// Queue has reached its maximum size.
    #[error("spool is full: maximum size {max_bytes} bytes reached")]
    MaxSizeReached { max_bytes: u64 },

    /// Compression error.
    #[error("compression error: {0}")]
    Compression(String),

    /// Decompression error.
    #[error("decompression error: {0}")]
    Decompression(String),

    /// Queue file is corrupted.
    #[error("spool file is corrupted: {0}")]
    Corrupted(String),
}
