// Project:   hyperi-rustlib
// File:      src/io/config.rs
// Purpose:   Shared file writer configuration types
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Shared configuration for NDJSON file writers.
//!
//! Used by both the DLQ file backend and the file output sink.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// File rotation period.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationPeriod {
    /// Rotate files every hour.
    #[default]
    Hourly,
    /// Rotate files every day.
    Daily,
}

/// Shared file writer configuration.
///
/// Contains the core settings for rotating NDJSON file output. Used by
/// [`super::NdjsonWriter`] and consumed by both DLQ and file output modules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileWriterConfig {
    /// Base directory for output files.
    pub path: PathBuf,

    /// File rotation period.
    pub rotation: RotationPeriod,

    /// Auto-cleanup files older than this many days.
    pub max_age_days: u32,

    /// Compress rotated files with flate2/gzip.
    pub compress_rotated: bool,
}

impl Default for FileWriterConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("/var/spool/dfe"),
            rotation: RotationPeriod::default(),
            max_age_days: 30,
            compress_rotated: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let config = FileWriterConfig::default();
        assert_eq!(config.path, PathBuf::from("/var/spool/dfe"));
        assert_eq!(config.rotation, RotationPeriod::Hourly);
        assert_eq!(config.max_age_days, 30);
        assert!(config.compress_rotated);
    }

    #[test]
    fn test_rotation_period_serde() {
        let json = r#""hourly""#;
        let period: RotationPeriod = serde_json::from_str(json).expect("deserialise");
        assert_eq!(period, RotationPeriod::Hourly);

        let json = r#""daily""#;
        let period: RotationPeriod = serde_json::from_str(json).expect("deserialise");
        assert_eq!(period, RotationPeriod::Daily);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = FileWriterConfig {
            path: "/tmp/test".into(),
            rotation: RotationPeriod::Daily,
            max_age_days: 7,
            compress_rotated: false,
        };
        let json = serde_json::to_string(&config).expect("serialise");
        let parsed: FileWriterConfig = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(parsed.path, PathBuf::from("/tmp/test"));
        assert_eq!(parsed.rotation, RotationPeriod::Daily);
        assert_eq!(parsed.max_age_days, 7);
        assert!(!parsed.compress_rotated);
    }
}
