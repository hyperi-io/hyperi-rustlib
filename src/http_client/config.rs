// Project:   hyperi-rustlib
// File:      src/http_client/config.rs
// Purpose:   HTTP client configuration
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! HTTP client configuration with config cascade support.

use serde::{Deserialize, Serialize};

/// Configuration for the HTTP client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpClientConfig {
    /// Request timeout in seconds. Default: 30.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    /// Connection timeout in seconds. Default: 10.
    #[serde(default = "default_connect_timeout_secs")]
    pub connect_timeout_secs: u64,

    /// Maximum number of retries for transient errors. Default: 3.
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// Minimum retry interval in milliseconds. Default: 100.
    #[serde(default = "default_min_retry_interval_ms")]
    pub min_retry_interval_ms: u64,

    /// Maximum retry interval in milliseconds. Default: 30000 (30s).
    #[serde(default = "default_max_retry_interval_ms")]
    pub max_retry_interval_ms: u64,

    /// Custom User-Agent header. Default: None (uses reqwest default).
    #[serde(default)]
    pub user_agent: Option<String>,
}

fn default_timeout_secs() -> u64 {
    30
}
fn default_connect_timeout_secs() -> u64 {
    10
}
fn default_max_retries() -> u32 {
    3
}
fn default_min_retry_interval_ms() -> u64 {
    100
}
fn default_max_retry_interval_ms() -> u64 {
    30_000
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            timeout_secs: default_timeout_secs(),
            connect_timeout_secs: default_connect_timeout_secs(),
            max_retries: default_max_retries(),
            min_retry_interval_ms: default_min_retry_interval_ms(),
            max_retry_interval_ms: default_max_retry_interval_ms(),
            user_agent: None,
        }
    }
}

impl HttpClientConfig {
    /// Load from the config cascade under the `http_client` key.
    #[must_use]
    pub fn from_cascade() -> Self {
        #[cfg(feature = "config")]
        {
            if let Some(cfg) = crate::config::try_get()
                && let Ok(hc) = cfg.unmarshal_key_registered::<Self>("http_client")
            {
                return hc;
            }
        }
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sensible() {
        let config = HttpClientConfig::default();
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.connect_timeout_secs, 10);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.min_retry_interval_ms, 100);
        assert_eq!(config.max_retry_interval_ms, 30_000);
        assert!(config.user_agent.is_none());
    }

    #[test]
    fn deserialise_from_yaml() {
        let yaml = r#"
timeout_secs: 60
max_retries: 5
user_agent: "my-app/1.0"
"#;
        let config: HttpClientConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(config.timeout_secs, 60);
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.user_agent.as_deref(), Some("my-app/1.0"));
        // Defaults for unset fields
        assert_eq!(config.connect_timeout_secs, 10);
    }
}
