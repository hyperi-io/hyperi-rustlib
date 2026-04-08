// Project:   hyperi-rustlib
// File:      src/transport/kafka/config.rs
// Purpose:   Kafka transport configuration with profiles and config-driven overrides
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Kafka configuration with profile-based defaults and config-driven overrides.
//!
//! ## Profile System
//!
//! HyperI Kafka uses a profile-based configuration system where:
//! 1. A **profile** provides opinionated librdkafka defaults for a use case
//! 2. **User config** can override any librdkafka setting via `librdkafka_overrides`
//! 3. Overrides always win over profile defaults
//!
//! ## Available Profiles
//!
//! - **`production`**: High-throughput, PB/day workloads. Large queues, cooperative
//!   rebalancing, disabled CRC checks, optimized fetch parameters.
//! - **`devtest`**: Development and testing. Relaxed SSL validation, smaller queues,
//!   faster reconnection, debug-friendly settings.
//!
//! ## Example YAML Config
//!
//! ```yaml
//! kafka:
//!   profile: production
//!   brokers:
//!     - kafka1:9092
//!     - kafka2:9092
//!   group: my-consumer-group
//!   topics:
//!     - events
//!   # Override specific librdkafka settings
//!   librdkafka_overrides:
//!     fetch.min.bytes: "2097152"  # 2MB instead of profile's 1MB
//!     statistics.interval.ms: "5000"
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;

// ============================================================================
// Topic Resolution Types
// ============================================================================

/// Topic suppression rule for auto-discovery.
///
/// When auto-discovering topics, if a topic with `preferred_suffix` exists
/// for a base name, the topic with `suppressed_suffix` for that same base
/// is removed. Default: `_load` suppresses `_land`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuppressionRule {
    /// The suffix of the preferred (kept) topic.
    pub preferred_suffix: String,
    /// The suffix of the suppressed (removed) topic.
    pub suppressed_suffix: String,
}

// ============================================================================
// Profile System
// ============================================================================

/// Kafka configuration profile.
///
/// Profiles provide opinionated librdkafka defaults for specific use cases.
/// Users can override any setting via `librdkafka_overrides`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KafkaProfile {
    /// Production profile: lean baseline for all DFE services.
    ///
    /// Only sets values that differ from librdkafka defaults.
    /// Services add overrides via `librdkafka_overrides`.
    #[default]
    Production,

    /// Development/test profile: fast iteration, low memory.
    ///
    /// Cooperative rebalancing, fast reconnects, debug logging.
    /// SSL certificate verification disabled by default.
    DevTest,
}

impl FromStr for KafkaProfile {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "production" | "prod" => Ok(Self::Production),
            "devtest" | "dev" | "test" | "development" => Ok(Self::DevTest),
            _ => Err(format!(
                "Unknown Kafka profile: {s}. Valid: production, devtest"
            )),
        }
    }
}

impl std::fmt::Display for KafkaProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Production => write!(f, "production"),
            Self::DevTest => write!(f, "devtest"),
        }
    }
}

// ============================================================================
// Merge Helper
// ============================================================================

/// Merge profile defaults with user overrides.
///
/// Starts with `profile` defaults, then applies `overrides` on top.
/// User overrides always win. Returns the final merged map.
///
/// # Example
///
/// ```
/// use std::collections::HashMap;
/// use hyperi_rustlib::transport::kafka::config::{merge_with_overrides, PRODUCTION_PROFILE};
///
/// let mut overrides = HashMap::new();
/// overrides.insert("fetch.min.bytes".to_string(), "2097152".to_string());
///
/// let merged = merge_with_overrides(PRODUCTION_PROFILE, &overrides);
/// assert_eq!(merged.get("fetch.min.bytes").unwrap(), "2097152");
/// assert_eq!(merged.get("partition.assignment.strategy").unwrap(), "cooperative-sticky");
/// ```
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
// Profile Defaults
// ============================================================================

