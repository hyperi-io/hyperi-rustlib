// Project:   hs-rustlib
// File:      src/transport/kafka/config.rs
// Purpose:   Kafka transport configuration with corporate defaults
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Kafka configuration with HyperSec defaults.
//!
//! Defaults match the Python `hs_pylib.kafka` module for consistency across
//! all HyperSec projects. Uses librdkafka configuration names directly.
//!
//! ## HyperSec Defaults
//!
//! **Consumer:**
//! - `auto.offset.reset`: earliest (start from beginning if no offset)
//! - `enable.auto.commit`: false (manual commit for at-least-once)
//! - `session.timeout.ms`: 45000 (45 seconds)
//! - `heartbeat.interval.ms`: 3000 (3 seconds)
//! - `max.poll.interval.ms`: 300000 (5 minutes)
//! - `fetch.min.bytes`: 1 (return immediately with any data)
//!
//! **Producer (via extra_config, at-least-once delivery):**
//! - `acks`: all (wait for all replicas)
//! - `retries`: 5
//! - `batch.size`: 10000 (10K batch size)
//! - `compression.type`: lz4 (fast compression)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Kafka transport configuration.
///
/// Follows HyperSec corporate defaults matching the Python kafkaplus module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KafkaConfig {
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

    /// Skip SSL certificate verification (dev/test only!).
    #[serde(default)]
    pub ssl_skip_verify: bool,

    // --- Consumer Settings ---
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

    /// Additional librdkafka config options.
    /// Use this for advanced settings not covered by explicit fields.
    #[serde(default)]
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
    300000
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
        Self {
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
            extra_config: HashMap::new(),
        }
    }
}

/// HyperSec producer defaults (applied via `with_producer_defaults()`).
///
/// These match the Python `hs_pylib.kafka.config.PRODUCER_DEFAULTS`.
/// Configured for **at-least-once** delivery (not exactly-once).
pub const PRODUCER_DEFAULTS: &[(&str, &str)] = &[
    ("acks", "all"),                    // Wait for all replicas (at-least-once)
    ("retries", "5"),                   // Retry on transient failures
    ("retry.backoff.ms", "100"),        // Backoff between retries
    ("delivery.timeout.ms", "120000"),  // 2 minutes max delivery time
    ("request.timeout.ms", "30000"),    // 30 seconds per request
    ("linger.ms", "5"),                 // Small delay for batching
    ("compression.type", "lz4"),        // Fast compression
    ("batch.size", "10000"),            // 10K batch size (HyperSec default)
];

impl KafkaConfig {
    /// Create a minimal config for testing.
    #[must_use]
    pub fn for_testing(brokers: &str, group: &str, topics: Vec<String>) -> Self {
        Self {
            brokers: vec![brokers.to_string()],
            group: group.to_string(),
            topics,
            ..Default::default()
        }
    }

    /// Create a config with SASL/SCRAM authentication.
    #[must_use]
    pub fn with_scram(
        mut self,
        mechanism: &str,
        username: &str,
        password: &str,
    ) -> Self {
        self.security_protocol = "sasl_plaintext".to_string();
        self.sasl_mechanism = Some(mechanism.to_string());
        self.sasl_username = Some(username.to_string());
        self.sasl_password = Some(password.to_string());
        self
    }

    /// Create a config with SASL/SSL authentication.
    #[must_use]
    pub fn with_scram_ssl(
        mut self,
        mechanism: &str,
        username: &str,
        password: &str,
    ) -> Self {
        self.security_protocol = "sasl_ssl".to_string();
        self.sasl_mechanism = Some(mechanism.to_string());
        self.sasl_username = Some(username.to_string());
        self.sasl_password = Some(password.to_string());
        self
    }

    /// Add TLS configuration.
    #[must_use]
    pub fn with_tls(mut self, ca_location: Option<&str>) -> Self {
        // Upgrade security protocol to SSL variant
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

    /// Apply corporate producer defaults to extra_config.
    ///
    /// Call this when using the transport for producing messages.
    #[must_use]
    pub fn with_producer_defaults(mut self) -> Self {
        for (key, value) in PRODUCER_DEFAULTS {
            self.extra_config
                .entry((*key).to_string())
                .or_insert_with(|| (*value).to_string());
        }
        self
    }

    /// Load configuration from environment variables.
    ///
    /// Reads environment variables with the given prefix:
    /// - `{PREFIX}_BOOTSTRAP_SERVERS` -> brokers
    /// - `{PREFIX}_GROUP_ID` -> group
    /// - `{PREFIX}_SECURITY_PROTOCOL` -> security_protocol
    /// - `{PREFIX}_SASL_MECHANISM` -> sasl_mechanism
    /// - `{PREFIX}_SASL_USERNAME` -> sasl_username
    /// - `{PREFIX}_SASL_PASSWORD` -> sasl_password
    #[must_use]
    pub fn from_env(prefix: &str) -> Self {
        let mut config = Self::default();

        if let Ok(val) = std::env::var(format!("{prefix}_BOOTSTRAP_SERVERS")) {
            config.brokers = val.split(',').map(|s| s.trim().to_string()).collect();
        }
        if let Ok(val) = std::env::var(format!("{prefix}_GROUP_ID")) {
            config.group = val;
        }
        if let Ok(val) = std::env::var(format!("{prefix}_CLIENT_ID")) {
            config.client_id = val;
        }
        if let Ok(val) = std::env::var(format!("{prefix}_SECURITY_PROTOCOL")) {
            config.security_protocol = val;
        }
        if let Ok(val) = std::env::var(format!("{prefix}_SASL_MECHANISM")) {
            config.sasl_mechanism = Some(val);
        }
        if let Ok(val) = std::env::var(format!("{prefix}_SASL_USERNAME")) {
            config.sasl_username = Some(val);
        }
        if let Ok(val) = std::env::var(format!("{prefix}_SASL_PASSWORD")) {
            config.sasl_password = Some(val);
        }
        if let Ok(val) = std::env::var(format!("{prefix}_SSL_CA_LOCATION")) {
            config.ssl_ca_location = Some(val);
        }

        config
    }
}
