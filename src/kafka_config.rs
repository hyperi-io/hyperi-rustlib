// Project:   hyperi-rustlib
// File:      src/kafka_config.rs
// Purpose:   Shared Kafka librdkafka defaults, profiles, and file config loader
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Shared Kafka librdkafka configuration profiles, merge helper, and file loader.
//!
//! This module is always available (no feature gate). The core profile constants
//! and merge helper have zero external dependencies. File loading supports
//! `.properties` without any feature gate; YAML and JSON require the
//! `directory-config` and `config` features respectively.
//!
//! ## Loading from Config Git Directory
//!
//! Services store librdkafka settings in their config git directory and load
//! them with [`config_from_file`]:
//!
//! ```rust,ignore
//! use hyperi_rustlib::kafka_config::{config_from_file, merge_with_overrides, CONSUMER_PRODUCTION};
//!
//! let overrides = config_from_file("/config/kafka.properties")?;
//! let rdkafka_config = merge_with_overrides(CONSUMER_PRODUCTION, &overrides);
//! ```

use std::collections::HashMap;
use std::path::Path;

use thiserror::Error;

// ============================================================================
// Error Type
// ============================================================================

/// Error loading librdkafka configuration from a file.
#[derive(Debug, Error)]
pub enum KafkaConfigError {
    /// File does not exist.
    #[error("kafka config file not found: {path}")]
    FileNotFound { path: std::path::PathBuf },

    /// File extension is not supported (or feature is not enabled).
    #[error("unsupported kafka config format: {ext}. Supported: .properties, .yaml, .yml, .json")]
    UnsupportedFormat { ext: String },

    /// File content could not be parsed.
    #[error("parse error in {path}: {message}")]
    ParseError { path: String, message: String },

    /// I/O error reading the file.
    #[error("io error reading kafka config: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type for kafka config file operations.
pub type KafkaConfigResult<T> = Result<T, KafkaConfigError>;

// ============================================================================
// File Loading
// ============================================================================

/// Load librdkafka configuration from a file in the config git directory.
///
/// Detects format from file extension:
///
/// | Extension | Format | Requires |
/// |-----------|--------|---------|
/// | `.properties` | Java-style `key=value` | nothing (always available) |
/// | `.yaml`, `.yml` | YAML flat mapping | `directory-config` feature |
/// | `.json` | JSON object | `config` feature |
///
/// The returned map passes directly to [`merge_with_overrides`] or as
/// `librdkafka_overrides` in `KafkaConfig`.
///
/// # Errors
///
/// Returns [`KafkaConfigError`] if the file is missing, the format is
/// unsupported, or parsing fails.
pub fn config_from_file(path: impl AsRef<Path>) -> KafkaConfigResult<HashMap<String, String>> {
    let path = path.as_ref();

    let content = std::fs::read_to_string(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            KafkaConfigError::FileNotFound {
                path: path.to_path_buf(),
            }
        } else {
            KafkaConfigError::Io(e)
        }
    })?;

    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let path_str = path.display().to_string();

    match ext.as_str() {
        "properties" => Ok(config_from_properties_str(&content)),
        "yaml" | "yml" => parse_yaml(&content, path_str),
        "json" => parse_json(&content, path_str),
        other => Err(KafkaConfigError::UnsupportedFormat {
            ext: other.to_string(),
        }),
    }
}

/// Parse Java-style `.properties` content into a librdkafka config map.
///
/// Handles:
/// - `key=value` pairs (splits on first `=` only, so values may contain `=`)
/// - `#` and `!` comments
/// - Empty lines and surrounding whitespace
///
/// Always available with no feature gate.
#[must_use]
pub fn config_from_properties_str(content: &str) -> HashMap<String, String> {
    let mut config = HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            config.insert(key.trim().to_string(), value.trim().to_string());
        }
    }

    config
}

fn parse_yaml(content: &str, path: String) -> KafkaConfigResult<HashMap<String, String>> {
    #[cfg(feature = "directory-config")]
    {
        serde_yaml_ng::from_str(content).map_err(|e| KafkaConfigError::ParseError {
            path,
            message: e.to_string(),
        })
    }
    #[cfg(not(feature = "directory-config"))]
    {
        let _ = (content, path);
        Err(KafkaConfigError::UnsupportedFormat {
            ext: "yaml — enable the `directory-config` feature".to_string(),
        })
    }
}