/// Production consumer profile — lean baseline.
///
/// Only settings that differ from librdkafka defaults with clear justification.
/// Services override on an exception basis via `librdkafka_overrides`.
///
/// | Setting | Value | librdkafka default | Why |
/// |---|---|---|---|
/// | `partition.assignment.strategy` | `cooperative-sticky` | `range,roundrobin` | KIP-429: avoids stop-the-world rebalances |
/// | `fetch.min.bytes` | 1 MiB | 1 byte | Batch fetches for throughput |
/// | `fetch.wait.max.ms` | 100 ms | 500 ms | Bound latency when fetch.min.bytes not met |
/// | `queued.min.messages` | 20000 | 100000 | 10-20K batches are most efficient |
/// | `enable.auto.commit` | false | true | DFE services manage offset commits |
/// | `statistics.interval.ms` | 1000 ms | 0 (disabled) | Enable Prometheus metrics |
pub const PRODUCTION_PROFILE: &[(&str, &str)] = &[
    ("partition.assignment.strategy", "cooperative-sticky"),
    ("fetch.min.bytes", "1048576"),
    ("fetch.wait.max.ms", "100"),
    ("queued.min.messages", "20000"),
    ("enable.auto.commit", "false"),
    ("statistics.interval.ms", "1000"),
];

/// Development/test consumer profile — minimal latency, low memory.
///
/// Inherits the same "only non-defaults" philosophy. Optimised for fast
/// iteration on developer machines: no fetch batching, smaller queues,
/// fast reconnects, debug logging.
///
/// | Setting | Value | librdkafka default | Why |
/// |---|---|---|---|
/// | `partition.assignment.strategy` | `cooperative-sticky` | `range,roundrobin` | Consistent across all environments |
/// | `queued.min.messages` | 1000 | 100000 | Lower memory for dev machines |
/// | `enable.auto.commit` | false | true | DFE services manage commits |
/// | `reconnect.backoff.ms` | 10 ms | 100 ms | Fast reconnect for quick iteration |
/// | `reconnect.backoff.max.ms` | 100 ms | 10000 ms | Cap quickly |
/// | `log.connection.close` | true | false | Debug-friendly |
/// | `statistics.interval.ms` | 1000 ms | 0 (disabled) | Enable metrics even in dev |
pub const DEVTEST_PROFILE: &[(&str, &str)] = &[
    ("partition.assignment.strategy", "cooperative-sticky"),
    ("queued.min.messages", "1000"),
    ("enable.auto.commit", "false"),
    ("reconnect.backoff.ms", "10"),
    ("reconnect.backoff.max.ms", "100"),
    ("log.connection.close", "true"),
    ("statistics.interval.ms", "1000"),
];

// ============================================================================
// Producer Profile Defaults
// ============================================================================

/// High-throughput producer — lean baseline.
///
/// Only settings that differ from librdkafka defaults.
/// Services override via `librdkafka_overrides`.
///
/// | Setting | Value | librdkafka default | Why |
/// |---|---|---|---|
/// | `linger.ms` | 100 ms | 5 ms | Accumulate larger batches |
/// | `compression.type` | zstd | none | Best ratio with good CPU |
/// | `socket.nagle.disable` | true | false | Kafka batches at app level |
/// | `statistics.interval.ms` | 1000 ms | 0 (disabled) | Enable Prometheus metrics |
pub const PRODUCER_HIGH_THROUGHPUT: &[(&str, &str)] = &[
    ("linger.ms", "100"),
    ("compression.type", "zstd"),
    ("socket.nagle.disable", "true"),
    ("statistics.interval.ms", "1000"),
];

