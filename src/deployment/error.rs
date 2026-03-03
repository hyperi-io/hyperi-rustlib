// Project:   hyperi-rustlib
// File:      src/deployment/error.rs
// Purpose:   Deployment validation error types
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Deployment validation error types.

use thiserror::Error;

/// A single contract mismatch between app defaults and deployment artifact.
#[derive(Debug, Clone)]
pub struct ContractMismatch {
    /// What was checked (e.g., "service.port", "EXPOSE port").
    pub field: String,
    /// Expected value (from app config).
    pub expected: String,
    /// Actual value (from deployment artifact).
    pub actual: String,
}

impl std::fmt::Display for ContractMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: expected '{}', got '{}'",
            self.field, self.expected, self.actual
        )
    }
}

/// Errors from deployment validation and generation.
#[derive(Debug, Error)]
pub enum DeploymentError {
    /// Failed to read a deployment artifact file.
    #[error("failed to read {path}: {source}")]
    ReadFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// Failed to write a deployment artifact file.
    #[error("failed to write {path}: {source}")]
    WriteFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// Failed to create a directory.
    #[error("failed to create directory {path}: {source}")]
    CreateDir {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// Failed to parse YAML.
    #[error("failed to parse YAML in {path}: {source}")]
    ParseYaml {
        path: String,
        #[source]
        source: serde_yaml_ng::Error,
    },

    /// File not found.
    #[error("file not found: {0}")]
    NotFound(String),
}
