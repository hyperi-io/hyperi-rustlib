// Project:   hs-rustlib
// File:      src/clickhouse_arrow/config.rs
// Purpose:   ClickHouse connection configuration
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! ClickHouse connection configuration.

use serde::{Deserialize, Serialize};

/// ClickHouse connection configuration.
///
/// Supports multiple hosts for high availability, with credentials and timeouts.
///
/// ## Example
///
/// ```rust
/// use hs_rustlib::clickhouse::ClickHouseConfig;
///
/// let config = ClickHouseConfig {
///     hosts: vec!["clickhouse-1:9000".to_string(), "clickhouse-2:9000".to_string()],
///     database: "events".to_string(),
///     username: "app_user".to_string(),
///     password: "secret".to_string(),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickHouseConfig {
    /// List of host:port addresses.
    ///
    /// If port is omitted, defaults to 9000 (native protocol).
    pub hosts: Vec<String>,

    /// Database name to connect to.
    pub database: String,

    /// Username for authentication.
    pub username: String,

    /// Password for authentication.
    #[serde(default)]
    pub password: String,

    /// Connection timeout in milliseconds.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_ms: u64,

    /// Request timeout in milliseconds.
    #[serde(default = "default_request_timeout")]
    pub request_timeout_ms: u64,
}

const fn default_connect_timeout() -> u64 {
    5000
}

const fn default_request_timeout() -> u64 {
    30000
}

impl Default for ClickHouseConfig {
    fn default() -> Self {
        Self {
            hosts: vec!["localhost:9000".to_string()],
            database: "default".to_string(),
            username: "default".to_string(),
            password: String::new(),
            connect_timeout_ms: default_connect_timeout(),
            request_timeout_ms: default_request_timeout(),
        }
    }
}

impl ClickHouseConfig {
    /// Create a new config with minimal settings.
    #[must_use]
    pub fn new(host: impl Into<String>, database: impl Into<String>) -> Self {
        Self {
            hosts: vec![host.into()],
            database: database.into(),
            ..Default::default()
        }
    }

    /// Set credentials.
    #[must_use]
    pub fn with_credentials(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.username = username.into();
        self.password = password.into();
        self
    }

    /// Add additional hosts for high availability.
    #[must_use]
    pub fn with_hosts(mut self, hosts: Vec<String>) -> Self {
        self.hosts = hosts;
        self
    }

    /// Get the first host (primary).
    #[must_use]
    pub fn primary_host(&self) -> Option<&str> {
        self.hosts.first().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ClickHouseConfig::default();
        assert_eq!(config.hosts, vec!["localhost:9000"]);
        assert_eq!(config.database, "default");
        assert_eq!(config.username, "default");
        assert!(config.password.is_empty());
        assert_eq!(config.connect_timeout_ms, 5000);
        assert_eq!(config.request_timeout_ms, 30000);
    }

    #[test]
    fn test_builder_pattern() {
        let config = ClickHouseConfig::new("ch.example.com:9000", "mydb")
            .with_credentials("user", "pass");

        assert_eq!(config.primary_host(), Some("ch.example.com:9000"));
        assert_eq!(config.database, "mydb");
        assert_eq!(config.username, "user");
        assert_eq!(config.password, "pass");
    }
}
