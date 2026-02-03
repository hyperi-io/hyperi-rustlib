// Project:   hs-rustlib
// File:      src/secrets/error.rs
// Purpose:   Secrets error types
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Error types for the secrets module.

use thiserror::Error;

/// Secrets module errors.
#[derive(Debug, Error)]
pub enum SecretsError {
    /// Secret not found.
    #[error("secret not found: {0}")]
    NotFound(String),

    /// Provider not configured.
    #[error("provider not configured: {0}")]
    ProviderNotConfigured(String),

    /// Provider communication error.
    #[error("provider error: {0}")]
    ProviderError(String),

    /// Authentication failed.
    #[error("authentication failed: {0}")]
    AuthError(String),

    /// I/O error.
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    /// Cache error.
    #[error("cache error: {0}")]
    CacheError(String),

    /// Invalid secret data.
    #[error("invalid data: {0}")]
    InvalidData(String),

    /// Refresh failed.
    #[error("refresh failed: {0}")]
    RefreshFailed(String),

    /// Configuration error.
    #[error("configuration error: {0}")]
    ConfigError(String),
}

/// Result type for secrets operations.
pub type SecretsResult<T> = Result<T, SecretsError>;