fn parse_json(content: &str, path: String) -> KafkaConfigResult<HashMap<String, String>> {
    #[cfg(feature = "config")]
    {
        serde_json::from_str(content).map_err(|e| KafkaConfigError::ParseError {
            path,
            message: e.to_string(),
        })
    }
    #[cfg(not(feature = "config"))]
    {
        let _ = (content, path);
        Err(KafkaConfigError::UnsupportedFormat {
            ext: "json — enable the `config` feature".to_string(),
        })
    }
}

// ============================================================================
// Merge Helper
// ============================================================================

/// Merge profile defaults with user overrides.
///
/// Starts with `profile` defaults, then applies `overrides` on top.
/// User overrides always win.
#[must_use]
pub fn merge_with_overrides<S: std::hash::BuildHasher>(
    profile: &[(&str, &str)],
    overrides: &HashMap<String, String, S>,
) -> HashMap<String, String> {
    let mut config = HashMap::with_capacity(profile.len() + overrides.len());

    for (key, value) in profile {
        config.insert((*key).to_string(), (*value).to_string());
    }
    for (key, value) in overrides {
        config.insert(key.clone(), value.clone());
    }

    config
}

// ============================================================================
// Consumer Profiles
// ============================================================================

/// Production consumer baseline — lean, only non-defaults.
///
/// | Setting | Value | librdkafka Default | Why |
/// |---|---|---|---|
/// | `partition.assignment.strategy` | `cooperative-sticky` | `range,roundrobin` | KIP-429: avoids stop-the-world rebalances |
/// | `fetch.min.bytes` | 1 MiB | 1 byte | Batch fetches for throughput |
/// | `fetch.wait.max.ms` | 100 ms | 500 ms | Bound latency when fetch.min.bytes not met |
/// | `queued.min.messages` | 20000 | 100000 | 10-20K batches are most efficient |
/// | `enable.auto.commit` | false | true | DFE services manage offset commits |
/// | `statistics.interval.ms` | 1000 ms | 0 (disabled) | Enable Prometheus metrics |
pub const CONSUMER_PRODUCTION: &[(&str, &str)] = &[
    ("partition.assignment.strategy", "cooperative-sticky"),
    ("fetch.min.bytes", "1048576"),
    ("fetch.wait.max.ms", "100"),
    ("queued.min.messages", "20000"),
    ("enable.auto.commit", "false"),
    ("statistics.interval.ms", "1000"),
];

/// Development/test consumer baseline — fast iteration, low memory.
///
/// | Setting | Value | librdkafka Default | Why |
/// |---|---|---|---|
/// | `partition.assignment.strategy` | `cooperative-sticky` | `range,roundrobin` | Consistent across environments |
/// | `queued.min.messages` | 1000 | 100000 | Lower memory for dev machines |
/// | `enable.auto.commit` | false | true | DFE services manage commits |
/// | `reconnect.backoff.ms` | 10 ms | 100 ms | Fast reconnect for quick iteration |
/// | `reconnect.backoff.max.ms` | 100 ms | 10000 ms | Cap quickly |
/// | `log.connection.close` | true | false | Debug-friendly |
/// | `statistics.interval.ms` | 1000 ms | 0 (disabled) | Enable metrics even in dev |
pub const CONSUMER_DEVTEST: &[(&str, &str)] = &[
    ("partition.assignment.strategy", "cooperative-sticky"),
    ("queued.min.messages", "1000"),
    ("enable.auto.commit", "false"),
    ("reconnect.backoff.ms", "10"),
    ("reconnect.backoff.max.ms", "100"),
    ("log.connection.close", "true"),
    ("statistics.interval.ms", "1000"),
];

