// Project:   hs-rustlib
// File:      src/transport/kafka/config.rs
// Purpose:   Kafka transport configuration with profiles and config-driven overrides
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Kafka configuration with profile-based defaults and config-driven overrides.
//!
//! ## Profile System
//!
//! HyperSec Kafka uses a profile-based configuration system where:
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
// Profile System
// ============================================================================

/// Kafka configuration profile.
///
/// Profiles provide opinionated librdkafka defaults for specific use cases.
/// Users can override any setting via `librdkafka_overrides`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KafkaProfile {
    /// Production profile: High-throughput, PB/day workloads.
    ///
    /// Optimizations:
    /// - Large pre-fetch queues (100K messages, 1GB per partition)
    /// - Cooperative sticky rebalancing (minimal disruption)
    /// - Disabled CRC checks (trust network/TLS)
    /// - Optimized fetch parameters (1MB min, 10MB max per partition)
    /// - 1MB socket buffers
    #[default]
    Production,

    /// Development/Test profile: Relaxed settings for local development.
    ///
    /// Features:
    /// - Smaller queues (lower memory usage)
    /// - SSL certificate verification disabled by default
    /// - Faster reconnection for quick iteration
    /// - Debug-friendly log settings
    /// - Cooperative rebalancing still enabled
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
// Profile Defaults
// ============================================================================

/// Production profile librdkafka settings.
///
/// Optimized for PB/day batch workloads with maximum throughput.
pub const PRODUCTION_PROFILE: &[(&str, &str)] = &[
    // --- Pre-fetch queue tuning (large queues for continuous data flow) ---
    ("queued.min.messages", "100000"), // 100K messages per partition
    ("queued.max.messages.kbytes", "1048576"), // 1GB max queue size
    // --- Fetch tuning (batch larger requests, fewer round-trips) ---
    ("fetch.wait.max.ms", "100"),   // Max wait for fetch response
    ("fetch.min.bytes", "1048576"), // 1MB minimum fetch
    ("fetch.message.max.bytes", "10485760"), // 10MB max message size
    ("max.partition.fetch.bytes", "10485760"), // 10MB per partition fetch
    ("receive.message.max.bytes", "104857600"), // 100MB max response size
    // --- Socket tuning ---
    ("socket.receive.buffer.bytes", "1048576"), // 1MB socket buffer
    ("socket.nagle.disable", "true"),           // Disable Nagle for lower latency
    ("socket.keepalive.enable", "true"),        // Keep connections alive
    // --- Rebalancing optimization ---
    ("partition.assignment.strategy", "cooperative-sticky"), // Incremental rebalancing
    // --- Disable unnecessary overhead ---
    ("check.crcs", "false"),           // Skip CRC verification (trust TLS)
    ("enable.partition.eof", "false"), // Don't emit EOF events
    ("fetch.error.backoff.ms", "100"), // Fast retry on fetch errors
    // --- Connection tuning ---
    ("reconnect.backoff.ms", "50"),        // Fast initial reconnect
    ("reconnect.backoff.max.ms", "1000"),  // Cap reconnect backoff at 1s
    ("connections.max.idle.ms", "540000"), // 9 minutes idle timeout
    ("metadata.max.age.ms", "180000"),     // Refresh metadata every 3 minutes
];

/// Development/Test profile librdkafka settings.
///
/// Relaxed settings for local development and testing environments.
pub const DEVTEST_PROFILE: &[(&str, &str)] = &[
    // --- Smaller queues (lower memory for dev machines) ---
    ("queued.min.messages", "1000"), // 1K messages per partition
    ("queued.max.messages.kbytes", "65536"), // 64MB max queue size
    // --- Standard fetch (no aggressive batching) ---
    ("fetch.wait.max.ms", "500"),             // Standard wait
    ("fetch.min.bytes", "1"),                 // Return immediately with any data
    ("fetch.message.max.bytes", "1048576"),   // 1MB max message
    ("max.partition.fetch.bytes", "1048576"), // 1MB per partition
    // --- Socket tuning ---
    ("socket.nagle.disable", "true"), // Still disable Nagle
    // --- Rebalancing (cooperative for all environments) ---
    ("partition.assignment.strategy", "cooperative-sticky"),
    // --- Keep CRC checks in dev for safety ---
    ("check.crcs", "true"),            // Verify message integrity
    ("enable.partition.eof", "false"), // Don't emit EOF events
    // --- Fast reconnection for quick iteration ---
    ("reconnect.backoff.ms", "10"),      // Very fast reconnect
    ("reconnect.backoff.max.ms", "100"), // Cap quickly
    ("fetch.error.backoff.ms", "50"),    // Fast retry
    // --- Debug-friendly ---
    ("log.connection.close", "true"), // Log connection closes
];

