// Project:   hyperi-rustlib
// File:      src/dlq/config.rs
// Purpose:   DLQ configuration types
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Configuration for the DLQ module.
//!
//! Supports file-based and Kafka-based backends with cascade or fan-out modes.
//!
//! ## Config Cascade Example
//!
//! ```yaml
//! dlq:
//!   mode: cascade
//!   file:
//!     enabled: true
//!     path: /var/spool/dfe/dlq
//!     rotation: hourly
//!     max_age_days: 30
//!     compress_rotated: true
//!   kafka:
//!     enabled: true
//!     routing: per_table
//!     topic_suffix: .dlq
//!     common_topic: dfe.dlq
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// Re-export RotationPeriod from the shared io module so existing consumers
// of `dlq::RotationPeriod` continue to work without changes.
use crate::io::FileWriterConfig;
pub use crate::io::RotationPeriod;

/// How backends are used when multiple are enabled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DlqMode {
    /// Try backends in order; stop on first success.
    /// Default order: Kafka first, file fallback.
    #[default]
    Cascade,

    /// Write to all enabled backends; report any failures.
    FanOut,

    /// File backend only (no Kafka dependency).
    FileOnly,

    /// Kafka backend only (current dfe-loader behaviour).
    KafkaOnly,
}

/// Top-level DLQ configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DlqConfig {
    /// Whether DLQ is enabled.
    pub enabled: bool,

    /// Backend routing mode.
    pub mode: DlqMode,

    /// Bounded mpsc capacity. When the queue is full, `try_send` returns
    /// `QueueFull` (overflow=Drop). Sized for failure-burst tolerance.
    /// Default 10_000.
    pub queue_capacity: usize,

    /// Drain coalesces up to this many entries into one backend write.
    /// Default 256.
    pub batch_size: usize,

    /// Flush a partial batch after this duration, even if not full.
    /// Default 100 ms.
    pub flush_interval_ms: u64,

    /// File backend configuration.
    pub file: FileDlqConfig,

    /// Kafka backend configuration.
    #[cfg(feature = "dlq-kafka")]
    pub kafka: KafkaDlqConfig,

    /// HTTP backend configuration.
    #[cfg(feature = "dlq-http")]
    pub http: super::http::HttpDlqConfig,

    /// Redis backend configuration.
    #[cfg(feature = "dlq-redis")]
    pub redis: super::redis_dlq::RedisDlqConfig,
}

impl Default for DlqConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: DlqMode::default(),
            queue_capacity: 10_000,
            batch_size: 256,
            flush_interval_ms: 100,
            file: FileDlqConfig::default(),
            #[cfg(feature = "dlq-kafka")]
            kafka: KafkaDlqConfig::default(),
            #[cfg(feature = "dlq-http")]
            http: super::http::HttpDlqConfig::default(),
            #[cfg(feature = "dlq-redis")]
            redis: super::redis_dlq::RedisDlqConfig::default(),
        }
    }
}

/// File-based DLQ configuration.
///
/// Writes NDJSON files with automatic rotation and cleanup.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FileDlqConfig {
    /// Enable the file backend.
    pub enabled: bool,

    /// Base directory for DLQ files.
    /// Service name is appended as a subdirectory.
    pub path: PathBuf,

    /// File rotation period.
    pub rotation: RotationPeriod,

    /// Auto-cleanup files older than this many days.
    pub max_age_days: u32,

    /// Compress rotated files with flate2/gzip.
    pub compress_rotated: bool,
}

impl Default for FileDlqConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: PathBuf::from("/var/spool/dfe/dlq"),
            rotation: RotationPeriod::default(),
            max_age_days: 30,
            compress_rotated: true,
        }
    }
}

impl FileDlqConfig {
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

/// Kafka-based DLQ configuration.
#[cfg(feature = "dlq-kafka")]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KafkaDlqConfig {
    /// Enable the Kafka backend.
    pub enabled: bool,

    /// Topic routing strategy.
    pub routing: DlqRouting,

    /// Suffix appended to destination for per-table routing.
    pub topic_suffix: String,

    /// Common topic when routing is `Common` or destination is unknown.
    pub common_topic: String,

    /// Send timeout in milliseconds.
    pub send_timeout_ms: u64,
}

#[cfg(feature = "dlq-kafka")]
impl Default for KafkaDlqConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            routing: DlqRouting::default(),
            topic_suffix: ".dlq".to_string(),
            common_topic: "dfe.dlq".to_string(),
            send_timeout_ms: 5000,
        }
    }
}

/// Kafka DLQ topic routing strategy.
#[cfg(feature = "dlq-kafka")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DlqRouting {
    /// Route to topic matching destination with suffix.
    /// e.g. "acme.auth" → "acme.auth.dlq"
    #[default]
    PerTable,

    /// Route all failures to a single common topic.
    Common,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = DlqConfig::default();
        assert!(config.enabled);
        assert_eq!(config.mode, DlqMode::Cascade);
        assert!(config.file.enabled);
        assert_eq!(config.file.max_age_days, 30);
        assert!(config.file.compress_rotated);
        assert_eq!(config.file.rotation, RotationPeriod::Hourly);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = DlqConfig {
            mode: DlqMode::FanOut,
            file: FileDlqConfig {
                enabled: true,
                path: "/tmp/test-dlq".into(),
                rotation: RotationPeriod::Daily,
                max_age_days: 7,
                compress_rotated: false,
            },
            queue_capacity: 50_000,
            batch_size: 128,
            flush_interval_ms: 250,
            ..DlqConfig::default()
        };
        let json = serde_json::to_string(&config).expect("serialise");
        let parsed: DlqConfig = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(parsed.mode, DlqMode::FanOut);
        assert_eq!(parsed.file.rotation, RotationPeriod::Daily);
        assert_eq!(parsed.file.max_age_days, 7);
        assert_eq!(parsed.queue_capacity, 50_000);
        assert_eq!(parsed.batch_size, 128);
        assert_eq!(parsed.flush_interval_ms, 250);
    }

    #[test]
    fn test_dlq_mode_serde() {
        let json = r#""cascade""#;
        let mode: DlqMode = serde_json::from_str(json).expect("deserialise");
        assert_eq!(mode, DlqMode::Cascade);

        let json = r#""fan_out""#;
        let mode: DlqMode = serde_json::from_str(json).expect("deserialise");
        assert_eq!(mode, DlqMode::FanOut);
    }
}