/// Low-latency consumer — minimal fetch delay.
///
/// | Setting | Value | librdkafka Default | Why |
/// |---|---|---|---|
/// | `partition.assignment.strategy` | `cooperative-sticky` | `range,roundrobin` | Consistent across envs |
/// | `fetch.wait.max.ms` | 10 ms | 500 ms | Return quickly |
/// | `queued.min.messages` | 1000 | 100000 | Smaller pre-fetch queue |
/// | `enable.auto.commit` | false | true | DFE manages commits |
/// | `reconnect.backoff.ms` | 10 ms | 100 ms | Fast reconnect |
/// | `reconnect.backoff.max.ms` | 100 ms | 10000 ms | Cap quickly |
/// | `statistics.interval.ms` | 1000 ms | 0 | Enable metrics |
pub const CONSUMER_LOW_LATENCY: &[(&str, &str)] = &[
    ("partition.assignment.strategy", "cooperative-sticky"),
    ("fetch.wait.max.ms", "10"),
    ("queued.min.messages", "1000"),
    ("enable.auto.commit", "false"),
    ("reconnect.backoff.ms", "10"),
    ("reconnect.backoff.max.ms", "100"),
    ("statistics.interval.ms", "1000"),
];

// ============================================================================
// Producer Profiles
// ============================================================================

/// Production producer baseline — high throughput, zstd compression.
///
/// | Setting | Value | librdkafka Default | Why |
/// |---|---|---|---|
/// | `linger.ms` | 100 ms | 5 ms | Accumulate larger batches |
/// | `compression.type` | zstd | none | Best ratio with good CPU |
/// | `socket.nagle.disable` | true | false | Kafka batches at app level |
/// | `statistics.interval.ms` | 1000 ms | 0 (disabled) | Enable Prometheus metrics |
pub const PRODUCER_PRODUCTION: &[(&str, &str)] = &[
    ("linger.ms", "100"),
    ("compression.type", "zstd"),
    ("socket.nagle.disable", "true"),
    ("statistics.interval.ms", "1000"),
];

/// Exactly-once producer — idempotence + ordering.
///
/// | Setting | Value | librdkafka Default | Why |
/// |---|---|---|---|
/// | `enable.idempotence` | true | false | Exactly-once within partition |
/// | `acks` | all | all (-1) | Invariant for EOS (explicit) |
/// | `max.in.flight.requests.per.connection` | 5 | 1000000 | Max for idempotent producer |
/// | `linger.ms` | 20 ms | 5 ms | Moderate batching |
/// | `compression.type` | zstd | none | Best ratio |
/// | `socket.nagle.disable` | true | false | Kafka batches at app level |
/// | `statistics.interval.ms` | 1000 ms | 0 | Enable metrics |
pub const PRODUCER_EXACTLY_ONCE: &[(&str, &str)] = &[
    ("enable.idempotence", "true"),
    ("acks", "all"),
    ("max.in.flight.requests.per.connection", "5"),
    ("linger.ms", "20"),
    ("compression.type", "zstd"),
    ("socket.nagle.disable", "true"),
    ("statistics.interval.ms", "1000"),
];

/// Low-latency producer — minimal delay, leader-ack only.
///
/// | Setting | Value | librdkafka Default | Why |
/// |---|---|---|---|
/// | `acks` | 1 | all (-1) | Leader ack only for speed |
/// | `linger.ms` | 0 ms | 5 ms | Send immediately |
/// | `compression.type` | lz4 | none | LZ4 is fastest codec |
/// | `socket.nagle.disable` | true | false | No TCP coalescing |
/// | `statistics.interval.ms` | 1000 ms | 0 | Enable metrics |
pub const PRODUCER_LOW_LATENCY: &[(&str, &str)] = &[
    ("acks", "1"),
    ("linger.ms", "0"),
    ("compression.type", "lz4"),
    ("socket.nagle.disable", "true"),
    ("statistics.interval.ms", "1000"),
];

