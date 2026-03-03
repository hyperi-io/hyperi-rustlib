// Project:   hyperi-rustlib
// File:      src/output/config.rs
// Purpose:   File output sink configuration
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Configuration for the file output sink.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::io::{FileWriterConfig, RotationPeriod};

/// File output sink configuration.
///
/// Writes raw NDJSON events to rotating files — used for testing and
/// bare-metal deployments where Kafka is not available.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FileOutputConfig {
    /// Enable the file output sink.
    pub enabled: bool,

    /// Base directory for output files.
    pub path: PathBuf,

    /// Output filename (e.g. "events.ndjson").
    pub filename: String,

    /// File rotation period.
    pub rotation: RotationPeriod,

    /// Auto-cleanup files older than this many days.
    pub max_age_days: u32,

    /// Compress rotated files with flate2/gzip.
    pub compress_rotated: bool,
}

impl Default for FileOutputConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: PathBuf::from("/var/spool/dfe/output"),
            filename: "events.ndjson".to_string(),
            rotation: RotationPeriod::Hourly,
            max_age_days: 7,
            compress_rotated: true,
        }
    }
}

impl FileOutputConfig {
    /// Convert to the shared `FileWriterConfig` for use with `NdjsonWriter`.
    #[must_use]
    pub fn to_writer_config(&self) -> FileWriterConfig {
        FileWriterConfig {
            path: self.path.clone(),
            rotation: self.rotation,
            max_age_days: self.max_age_days,
            compress_rotated: self.compress_rotated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let config = FileOutputConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.path, PathBuf::from("/var/spool/dfe/output"));
        assert_eq!(config.filename, "events.ndjson");
        assert_eq!(config.rotation, RotationPeriod::Hourly);
        assert_eq!(config.max_age_days, 7);
        assert!(config.compress_rotated);
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = FileOutputConfig {
            enabled: true,
            path: "/tmp/test-output".into(),
            filename: "data.ndjson".into(),
            rotation: RotationPeriod::Daily,
            max_age_days: 30,
            compress_rotated: false,
        };
        let json = serde_json::to_string(&config).expect("serialise");
        let parsed: FileOutputConfig = serde_json::from_str(&json).expect("deserialise");
        assert!(parsed.enabled);
        assert_eq!(parsed.filename, "data.ndjson");
        assert_eq!(parsed.rotation, RotationPeriod::Daily);
    }
}