/// Exactly-once producer — idempotence + ordering.
///
/// Only settings that differ from librdkafka defaults.
/// `acks=all` and `max.in.flight=5` are already defaults but explicit
/// here because they are *invariants* for exactly-once correctness.
///
/// | Setting | Value | librdkafka default | Why |
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
/// Only settings that differ from librdkafka defaults.
///
/// | Setting | Value | librdkafka default | Why |
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
/// Only settings that differ from librdkafka defaults.
///
/// | Setting | Value | librdkafka default | Why |
/// |---|---|---|---|
/// | `acks` | 1 | all (-1) | Faster for dev |
/// | `linger.ms` | 5 ms | 5 ms | Default is fine for dev |
/// | `compression.type` | none | none | No overhead in dev |
/// | `socket.nagle.disable` | true | false | No TCP coalescing |
/// | `statistics.interval.ms` | 1000 ms | 0 | Enable metrics in dev |
pub const PRODUCER_DEVTEST: &[(&str, &str)] = &[
    ("acks", "1"),
    ("socket.nagle.disable", "true"),
    ("statistics.interval.ms", "1000"),
];

/// Legacy producer defaults — now aliases `PRODUCER_HIGH_THROUGHPUT`.
#[deprecated(since = "2.0.0", note = "Use PRODUCER_HIGH_THROUGHPUT instead")]
pub const PRODUCER_DEFAULTS: &[(&str, &str)] = PRODUCER_HIGH_THROUGHPUT;

// ============================================================================
// Legacy Constants (for backward compatibility)
// ============================================================================

/// Alias for `PRODUCTION_PROFILE` (backward compatibility).
#[deprecated(since = "2.0.0", note = "Use PRODUCTION_PROFILE instead")]
pub const HIGH_THROUGHPUT_CONSUMER_DEFAULTS: &[(&str, &str)] = PRODUCTION_PROFILE;

/// Low-latency consumer — minimal fetch delay.
///
/// Only settings that differ from librdkafka defaults.
///
/// | Setting | Value | librdkafka default | Why |
/// |---|---|---|---|
/// | `partition.assignment.strategy` | `cooperative-sticky` | `range,roundrobin` | Consistent across envs |
/// | `fetch.wait.max.ms` | 10 ms | 500 ms | Return quickly |
/// | `queued.min.messages` | 1000 | 100000 | Smaller pre-fetch queue |
/// | `enable.auto.commit` | false | true | DFE manages commits |
/// | `reconnect.backoff.ms` | 10 ms | 100 ms | Fast reconnect |
/// | `reconnect.backoff.max.ms` | 100 ms | 10000 ms | Cap quickly |
/// | `statistics.interval.ms` | 1000 ms | 0 | Enable metrics |
pub const LOW_LATENCY_CONSUMER_DEFAULTS: &[(&str, &str)] = &[
    ("partition.assignment.strategy", "cooperative-sticky"),
    ("fetch.wait.max.ms", "10"),
    ("queued.min.messages", "1000"),
    ("enable.auto.commit", "false"),
    ("reconnect.backoff.ms", "10"),
    ("reconnect.backoff.max.ms", "100"),
    ("statistics.interval.ms", "1000"),
];

// ============================================================================
// Configuration Struct
// ============================================================================

