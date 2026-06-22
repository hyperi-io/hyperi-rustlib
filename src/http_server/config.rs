// Project:   hyperi-rustlib
// File:      src/http_server/config.rs
// Purpose:   HTTP server configuration
// Language:  Rust
//
// License:   BUSL-1.1
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

    /// Keep-alive timeout in ms. Default 75 s.
    ///
    /// **Not wired.** axum 0.8 `serve()` doesn't surface keep-alive;
    /// needs hyper builder. Follow-up.
    #[serde(default = "default_keep_alive_timeout_ms")]
    pub keep_alive_timeout_ms: u64,

    /// In-flight request cap. Default 10,000. Enforced via
    /// `tower::limit::ConcurrencyLimitLayer`. Excess requests queue.
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,

    /// Mount /health/live + /health/ready. Default true.
    #[serde(default = "default_true")]
    pub enable_health_endpoints: bool,

    /// Mount /metrics.
    ///
    /// **Not wired here.** MetricsManager owns its own /metrics
    /// listener (often a separate admin port). Forward-compat for
    /// framework-managed wiring.
    #[serde(default)]
    pub enable_metrics_endpoint: bool,

    /// Mount /config (redacted effective config). Default false.
    #[serde(default)]
    pub enable_config_endpoint: bool,

    /// HTTP/2 (also required for gRPC).
    ///
    /// **Not wired.** axum 0.8 `serve()` always negotiates HTTP/1
    /// and HTTP/2 on cleartext. Disabling needs hyper builder.
    #[serde(default = "default_true")]
    pub enable_http2: bool,

    /// TLS cert path (PEM).
    ///
    /// **Not wired.** Needs `axum_server::tls_rustls` (not a dep).
    /// K8s pattern: terminate TLS at Ingress / Service Mesh,
    /// cleartext in-pod.
    #[serde(default)]
    pub tls_cert_path: Option<String>,

    /// TLS key path (PEM). See `tls_cert_path` -- not wired.
    #[serde(default)]
    pub tls_key_path: Option<String>,

    /// Graceful drain budget in ms. Default 30 s. Caps drain time
    /// to fit under K8s `terminationGracePeriodSeconds`.
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

    /// Whether TLS cert+key paths are present in config.
    ///
    /// NOTE: this only reports that the fields are *set* -- in-process TLS is
    /// NOT terminated by this server (see `tls_cert_path`). Use
    /// [`validate`](Self::validate) at startup so a config that *expects*
    /// in-pod TLS fails loudly instead of silently serving cleartext.
    #[must_use]
    pub fn is_tls_enabled(&self) -> bool {
        self.tls_cert_path.is_some() && self.tls_key_path.is_some()
    }

    /// Validate the server config (finding 10: `is_tls_enabled` must not lie).
    ///
    /// In-process TLS termination is not supported -- the K8s pattern is to
    /// terminate TLS at the ingress / service mesh and run cleartext in-pod.
    /// If `tls_cert_path`/`tls_key_path` are set, the operator expects in-pod
    /// TLS that will not happen, so this errors rather than letting the server
    /// bind cleartext while the config claims TLS. Call at startup.
    ///
    /// # Errors
    ///
    /// Returns `Err` if either TLS path is set.
    pub fn validate(&self) -> Result<(), String> {
        if self.tls_cert_path.is_some() || self.tls_key_path.is_some() {
            return Err(
                "http_server: in-process TLS is not supported (tls_cert_path / \
                 tls_key_path set) -- terminate TLS at the ingress / service mesh \
                 and leave these unset, or front the service with a TLS sidecar"
                    .to_string(),
            );
        }
        Ok(())
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
    fn validate_rejects_unsupported_in_process_tls() {
        // Default (no TLS paths) validates.
        assert!(HttpServerConfig::default().validate().is_ok());

        // Setting either TLS path is rejected -- the server can't terminate
        // TLS, so a config expecting in-pod TLS must fail loudly, not serve
        // cleartext while is_tls_enabled() reports true.
        let mut config = HttpServerConfig::default();
        config.tls_cert_path = Some("/path/to/cert.pem".to_string());
        assert!(!config.is_tls_enabled()); // only one path set
        assert!(
            config.validate().is_err(),
            "a set TLS path must be rejected"
        );

        config.tls_key_path = Some("/path/to/key.pem".to_string());
        assert!(config.is_tls_enabled());
        assert!(
            config.validate().is_err(),
            "is_tls_enabled() true but in-process TLS unsupported -> reject"
        );
    }

    #[test]
    fn test_duration_conversions() {
        let config = HttpServerConfig::default();
        assert_eq!(config.request_timeout(), Duration::from_secs(30));
        assert_eq!(config.keep_alive_timeout(), Duration::from_secs(75));
        assert_eq!(config.shutdown_timeout(), Duration::from_secs(30));
    }
}
