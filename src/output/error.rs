// Project:   hyperi-rustlib
// File:      src/output/error.rs
// Purpose:   File output sink error types
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Error types for the file output sink.

use thiserror::Error;

/// Errors from file output operations.
#[derive(Debug, Error)]
pub enum OutputError {
    /// I/O error writing to file.
    #[error("output I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Output sink is disabled.
    #[error("file output sink is disabled")]
    Disabled,
}
