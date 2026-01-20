// Project:   hs-rustlib
// File:      src/license/error.rs
// Purpose:   License module error types
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Error types for the license module.

use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during license operations.
#[derive(Debug, Error)]
pub enum LicenseError {
    /// Failed to load license file from disk.
    #[error("failed to load license file '{path}': {reason}")]
    LoadFailed { path: PathBuf, reason: String },

    /// Failed to fetch license from URL.
    #[error("failed to fetch license from '{url}': {reason}")]
    FetchFailed { url: String, reason: String },

    /// Decryption of license data failed.
    #[error("license decryption failed: {reason}")]
    DecryptionFailed { reason: String },

    /// Encryption of license data failed.
    #[error("license encryption failed: {reason}")]
    EncryptionFailed { reason: String },

    /// License JSON parsing failed.
    #[error("invalid license format: {reason}")]
    ParseFailed { reason: String },

    /// License signature verification failed.
    #[error("license signature invalid: {reason}")]
    SignatureInvalid { reason: String },

    /// License has expired.
    #[error("license expired on {expiry}")]
    Expired { expiry: String },

    /// Integrity check failed (tampering detected).
    #[error("integrity check failed: {reason}")]
    IntegrityFailed { reason: String },

    /// License not initialized.
    #[error("license not initialized - call license::init() first")]
    NotInitialized,

    /// License already initialized.
    #[error("license already initialized")]
    AlreadyInitialized,
}