// ============================================================================
// Producer Profile Defaults
// ============================================================================

/// High-throughput producer settings for PB/day workloads.
///
/// Optimized for maximum throughput with at-least-once delivery:
/// - Large batches (256KB) with 100ms linger for batch accumulation
/// - 1GB producer queue (1M messages)
/// - LZ4 compression (best throughput/ratio tradeoff)
/// - High in-flight requests for parallel sends
/// - Sticky partitioner for better batching
pub const PRODUCER_HIGH_THROUGHPUT: &[(&str, &str)] = &[
    // --- Delivery guarantees (at-least-once) ---
    ("acks", "all"),                 // Wait for all replicas
    ("enable.idempotence", "false"), // Disabled for max throughput
    // --- Batching (maximize batch size) ---
    ("linger.ms", "100"),            // 100ms to accumulate batches
    ("batch.size", "262144"),        // 256KB batch size
    ("batch.num.messages", "10000"), // Max 10K messages per batch
    // --- Queue sizing (1GB buffer) ---
    ("queue.buffering.max.messages", "1000000"), // 1M messages
    ("queue.buffering.max.kbytes", "1048576"),   // 1GB max queue
    ("queue.buffering.max.ms", "100"),           // Alias for linger.ms
    // --- Compression (LZ4 = best throughput) ---
    ("compression.type", "lz4"), // Fast compression
    ("compression.level", "1"),  // Fastest compression level
    // --- Parallelism (high in-flight for throughput) ---
    ("max.in.flight.requests.per.connection", "10"), // Parallel sends (ordering not guaranteed)
    // --- Timeouts ---
    ("delivery.timeout.ms", "120000"), // 2 minutes max delivery
    ("request.timeout.ms", "30000"),   // 30s per request
    ("message.timeout.ms", "120000"),  // Match delivery timeout
    // --- Retries ---
    ("retries", "5"),                 // Retry transient failures
    ("retry.backoff.ms", "100"),      // 100ms backoff
    ("retry.backoff.max.ms", "1000"), // Cap at 1s
    // --- Socket tuning ---
    ("socket.send.buffer.bytes", "1048576"), // 1MB send buffer
    ("socket.nagle.disable", "true"),        // Disable Nagle
    ("socket.keepalive.enable", "true"),     // Keep connections alive
    // --- Partitioner ---
    ("partitioner", "consistent_random"), // Sticky-like for no-key messages
];

/// Producer settings optimized for exactly-once semantics.
///
/// Use when you need strong ordering guarantees and no duplicates:
/// - Idempotence enabled (exactly-once within partition)
/// - Limited in-flight requests (ordering preserved)
/// - Smaller batches for lower latency
pub const PRODUCER_EXACTLY_ONCE: &[(&str, &str)] = &[
    // --- Delivery guarantees (exactly-once) ---
    ("acks", "all"),                // Wait for all replicas
    ("enable.idempotence", "true"), // Exactly-once semantics
    // --- Ordering (limited in-flight) ---
    ("max.in.flight.requests.per.connection", "5"), // Max for idempotence
    // --- Batching (moderate) ---
    ("linger.ms", "20"),     // Smaller linger for latency
    ("batch.size", "65536"), // 64KB batch
    // --- Queue sizing ---
    ("queue.buffering.max.messages", "100000"), // 100K messages
    ("queue.buffering.max.kbytes", "262144"),   // 256MB max queue
    // --- Compression ---
    ("compression.type", "lz4"),
    // --- Timeouts ---
    ("delivery.timeout.ms", "120000"),
    ("request.timeout.ms", "30000"),
    // --- Retries (high for idempotence) ---
    ("retries", "2147483647"), // Infinite retries (limited by timeout)
    ("retry.backoff.ms", "100"),
    // --- Socket tuning ---
    ("socket.send.buffer.bytes", "262144"), // 256KB send buffer
    ("socket.nagle.disable", "true"),
];

/// Low-latency producer settings for real-time use cases.
///
/// Optimized for minimal end-to-end latency:
/// - No batching (immediate send)
/// - Small buffers
/// - Fast acknowledgment (acks=1)
pub const PRODUCER_LOW_LATENCY: &[(&str, &str)] = &[
    // --- Delivery guarantees (faster acks) ---
    ("acks", "1"), // Leader ack only for speed
    ("enable.idempotence", "false"),
    // --- No batching ---
    ("linger.ms", "0"),      // Send immediately
    ("batch.size", "16384"), // Small batch (16KB)
    // --- Queue sizing (smaller) ---
    ("queue.buffering.max.messages", "10000"),
    ("queue.buffering.max.kbytes", "65536"), // 64MB
    // --- Compression (optional, can disable) ---
    ("compression.type", "lz4"), // Still use LZ4 (fast)
    // --- Parallelism ---
    ("max.in.flight.requests.per.connection", "5"),
    // --- Timeouts (shorter) ---
    ("delivery.timeout.ms", "30000"), // 30s max
    ("request.timeout.ms", "10000"),  // 10s per request
    // --- Retries ---
    ("retries", "3"),
    ("retry.backoff.ms", "50"),
    // --- Socket ---
    ("socket.nagle.disable", "true"),
];