/// Kafka transport configuration.
///
/// Uses a profile-based system where profiles provide opinionated defaults,
/// and `librdkafka_overrides` allows overriding any setting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KafkaConfig {
    /// Configuration profile (production, devtest).
    ///
    /// The profile provides baseline librdkafka settings optimized for the use case.
    /// Use `librdkafka_overrides` to customize specific settings.
    #[serde(default)]
    pub profile: KafkaProfile,

    /// Kafka broker addresses.
    #[serde(default = "default_brokers")]
    pub brokers: Vec<String>,

    /// Consumer group ID.
    #[serde(default = "default_group")]
    pub group: String,

    /// Client ID for identification in broker logs.
    #[serde(default = "default_client_id")]
    pub client_id: String,

    /// Topics to subscribe to.
    #[serde(default)]
    pub topics: Vec<String>,

    /// Enable auto-discovery when `topics` is empty.
    /// When false (default), empty `topics` means no subscription (producer-only).
    /// When true, empty `topics` triggers broker auto-discovery with
    /// `topic_include`/`topic_exclude` filters and suppression rules.
    #[serde(default)]
    pub auto_discover: bool,

    /// Regex patterns for topic include filtering (empty = accept all).
    /// Topics must match at least one pattern (OR logic).
    #[serde(default)]
    pub topic_include: Vec<String>,

    /// Regex patterns for topic exclude filtering.
    /// Topics matching any pattern are excluded. Exclude wins over include.
    /// Default: `["^__"]` (excludes Kafka internal topics like `__consumer_offsets`).
    #[serde(default = "default_topic_exclude")]
    pub topic_exclude: Vec<String>,

    /// Periodic topic refresh interval in seconds (0 = disabled).
    /// Only applies when `topics` is empty (auto-discovery mode).
    #[serde(default = "default_topic_refresh_secs")]
    pub topic_refresh_secs: u64,

    /// Suppression rules: if a topic with preferred_suffix exists,
    /// suppress the topic with suppressed_suffix for the same base name.
    /// Default: _load suppresses _land (DFE convention).
    #[serde(default = "default_topic_suppression_rules")]
    pub topic_suppression_rules: Vec<SuppressionRule>,

    /// Security protocol (plaintext, ssl, sasl_plaintext, sasl_ssl).
    #[serde(default = "default_security_protocol")]
    pub security_protocol: String,

    /// SASL mechanism (PLAIN, SCRAM-SHA-256, SCRAM-SHA-512, OAUTHBEARER).
    #[serde(default)]
    pub sasl_mechanism: Option<String>,

    /// SASL username.
    #[serde(default)]
    pub sasl_username: Option<String>,

    /// SASL password.
    #[serde(default)]
    pub sasl_password: Option<crate::SensitiveString>,

    // --- TLS Configuration ---
    /// SSL CA certificate file path.
    #[serde(default)]
    pub ssl_ca_location: Option<String>,

    /// SSL client certificate file path.
    #[serde(default)]
    pub ssl_certificate_location: Option<String>,

    /// SSL client key file path.
    #[serde(default)]
    pub ssl_key_location: Option<String>,

    /// Skip SSL certificate verification.
    ///
    /// Automatically enabled for `devtest` profile.
    #[serde(default)]
    pub ssl_skip_verify: bool,

    // --- Consumer Settings (explicit fields for common options) ---
    /// Enable auto-commit (default: false for manual commit).
    #[serde(default)]
    pub enable_auto_commit: bool,

    /// Auto-commit interval in milliseconds.
    #[serde(default = "default_auto_commit_interval")]
    pub auto_commit_interval_ms: u32,

    /// Session timeout in milliseconds.
    #[serde(default = "default_session_timeout")]
    pub session_timeout_ms: u32,

    /// Heartbeat interval in milliseconds.
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval_ms: u32,

    /// Maximum poll interval in milliseconds.
    #[serde(default = "default_max_poll_interval")]
    pub max_poll_interval_ms: u32,

    /// Fetch minimum bytes.
    #[serde(default = "default_fetch_min_bytes")]
    pub fetch_min_bytes: i32,

    /// Fetch maximum bytes.
    #[serde(default = "default_fetch_max_bytes")]
    pub fetch_max_bytes: i32,

    /// Maximum messages per partition per poll.
    #[serde(default = "default_max_partition_fetch_bytes")]
    pub max_partition_fetch_bytes: i32,

    /// Auto offset reset (earliest, latest, none).
    #[serde(default = "default_auto_offset_reset")]
    pub auto_offset_reset: String,

    /// Enable partition EOF events.
    #[serde(default)]
    pub enable_partition_eof: bool,

    /// Librdkafka configuration overrides.
    ///
    /// These settings override both the profile defaults and explicit config fields.
    /// Use this to customize any librdkafka setting not exposed as an explicit field.
    ///
    /// Example:
    /// ```yaml
    /// librdkafka_overrides:
    ///   statistics.interval.ms: "5000"
    ///   fetch.min.bytes: "2097152"
    /// ```
    #[serde(default)]
    pub librdkafka_overrides: HashMap<String, String>,

    /// Legacy field - use `librdkafka_overrides` instead.
    #[serde(default)]
    #[deprecated(since = "1.3.0", note = "Use `librdkafka_overrides` instead")]
    pub extra_config: HashMap<String, String>,
}

