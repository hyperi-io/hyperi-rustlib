// Project:   hs-rustlib
// File:      src/spool/config.rs
// Purpose:   Spool configuration
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Spool configuration.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration for the disk-backed spool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpoolConfig {
    /// Path to the queue file.
    pub path: PathBuf,

    /// Enable zstd compression for stored items.
    /// Reduces disk usage at the cost of CPU.
    /// Defaults to false.
    #[serde(default)]
    pub compress: bool,

    /// Zstd compression level (1-22, higher = better compression, slower).
    /// Only used if `compress` is true.
    /// Defaults to 3 (fast compression).
    #[serde(default = "default_compression_level")]
    pub compression_level: i32,

    /// Maximum number of items in the queue.
    /// If set, push operations will fail when the limit is reached.
    /// Defaults to None (unlimited).
    #[serde(default)]
    pub max_items: Option<usize>,

    /// Maximum total size of the queue file in bytes.
    /// If set, push operations will fail when the limit is reached.
    /// Defaults to None (unlimited).
    #[serde(default)]
    pub max_size_bytes: Option<u64>,
}

fn default_compression_level() -> i32 {
    3
}

impl Default for SpoolConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("spool.queue"),
            compress: false,
            compression_level: default_compression_level(),
            max_items: None,
            max_size_bytes: None,
        }
    }
}

impl SpoolConfig {
    /// Create a new config with the given path.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            ..Default::default()
        }
    }

    /// Create a config with compression enabled.
    #[must_use]
    pub fn with_compression(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            compress: true,
            ..Default::default()
        }
    }

    /// Set whether compression is enabled.
    #[must_use]
    pub fn compress(mut self, enabled: bool) -> Self {
        self.compress = enabled;
        self
    }

    /// Set the compression level (1-22).
    #[must_use]
    pub fn compression_level(mut self, level: i32) -> Self {
        self.compression_level = level.clamp(1, 22);
        self
    }

    /// Set the maximum number of items.
    #[must_use]
    pub fn max_items(mut self, max: usize) -> Self {
        self.max_items = Some(max);
        self
    }

    /// Set the maximum queue file size in bytes.
    #[must_use]
    pub fn max_size_bytes(mut self, max: u64) -> Self {
        self.max_size_bytes = Some(max);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SpoolConfig::default();
        assert_eq!(config.path, PathBuf::from("spool.queue"));
        assert!(!config.compress);
        assert_eq!(config.compression_level, 3);
        assert!(config.max_items.is_none());
        assert!(config.max_size_bytes.is_none());
    }

    #[test]
    fn test_new_with_path() {
        let config = SpoolConfig::new("/tmp/test.queue");
        assert_eq!(config.path, PathBuf::from("/tmp/test.queue"));
    }

    #[test]
    fn test_with_compression() {
        let config = SpoolConfig::with_compression("/tmp/test.queue");
        assert!(config.compress);
    }

    #[test]
    fn test_builder_pattern() {
        let config = SpoolConfig::new("/tmp/test.queue")
            .compress(true)
            .compression_level(10)
            .max_items(1000)
            .max_size_bytes(1024 * 1024);

        assert!(config.compress);
        assert_eq!(config.compression_level, 10);
        assert_eq!(config.max_items, Some(1000));
        assert_eq!(config.max_size_bytes, Some(1024 * 1024));
    }

    #[test]
    fn test_compression_level_clamped() {
        let config = SpoolConfig::default().compression_level(100);
        assert_eq!(config.compression_level, 22);

        let config = SpoolConfig::default().compression_level(-5);
        assert_eq!(config.compression_level, 1);
    }
}