/// DevTest producer settings.
///
/// Relaxed settings for local development.
pub const PRODUCER_DEVTEST: &[(&str, &str)] = &[
    ("acks", "1"), // Faster for dev
    ("enable.idempotence", "false"),
    ("linger.ms", "10"),
    ("batch.size", "32768"), // 32KB
    ("queue.buffering.max.messages", "10000"),
    ("queue.buffering.max.kbytes", "65536"),
    ("compression.type", "none"), // No compression in dev
    ("max.in.flight.requests.per.connection", "5"),
    ("delivery.timeout.ms", "30000"),
    ("request.timeout.ms", "10000"),
    ("retries", "3"),
    ("retry.backoff.ms", "50"),
    ("socket.nagle.disable", "true"),
];

/// Legacy producer defaults (backward compatibility).
pub const PRODUCER_DEFAULTS: &[(&str, &str)] = &[
    ("acks", "all"),
    ("retries", "5"),
    ("retry.backoff.ms", "100"),
    ("delivery.timeout.ms", "120000"),
    ("request.timeout.ms", "30000"),
    ("linger.ms", "50"),
    ("compression.type", "lz4"),
    ("batch.size", "65536"),
    ("queue.buffering.max.messages", "100000"),
];

// ============================================================================
// Legacy Constants (for backward compatibility)
// ============================================================================

/// Alias for `PRODUCTION_PROFILE` (backward compatibility).
pub const HIGH_THROUGHPUT_CONSUMER_DEFAULTS: &[(&str, &str)] = PRODUCTION_PROFILE;

/// Low-latency consumer defaults (for real-time workloads).
///
/// Use this for real-time alerting, trading signals, etc.
pub const LOW_LATENCY_CONSUMER_DEFAULTS: &[(&str, &str)] = &[
    ("fetch.wait.max.ms", "10"),             // Return quickly
    ("fetch.min.bytes", "1"),                // No batching wait
    ("reconnect.backoff.ms", "10"),          // Very fast reconnect
    ("reconnect.backoff.max.ms", "100"),     // Cap backoff quickly
    ("socket.nagle.disable", "true"),        // Disable Nagle
    ("queued.min.messages", "1000"),         // Smaller queue
    ("queued.max.messages.kbytes", "65536"), // 64MB max queue
    ("partition.assignment.strategy", "cooperative-sticky"),
    ("fetch.error.backoff.ms", "10"), // Very fast retry
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
    pub sasl_password: Option<String>,

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

fn default_brokers() -> Vec<String> {
    vec!["localhost:9092".to_string()]
}

fn default_group() -> String {
    "hs-rustlib-consumer".to_string()
}

fn default_client_id() -> String {
    "hs-rustlib".to_string()
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
        self.sasl_password = Some(password.to_string());
        self
    }

    /// Create a config with SASL/SSL authentication.
    #[must_use]
    pub fn with_scram_ssl(mut self, mechanism: &str, username: &str, password: &str) -> Self {
        self.security_protocol = "sasl_ssl".to_string();
        self.sasl_mechanism = Some(mechanism.to_string());
        self.sasl_username = Some(username.to_string());
        self.sasl_password = Some(password.to_string());
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
    #[allow(deprecated)]
    pub fn with_producer_defaults(mut self) -> Self {
        for (key, value) in PRODUCER_DEFAULTS {
            self.extra_config
                .entry((*key).to_string())
                .or_insert_with(|| (*value).to_string());
        }
        self
    }

    /// Apply high-throughput consumer defaults.
    ///
    /// **Deprecated**: Use `KafkaConfig::production()` or `.with_profile(KafkaProfile::Production)` instead.
    #[must_use]
    #[allow(deprecated)]
    pub fn with_high_throughput(mut self) -> Self {
        for (key, value) in HIGH_THROUGHPUT_CONSUMER_DEFAULTS {
            self.extra_config
                .entry((*key).to_string())
                .or_insert_with(|| (*value).to_string());
        }
        self
    }

    /// Apply low-latency consumer defaults.
    #[must_use]
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
        if let Some(val) = prefixed("PROFILE", &[]).get() {
            if let Ok(profile) = val.parse() {
                config.profile = profile;
                if config.profile == KafkaProfile::DevTest {
                    config.ssl_skip_verify = true;
                }
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
            config.sasl_password = Some(val);
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