fn default_topic_exclude() -> Vec<String> {
    vec!["^__".to_string()]
}

fn default_topic_refresh_secs() -> u64 {
    60
}

fn default_topic_suppression_rules() -> Vec<SuppressionRule> {
    vec![SuppressionRule {
        preferred_suffix: "_load".into(),
        suppressed_suffix: "_land".into(),
    }]
}

fn default_brokers() -> Vec<String> {
    vec!["localhost:9092".to_string()]
}

fn default_group() -> String {
    "hyperi-rustlib-consumer".to_string()
}

fn default_client_id() -> String {
    "hyperi-rustlib".to_string()
}

fn default_security_protocol() -> String {
    "plaintext".to_string()
}

fn default_auto_commit_interval() -> u32 {
    5000
}

fn default_session_timeout() -> u32 {
    45000
}

fn default_heartbeat_interval() -> u32 {
    3000
}

fn default_max_poll_interval() -> u32 {
    300_000
}

fn default_fetch_min_bytes() -> i32 {
    1
}

fn default_fetch_max_bytes() -> i32 {
    52_428_800 // 50 MB
}

fn default_max_partition_fetch_bytes() -> i32 {
    1_048_576 // 1 MB
}

fn default_auto_offset_reset() -> String {
    "earliest".to_string()
}

impl Default for KafkaConfig {
    fn default() -> Self {
        #[allow(deprecated)]
        Self {
            profile: KafkaProfile::default(),
            brokers: default_brokers(),
            group: default_group(),
            client_id: default_client_id(),
            topics: Vec::new(),
            auto_discover: false,
            topic_include: Vec::new(),
            topic_exclude: default_topic_exclude(),
            topic_refresh_secs: default_topic_refresh_secs(),
            topic_suppression_rules: default_topic_suppression_rules(),
            security_protocol: default_security_protocol(),
            sasl_mechanism: None,
            sasl_username: None,
            sasl_password: None,
            ssl_ca_location: None,
            ssl_certificate_location: None,
            ssl_key_location: None,
            ssl_skip_verify: false,
            enable_auto_commit: false,
            auto_commit_interval_ms: default_auto_commit_interval(),
            session_timeout_ms: default_session_timeout(),
            heartbeat_interval_ms: default_heartbeat_interval(),
            max_poll_interval_ms: default_max_poll_interval(),
            fetch_min_bytes: default_fetch_min_bytes(),
            fetch_max_bytes: default_fetch_max_bytes(),
            max_partition_fetch_bytes: default_max_partition_fetch_bytes(),
            auto_offset_reset: default_auto_offset_reset(),
            enable_partition_eof: false,
            librdkafka_overrides: HashMap::new(),
            extra_config: HashMap::new(),
        }
    }
}

impl KafkaConfig {
    /// Create a config with the production profile.
    #[must_use]
    pub fn production() -> Self {
        Self {
            profile: KafkaProfile::Production,
            ..Default::default()
        }
    }

    /// Create a config with the devtest profile.
    ///
    /// Automatically enables SSL skip verify.
    #[must_use]
    pub fn devtest() -> Self {
        Self {
            profile: KafkaProfile::DevTest,
            ssl_skip_verify: true,
            ..Default::default()
        }
    }

    /// Create a minimal config for testing.
    #[must_use]
    pub fn for_testing(brokers: &str, group: &str, topics: Vec<String>) -> Self {
        Self {
            profile: KafkaProfile::DevTest,
            brokers: vec![brokers.to_string()],
            group: group.to_string(),
            topics,
            ssl_skip_verify: true,
            ..Default::default()
        }
    }

    /// Set the configuration profile.
    #[must_use]
    pub fn with_profile(mut self, profile: KafkaProfile) -> Self {
        self.profile = profile;
        if profile == KafkaProfile::DevTest {
            self.ssl_skip_verify = true;
        }
        self
    }