/// DevTest producer — fast acks, no compression.
///
/// | Setting | Value | librdkafka Default | Why |
/// |---|---|---|---|
/// | `acks` | 1 | all (-1) | Faster for dev |
/// | `socket.nagle.disable` | true | false | No TCP coalescing |
/// | `statistics.interval.ms` | 1000 ms | 0 | Enable metrics in dev |
pub const PRODUCER_DEVTEST: &[(&str, &str)] = &[
    ("acks", "1"),
    ("socket.nagle.disable", "true"),
    ("statistics.interval.ms", "1000"),
];

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumer_production_only_non_defaults() {
        assert_eq!(CONSUMER_PRODUCTION.len(), 6);
        let map: HashMap<&str, &str> = CONSUMER_PRODUCTION.iter().copied().collect();
        assert_eq!(map["partition.assignment.strategy"], "cooperative-sticky");
        assert_eq!(map["fetch.min.bytes"], "1048576");
        assert_eq!(map["fetch.wait.max.ms"], "100");
        assert_eq!(map["queued.min.messages"], "20000");
        assert_eq!(map["enable.auto.commit"], "false");
        assert_eq!(map["statistics.interval.ms"], "1000");
    }

    #[test]
    fn producer_production_only_non_defaults() {
        assert_eq!(PRODUCER_PRODUCTION.len(), 4);
        let map: HashMap<&str, &str> = PRODUCER_PRODUCTION.iter().copied().collect();
        assert_eq!(map["linger.ms"], "100");
        assert_eq!(map["compression.type"], "zstd");
        assert_eq!(map["socket.nagle.disable"], "true");
        assert_eq!(map["statistics.interval.ms"], "1000");
    }

    #[test]
    fn merge_user_overrides_win() {
        let mut overrides = HashMap::new();
        overrides.insert("fetch.min.bytes".to_string(), "2097152".to_string());
        overrides.insert("custom.setting".to_string(), "value".to_string());

        let merged = merge_with_overrides(CONSUMER_PRODUCTION, &overrides);

        assert_eq!(merged["fetch.min.bytes"], "2097152");
        assert_eq!(merged["custom.setting"], "value");
        assert_eq!(
            merged["partition.assignment.strategy"],
            "cooperative-sticky"
        );
    }

    #[test]
    fn merge_empty_overrides_returns_profile() {
        let overrides = HashMap::new();
        let merged = merge_with_overrides(CONSUMER_PRODUCTION, &overrides);
        assert_eq!(merged.len(), CONSUMER_PRODUCTION.len());
    }

    #[test]
    fn merge_empty_profile_returns_overrides() {
        let mut overrides = HashMap::new();
        overrides.insert("key".to_string(), "value".to_string());
        let merged = merge_with_overrides(&[], &overrides);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged["key"], "value");
    }

    #[test]
    fn properties_str_basic() {
        let content = "\
# This is a comment
bootstrap.servers=kafka1:9092,kafka2:9092
security.protocol=SASL_SSL
sasl.mechanism=SCRAM-SHA-512
! Another comment style
";
        let config = config_from_properties_str(content);
        assert_eq!(config.len(), 3);
        assert_eq!(config["bootstrap.servers"], "kafka1:9092,kafka2:9092");
        assert_eq!(config["security.protocol"], "SASL_SSL");
        assert_eq!(config["sasl.mechanism"], "SCRAM-SHA-512");
    }

    #[test]
    fn properties_str_value_with_equals() {
        let content = "ssl.certificate.pem=MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMI==\n";
        let config = config_from_properties_str(content);
        assert_eq!(
            config["ssl.certificate.pem"],
            "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMI=="
        );
    }

    #[test]
    fn properties_str_empty_and_whitespace() {
        let config = config_from_properties_str("   \n# comment\n\n");
        assert!(config.is_empty());
    }

    #[test]
    fn config_from_file_properties() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kafka.properties");
        std::fs::write(
            &path,
            "bootstrap.servers=kafka:9092\ncompression.type=zstd\n",
        )
        .unwrap();

        let config = config_from_file(&path).unwrap();
        assert_eq!(config["bootstrap.servers"], "kafka:9092");
        assert_eq!(config["compression.type"], "zstd");
    }

    #[test]
    fn config_from_file_not_found() {
        let result = config_from_file("/nonexistent/kafka.properties");
        assert!(matches!(result, Err(KafkaConfigError::FileNotFound { .. })));
    }

    #[test]
    fn config_from_file_unsupported_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kafka.toml");
        std::fs::write(&path, "key = value\n").unwrap();

        let result = config_from_file(&path);
        assert!(matches!(
            result,
            Err(KafkaConfigError::UnsupportedFormat { .. })
        ));
    }
}
