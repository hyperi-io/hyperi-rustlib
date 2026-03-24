// Project:   hyperi-rustlib
// File:      src/http_server/config.rs
// Purpose:   HTTP server configuration
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! HTTP server configuration.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// HTTP server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct HttpServerConfig {
    /// Address to bind to (e.g., "0.0.0.0:8080").
    pub bind_address: String,

    /// Request timeout in milliseconds.
    /// Defaults to 30 seconds.
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,

    /// Keep-alive timeout in milliseconds.
    /// Defaults to 75 seconds.
    #[serde(default = "default_keep_alive_timeout_ms")]
    pub keep_alive_timeout_ms: u64,

    /// Maximum number of concurrent connections.
    /// Defaults to 10,000.
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,

    /// Whether to include health check endpoints (/health/live, /health/ready).
    /// Defaults to true.
    #[serde(default = "default_true")]
    pub enable_health_endpoints: bool,

    /// Whether to include metrics endpoint (/metrics).
    /// Defaults to false (use metrics module's server instead if needed).
    #[serde(default)]
    pub enable_metrics_endpoint: bool,

    /// Whether to include config registry endpoint (/config).
    /// Returns the redacted effective config from the registry.
    /// Defaults to false (opt-in for admin/debug use).
    #[serde(default)]
    pub enable_config_endpoint: bool,

    /// Enable HTTP/2 support (also required for gRPC).
    /// Defaults to true.
    #[serde(default = "default_true")]
    pub enable_http2: bool,

    /// TLS certificate path (PEM format).
    /// If set along with `tls_key_path`, TLS is enabled.
    #[serde(default)]
    pub tls_cert_path: Option<String>,

    /// TLS private key path (PEM format).
    #[serde(default)]
    pub tls_key_path: Option<String>,

    /// Graceful shutdown timeout in milliseconds.
    /// Defaults to 30 seconds.
    #[serde(default = "default_shutdown_timeout_ms")]
    pub shutdown_timeout_ms: u64,
}

fn default_request_timeout_ms() -> u64 {
    30_000
}

fn default_keep_alive_timeout_ms() -> u64 {
    75_000
}

fn default_max_connections() -> usize {
    10_000
}

fn default_shutdown_timeout_ms() -> u64 {
    30_000
}

fn default_true() -> bool {
    true
}

impl Default for HttpServerConfig {
    fn default() -> Self {
        Self {
            bind_address: "0.0.0.0:8080".to_string(),
            request_timeout_ms: default_request_timeout_ms(),
            keep_alive_timeout_ms: default_keep_alive_timeout_ms(),
            max_connections: default_max_connections(),
            enable_health_endpoints: true,
            enable_metrics_endpoint: false,
            enable_config_endpoint: false,
            enable_http2: true,
            tls_cert_path: None,
            tls_key_path: None,
            shutdown_timeout_ms: default_shutdown_timeout_ms(),
        }
    }
}

impl HttpServerConfig {
    /// Create a new config with the given bind address.
    #[must_use]
    pub fn new(bind_address: impl Into<String>) -> Self {
        Self {
            bind_address: bind_address.into(),
            ..Default::default()
        }
    }

    /// Get request timeout as Duration.
    #[must_use]
    pub fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.request_timeout_ms)
    }

    /// Get keep-alive timeout as Duration.
    #[must_use]
    pub fn keep_alive_timeout(&self) -> Duration {
        Duration::from_millis(self.keep_alive_timeout_ms)
    }

    /// Get shutdown timeout as Duration.
    #[must_use]
    pub fn shutdown_timeout(&self) -> Duration {
        Duration::from_millis(self.shutdown_timeout_ms)
    }

    /// Check if TLS is configured.
    #[must_use]
    pub fn is_tls_enabled(&self) -> bool {
        self.tls_cert_path.is_some() && self.tls_key_path.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HttpServerConfig::default();
        assert_eq!(config.bind_address, "0.0.0.0:8080");
        assert_eq!(config.request_timeout_ms, 30_000);
        assert_eq!(config.keep_alive_timeout_ms, 75_000);
        assert_eq!(config.max_connections, 10_000);
        assert!(config.enable_health_endpoints);
        assert!(!config.enable_metrics_endpoint);
        assert!(config.enable_http2);
        assert!(!config.is_tls_enabled());
    }

    #[test]
    fn test_new_with_address() {
        let config = HttpServerConfig::new("127.0.0.1:3000");
        assert_eq!(config.bind_address, "127.0.0.1:3000");
    }

    #[test]
    fn test_tls_enabled() {
        let mut config = HttpServerConfig::default();
        assert!(!config.is_tls_enabled());

        config.tls_cert_path = Some("/path/to/cert.pem".to_string());
        assert!(!config.is_tls_enabled());

        config.tls_key_path = Some("/path/to/key.pem".to_string());
        assert!(config.is_tls_enabled());
    }

    #[test]
    fn test_duration_conversions() {
        let config = HttpServerConfig::default();
        assert_eq!(config.request_timeout(), Duration::from_secs(30));
        assert_eq!(config.keep_alive_timeout(), Duration::from_secs(75));
        assert_eq!(config.shutdown_timeout(), Duration::from_secs(30));
    }
}