    /// Get the profile's librdkafka defaults.
    #[must_use]
    pub fn profile_defaults(&self) -> &'static [(&'static str, &'static str)] {
        match self.profile {
            KafkaProfile::Production => PRODUCTION_PROFILE,
            KafkaProfile::DevTest => DEVTEST_PROFILE,
        }
    }

    /// Build the final librdkafka config map.
    ///
    /// Order of precedence (lowest to highest):
    /// 1. Profile defaults
    /// 2. Explicit config fields (fetch_min_bytes, etc.)
    /// 3. `extra_config` (legacy, deprecated)
    /// 4. `librdkafka_overrides` (highest priority)
    #[must_use]
    #[allow(deprecated)]
    pub fn build_librdkafka_config(&self) -> HashMap<String, String> {
        let mut config = HashMap::new();

        // 1. Apply profile defaults
        for (key, value) in self.profile_defaults() {
            config.insert((*key).to_string(), (*value).to_string());
        }

        // 2. Explicit config fields override profile
        // (These are only applied if they differ from the "default" to avoid
        // overriding profile settings unintentionally)
        // Note: In practice, the transport layer applies these directly

        // 3. Legacy extra_config
        for (key, value) in &self.extra_config {
            config.insert(key.clone(), value.clone());
        }

        // 4. librdkafka_overrides (highest priority)
        for (key, value) in &self.librdkafka_overrides {
            config.insert(key.clone(), value.clone());
        }

        config
    }

    /// Add a librdkafka override.
    ///
    /// This has the highest priority and will override profile defaults.
    #[must_use]
    pub fn with_override(mut self, key: &str, value: &str) -> Self {
        self.librdkafka_overrides
            .insert(key.to_string(), value.to_string());
        self
    }

    /// Add multiple librdkafka overrides.
    #[must_use]
    pub fn with_overrides(mut self, overrides: &[(&str, &str)]) -> Self {
        for (key, value) in overrides {
            self.librdkafka_overrides
                .insert((*key).to_string(), (*value).to_string());
        }
        self
    }

    // ========================================================================
    // Authentication Methods
    // ========================================================================

    /// Create a config with SASL/SCRAM authentication.
    #[must_use]
    pub fn with_scram(mut self, mechanism: &str, username: &str, password: &str) -> Self {
        self.security_protocol = "sasl_plaintext".to_string();
        self.sasl_mechanism = Some(mechanism.to_string());
        self.sasl_username = Some(username.to_string());
        self.sasl_password = Some(crate::SensitiveString::new(password));
        self
    }

    /// Create a config with SASL/SSL authentication.
    #[must_use]
    pub fn with_scram_ssl(mut self, mechanism: &str, username: &str, password: &str) -> Self {
        self.security_protocol = "sasl_ssl".to_string();
        self.sasl_mechanism = Some(mechanism.to_string());
        self.sasl_username = Some(username.to_string());
        self.sasl_password = Some(crate::SensitiveString::new(password));
        self
    }

    /// Add TLS configuration.
    #[must_use]
    pub fn with_tls(mut self, ca_location: Option<&str>) -> Self {
        if self.security_protocol == "plaintext" {
            self.security_protocol = "ssl".to_string();
        } else if self.security_protocol == "sasl_plaintext" {
            self.security_protocol = "sasl_ssl".to_string();
        }
        self.ssl_ca_location = ca_location.map(String::from);
        self
    }

    /// Add client certificate for mutual TLS.
    #[must_use]
    pub fn with_client_cert(mut self, cert_location: &str, key_location: &str) -> Self {
        self.ssl_certificate_location = Some(cert_location.to_string());
        self.ssl_key_location = Some(key_location.to_string());
        self
    }

    /// Skip SSL certificate verification.
    ///
    /// **WARNING**: Only use in development/test environments!
    #[must_use]
    pub fn with_ssl_skip_verify(mut self) -> Self {
        self.ssl_skip_verify = true;
        self
    }

    /// Enable SSL but accept any certificate (for dev/test with self-signed certs).
    #[must_use]
    pub fn with_ssl_insecure(mut self) -> Self {
        if self.security_protocol == "plaintext" {
            self.security_protocol = "ssl".to_string();
        } else if self.security_protocol == "sasl_plaintext" {
            self.security_protocol = "sasl_ssl".to_string();
        }
        self.ssl_skip_verify = true;
        self
    }

    // ========================================================================
    // Convenience Methods (apply common patterns as overrides)
    // ========================================================================

    /// Apply producer defaults.
    #[must_use]
    #[deprecated(since = "2.0.0", note = "Use producer profile constants directly")]
    #[allow(deprecated)]
    pub fn with_producer_defaults(mut self) -> Self {
        for (key, value) in PRODUCER_HIGH_THROUGHPUT {
            self.extra_config
                .entry((*key).to_string())
                .or_insert_with(|| (*value).to_string());
        }
        self
    }

    /// Apply high-throughput consumer defaults.
    #[must_use]
    #[deprecated(since = "2.0.0", note = "Use KafkaConfig::production() instead")]
    #[allow(deprecated)]
    pub fn with_high_throughput(mut self) -> Self {
        for (key, value) in PRODUCTION_PROFILE {
            self.extra_config
                .entry((*key).to_string())
                .or_insert_with(|| (*value).to_string());
        }
        self
    }

    /// Apply low-latency consumer defaults.
    #[must_use]
    #[deprecated(since = "2.0.0", note = "Use LOW_LATENCY_CONSUMER_DEFAULTS directly")]
    #[allow(deprecated)]
    pub fn with_low_latency(mut self) -> Self {
        for (key, value) in LOW_LATENCY_CONSUMER_DEFAULTS {
            self.extra_config
                .entry((*key).to_string())
                .or_insert_with(|| (*value).to_string());
        }
        self
    }

    /// Enable statistics collection at the specified interval.
    #[must_use]
    pub fn with_statistics(mut self, interval_ms: u32) -> Self {
        self.librdkafka_overrides.insert(
            "statistics.interval.ms".to_string(),
            interval_ms.to_string(),
        );
        self
    }

    /// Apply cloud-optimized connection settings.
    #[must_use]
    pub fn with_cloud_connection_tuning(mut self) -> Self {
        let cloud_settings = [
            ("socket.keepalive.enable", "true"),
            ("metadata.max.age.ms", "180000"),
            ("socket.connection.setup.timeout.ms", "30000"),
            ("connections.max.idle.ms", "540000"),
        ];
        for (key, value) in cloud_settings {
            self.librdkafka_overrides
                .entry(key.to_string())
                .or_insert_with(|| value.to_string());
        }
        self
    }

    // ========================================================================
    // Environment Loading
    // ========================================================================

    /// Load configuration from environment variables with prefix.
    ///
    /// Reads environment variables with the given prefix:
    /// - `{PREFIX}_PROFILE` -> profile (production, devtest)
    /// - `{PREFIX}_BOOTSTRAP_SERVERS` -> brokers (legacy: `{PREFIX}_BROKERS`)
    /// - `{PREFIX}_GROUP_ID` -> group
    /// - `{PREFIX}_SECURITY_PROTOCOL` -> security_protocol
    /// - `{PREFIX}_SASL_MECHANISM` -> sasl_mechanism
    /// - `{PREFIX}_SASL_USERNAME` -> sasl_username (legacy: `{PREFIX}_SASL_USER`)
    /// - `{PREFIX}_SASL_PASSWORD` -> sasl_password
    /// - `{PREFIX}_SSL_SKIP_VERIFY` -> ssl_skip_verify
    /// - `{PREFIX}_TOPICS` -> topics (comma-separated)
    ///
    /// Also supports standard `KAFKA_*` environment variables as fallback
    /// when using a custom prefix.
    #[must_use]
    pub fn from_env(prefix: &str) -> Self {
        use crate::config::env_compat::EnvVar;

        let mut config = Self::default();

        // Helper to create prefixed env var with legacy support
        let prefixed = |name: &str, legacy: &[&str]| {
            let mut var = EnvVar::new(&format!("{prefix}_{name}"));
            for l in legacy {
                var = var.with_legacy(&format!("{prefix}_{l}"));
            }
            // Also check KAFKA_* as fallback
            var = var.with_legacy(&format!("KAFKA_{name}"));
            var
        };

        // Profile
        if let Some(val) = prefixed("PROFILE", &[]).get()
            && let Ok(profile) = val.parse()
        {
            config.profile = profile;
            if config.profile == KafkaProfile::DevTest {
                config.ssl_skip_verify = true;
            }
        }

        // Bootstrap servers (with BROKERS as legacy)
        if let Some(brokers) = prefixed("BOOTSTRAP_SERVERS", &["BROKERS"]).get_list() {
            config.brokers = brokers;
        }

        // Group ID
        if let Some(val) = prefixed("GROUP_ID", &["GROUP", "CONSUMER_GROUP"]).get() {
            config.group = val;
        }

        // Client ID
        if let Some(val) = prefixed("CLIENT_ID", &[]).get() {
            config.client_id = val;
        }

        // Security protocol
        if let Some(val) = prefixed("SECURITY_PROTOCOL", &[]).get() {
            config.security_protocol = val;
        }

        // SASL mechanism
        if let Some(val) = prefixed("SASL_MECHANISM", &[]).get() {
            config.sasl_mechanism = Some(val);
        }

        // SASL username (with SASL_USER as legacy)
        if let Some(val) = prefixed("SASL_USERNAME", &["SASL_USER"]).get() {
            config.sasl_username = Some(val);
        }

        // SASL password
        if let Some(val) = prefixed("SASL_PASSWORD", &[]).get() {
            config.sasl_password = Some(crate::SensitiveString::from(val));
        }

        // SSL CA location
        if let Some(val) = prefixed("SSL_CA_LOCATION", &["CA_CERT", "SSL_CA"]).get() {
            config.ssl_ca_location = Some(val);
        }

        // SSL skip verify
        if let Some(val) = prefixed("SSL_SKIP_VERIFY", &["SSL_INSECURE", "INSECURE"]).get_bool() {
            config.ssl_skip_verify = val;
        }

        // Topics
        if let Some(topics) = prefixed("TOPICS", &["TOPIC"]).get_list() {
            config.topics = topics;
        }

        config
    }

    /// Load configuration from standard `KAFKA_*` environment variables.
    ///
    /// This is a convenience method that uses the standard Kafka prefix.
    /// Supports legacy aliases with deprecation warnings.
    ///
    /// Standard variables:
    /// - `KAFKA_BOOTSTRAP_SERVERS` (legacy: `KAFKA_BROKERS`)
    /// - `KAFKA_SASL_USERNAME` (legacy: `KAFKA_SASL_USER`)
    /// - `KAFKA_SECURITY_PROTOCOL`
    /// - `KAFKA_SASL_MECHANISM`
    /// - `KAFKA_SASL_PASSWORD`
    /// - `KAFKA_SSL_SKIP_VERIFY`
    /// - `KAFKA_TOPICS`
    /// - `KAFKA_GROUP_ID`
    /// - `KAFKA_CLIENT_ID`
    /// - `KAFKA_PROFILE`
    #[must_use]
    pub fn from_env_standard() -> Self {
        Self::from_env("KAFKA")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kafka_config_topic_resolution_defaults() {
        let config = KafkaConfig::default();
        assert!(config.topic_include.is_empty());
        assert_eq!(config.topic_exclude, vec!["^__".to_string()]);
        assert!(!config.auto_discover);
        assert_eq!(config.topic_refresh_secs, 60);
        assert_eq!(config.topic_suppression_rules.len(), 1);
        assert_eq!(config.topic_suppression_rules[0].preferred_suffix, "_load");
        assert_eq!(config.topic_suppression_rules[0].suppressed_suffix, "_land");
    }
}
