// Project:   hyperi-rustlib
// File:      src/cli/error.rs
// Purpose:   CLI error types
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Error types for the CLI module.

use thiserror::Error;

/// Errors from CLI operations.
#[derive(Debug, Error)]
pub enum CliError {
    /// Configuration loading failed.
    #[error("config error: {0}")]
    Config(String),

    /// Logger initialisation failed.
    #[error("logger error: {0}")]
    Logger(String),

    /// Metrics server failed.
    #[error("metrics error: {0}")]
    Metrics(String),

    /// Service runtime error.
    #[error("service error: {0}")]
    Service(String),

    /// Invalid CLI argument.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(feature = "logger")]
impl From<crate::logger::LoggerError> for CliError {
    fn from(e: crate::logger::LoggerError) -> Self {
        Self::Logger(e.to_string())
    }
}

#[cfg(feature = "config")]
impl From<crate::config::ConfigError> for CliError {
    fn from(e: crate::config::ConfigError) -> Self {
        Self::Config(e.to_string())
    }
}
