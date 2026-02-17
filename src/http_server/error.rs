// Project:   hyperi-rustlib
// File:      src/http_server/error.rs
// Purpose:   HTTP server error types
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! HTTP server error types.

use std::io;
use thiserror::Error;

/// Errors that can occur when running the HTTP server.
#[derive(Debug, Error)]
pub enum HttpServerError {
    /// Failed to bind to the specified address.
    #[error("failed to bind to {address}: {source}")]
    Bind {
        address: String,
        #[source]
        source: io::Error,
    },

    /// Failed to load TLS configuration.
    #[error("failed to load TLS configuration: {0}")]
    TlsConfig(String),

    /// Server encountered an I/O error.
    #[error("server I/O error: {0}")]
    Io(#[from] io::Error),

    /// Graceful shutdown timed out.
    #[error("graceful shutdown timed out after {timeout_secs}s")]
    ShutdownTimeout { timeout_secs: u64 },

    /// Server error during operation.
    #[error("server error: {0}")]
    Server(String),
}
