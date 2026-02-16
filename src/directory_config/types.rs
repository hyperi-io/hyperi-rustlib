// Project:   hyperi-rustlib
// File:      src/directory_config/types.rs
// Purpose:   Configuration types for DirectoryConfigStore
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Configuration for the directory config store.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DirectoryConfigStoreConfig {
    /// Path to the YAML config directory.
    pub directory: PathBuf,

    /// Polling interval for background refresh (default: 30s).
    pub refresh_interval: Duration,

    /// Enable git integration if directory is a git repo (default: true).
    pub git_enabled: bool,

    /// Push commits to remote after write (default: false).
    pub git_push: bool,

    /// Git author name for commits.
    pub git_author_name: String,

    /// Git author email for commits.
    pub git_author_email: String,
}

impl Default for DirectoryConfigStoreConfig {
    fn default() -> Self {
        Self {
            directory: PathBuf::from("/etc/dfe/config"),
            refresh_interval: Duration::from_secs(30),
            git_enabled: true,
            git_push: false,
            git_author_name: "DirectoryConfigStore".to_string(),
            git_author_email: "config@hyperi.io".to_string(),
        }
    }
}

/// Detected write capability of the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// Directory is not writable.
    ReadOnly,
    /// Directory is writable, direct file writes.
    DirectWrite,
    /// Directory is writable and is a git repo, changes are committed.
    GitCommit,
}

/// Describes a change to a config table.
#[derive(Debug, Clone)]
pub struct ChangeEvent {
    /// Table name that changed.
    pub table: String,
    /// Type of change.
    pub operation: ChangeOperation,
}

/// Type of change operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeOperation {
    /// Table was created or a key was set.
    Updated,
    /// Table or key was deleted.
    Deleted,
    /// Table was refreshed from disk (background poll).
    Refreshed,
}

/// Result of a write operation.
#[derive(Debug, Clone)]
pub struct WriteResult {
    /// Table that was modified.
    pub table: String,
    /// Operation performed.
    pub operation: ChangeOperation,
    /// Git branch (if git mode).
    pub branch: Option<String>,
    /// Git commit hash (if git mode).
    pub commit: Option<String>,
}
