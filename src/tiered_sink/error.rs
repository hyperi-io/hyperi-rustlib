// Project:   hyperi-rustlib
// File:      src/tiered_sink/error.rs
// Purpose:   TieredSink error types
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! TieredSink error types.

use std::io;
use thiserror::Error;

/// Errors that can occur in TieredSink operations.
#[derive(Debug, Error)]
pub enum TieredSinkError {
    /// Failed to open or create the spool file.
    #[error("failed to open spool at {path}: {message}")]
    SpoolOpen { path: String, message: String },

    /// Spool operation failed.
    #[error("spool error: {0}")]
    Spool(String),

    /// Spool is full (max items or size reached).
    #[error("spool is full: {0}")]
    SpoolFull(String),

    /// Compression/decompression failed.
    #[error("codec error: {0}")]
    Codec(#[from] io::Error),

    /// Primary sink returned a fatal error.
    #[error("sink error: {0}")]
    Sink(String),

    /// Operation was cancelled.
    #[error("operation cancelled")]
    Cancelled,
}
