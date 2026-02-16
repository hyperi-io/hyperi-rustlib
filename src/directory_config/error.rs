// Project:   hyperi-rustlib
// File:      src/directory_config/error.rs
// Purpose:   Error types for DirectoryConfigStore
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use thiserror::Error;

/// Errors from directory config store operations.
#[derive(Debug, Error)]
pub enum DirectoryConfigError {
    /// Invalid table name (path traversal, backslash, etc.).
    #[error("invalid table name: {0}")]
    InvalidTableName(String),

    /// Store has not been started.
    #[error("store not started")]
    NotStarted,

    /// Store is already running.
    #[error("store already running")]
    AlreadyRunning,

    /// Table (YAML file) not found.
    #[error("table not found: {0}")]
    TableNotFound(String),

    /// Key not found within a table.
    #[error("key '{key}' not found in table '{table}'")]
    KeyNotFound { table: String, key: String },

    /// Table already exists (on create).
    #[error("table already exists: {0}")]
    TableExists(String),

    /// Store is read-only, writes not permitted.
    #[error("store is read-only")]
    ReadOnly,

    /// YAML parse error.
    #[error("YAML parse error in '{file}': {message}")]
    ParseError { file: String, message: String },

    /// YAML serialisation error.
    #[error("YAML serialisation error: {0}")]
    SerializationError(String),

    /// I/O error.
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    /// Directory does not exist or is not a directory.
    #[error("directory not found: {0}")]
    DirectoryNotFound(String),

    /// Git operation failed.
    #[cfg(feature = "directory-config-git")]
    #[error("git error: {0}")]
    GitError(String),

    /// Git not available but git operation requested.
    #[error("not a git repository")]
    NotGitRepo,
}

/// Result type alias for directory config operations.
pub type DirectoryConfigResult<T> = Result<T, DirectoryConfigError>;
