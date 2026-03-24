// Project:   hyperi-rustlib
// File:      src/cache/config.rs
// Purpose:   Cache configuration
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Cache configuration with per-source TTL support.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Configuration for the in-memory cache.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Maximum number of entries. Default: 10,000.
    #[serde(default = "default_max_capacity")]
    pub max_capacity: u64,

    /// Default TTL in seconds for entries without a source-specific TTL.
    /// Default: 3600 (1 hour).
    #[serde(default = "default_ttl_secs")]
    pub default_ttl_secs: u64,

    /// Per-source TTL overrides in seconds.
    /// Keys are source names (e.g., "http", "db", "search").
    #[serde(default)]
    pub source_ttls: HashMap<String, u64>,
}

fn default_max_capacity() -> u64 {
    10_000
}

fn default_ttl_secs() -> u64 {
    3600
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_capacity: default_max_capacity(),
            default_ttl_secs: default_ttl_secs(),
            source_ttls: HashMap::new(),
        }
    }
}

impl CacheConfig {
    /// Load from the config cascade under the `cache` key.
    #[must_use]
    pub fn from_cascade() -> Self {
        #[cfg(feature = "config")]
        {
            if let Some(cfg) = crate::config::try_get()
                && let Ok(cc) = cfg.unmarshal_key_registered::<Self>("cache")
            {
                return cc;
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
        let config = CacheConfig::default();
        assert_eq!(config.max_capacity, 10_000);
        assert_eq!(config.default_ttl_secs, 3600);
        assert!(config.source_ttls.is_empty());
    }

    #[test]
    fn deserialise_with_source_ttls() {
        let yaml = r"
max_capacity: 5000
default_ttl_secs: 1800
source_ttls:
  http: 86400
  db: 900
";
        let config: CacheConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(config.max_capacity, 5000);
        assert_eq!(config.default_ttl_secs, 1800);
        assert_eq!(config.source_ttls["http"], 86400);
        assert_eq!(config.source_ttls["db"], 900);
    }
}
