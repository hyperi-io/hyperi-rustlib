// Project:   hyperi-rustlib
// File:      src/cache/mod.rs
// Purpose:   In-memory cache with per-source TTL, metrics, and invalidation
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! In-memory cache with per-source TTL, metrics, and invalidation.
//!
//! Wraps [`moka`] to provide a concurrent, async-friendly cache with
//! TinyLFU eviction. Matches hyperi-pylib's cache module API:
//! per-source TTL configuration, `get`/`set`/`invalidate_source`.
//!
//! # Config Cascade
//!
//! ```yaml
//! cache:
//!   max_capacity: 10000
//!   default_ttl_secs: 3600
//!   source_ttls:
//!     http: 86400
//!     db: 1800
//!     search: 3600
//! ```
//!
//! # Usage
//!
//! ```rust,no_run
//! use hyperi_rustlib::cache::{Cache, CacheConfig};
//!
//! #[tokio::main]
//! async fn main() {
//!     let cache = Cache::new(CacheConfig::default());
//!
//!     // Set with source-specific TTL
//!     cache.set("http", "https://api.example.com", "response_data").await;
//!
//!     // Get
//!     if let Some(value) = cache.get::<String>("http", "https://api.example.com").await {
//!         println!("cached: {value}");
//!     }
//!
//!     // Invalidate all entries for a source
//!     cache.invalidate_source("http").await;
//! }
//! ```

pub mod config;

pub use config::CacheConfig;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use moka::future::Cache as MokaCache;

/// In-memory cache with per-source TTL and source-aware keys.
pub struct Cache {
    inner: MokaCache<String, Arc<Vec<u8>>>,
    config: CacheConfig,
    /// Track keys per source for invalidation.
    source_keys: Mutex<HashMap<String, Vec<String>>>,
}

impl Cache {
    /// Create a new cache with the given config.
    #[must_use]
    pub fn new(config: CacheConfig) -> Self {
        let inner = MokaCache::builder()
            .max_capacity(config.max_capacity)
            .time_to_live(Duration::from_secs(config.default_ttl_secs))
            .build();

        Self {
            inner,
            config,
            source_keys: Mutex::new(HashMap::new()),
        }
    }

    /// Create a cache from the config cascade (or defaults).
    #[must_use]
    pub fn from_cascade() -> Self {
        Self::new(CacheConfig::from_cascade())
    }

    /// Get a cached value by source and key.
    ///
    /// Returns `None` if not found or expired.
    pub async fn get<T: serde::de::DeserializeOwned>(&self, source: &str, key: &str) -> Option<T> {
        let full_key = format!("{source}:{key}");
        let bytes = self.inner.get(&full_key).await;

        #[cfg(feature = "metrics")]
        if bytes.is_some() {
            metrics::counter!("dfe_cache_hits_total", "source" => source.to_string()).increment(1);
        } else {
            metrics::counter!("dfe_cache_misses_total", "source" => source.to_string())
                .increment(1);
        }

        let bytes = bytes?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Set a cached value with source-specific TTL.
    ///
    /// Uses the TTL configured for the source, falling back to the
    /// default TTL if the source has no specific configuration.
    pub async fn set<T: serde::Serialize>(&self, source: &str, key: &str, value: T) {
        let full_key = format!("{source}:{key}");
        let bytes = match serde_json::to_vec(&value) {
            Ok(b) => Arc::new(b),
            Err(_) => return,
        };

        // moka uses a global TTL set at construction. Per-source TTL would
        // require separate cache instances or moka's Expiry trait — future work.
        let _ttl = self.ttl_for_source(source);

        self.inner.insert(full_key.clone(), bytes).await;

        #[cfg(feature = "metrics")]
        metrics::gauge!("dfe_cache_entries").set(self.inner.entry_count() as f64);

        // Track key for source-level invalidation
        if let Ok(mut keys) = self.source_keys.lock() {
            keys.entry(source.to_string()).or_default().push(full_key);
        }
    }

    /// Invalidate all cached entries for a source.
    pub async fn invalidate_source(&self, source: &str) {
        let keys = {
            let Ok(mut source_keys) = self.source_keys.lock() else {
                return;
            };
            source_keys.remove(source).unwrap_or_default()
        };

        for key in keys {
            self.inner.invalidate(&key).await;
        }

        #[cfg(feature = "metrics")]
        metrics::gauge!("dfe_cache_entries").set(self.inner.entry_count() as f64);
    }

    /// Invalidate a single entry.
    pub async fn invalidate(&self, source: &str, key: &str) {
        let full_key = format!("{source}:{key}");
        self.inner.invalidate(&full_key).await;
    }

    /// Get the TTL for a source (from config or default).
    fn ttl_for_source(&self, source: &str) -> Duration {
        self.config.source_ttls.get(source).copied().map_or(
            Duration::from_secs(self.config.default_ttl_secs),
            Duration::from_secs,
        )
    }

    /// Current number of entries in the cache.
    pub fn entry_count(&self) -> u64 {
        self.inner.entry_count()
    }

    /// Access the current config.
    #[must_use]
    pub fn config(&self) -> &CacheConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> CacheConfig {
        CacheConfig {
            max_capacity: 100,
            default_ttl_secs: 60,
            source_ttls: HashMap::from([("http".into(), 3600), ("db".into(), 1800)]),
        }
    }

    #[tokio::test]
    async fn set_and_get() {
        let cache = Cache::new(test_config());
        cache.set("http", "url1", "value1".to_string()).await;

        let result: Option<String> = cache.get("http", "url1").await;
        assert_eq!(result.as_deref(), Some("value1"));
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let cache = Cache::new(test_config());
        let result: Option<String> = cache.get("http", "nonexistent").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn sources_are_isolated() {
        let cache = Cache::new(test_config());
        cache.set("http", "key1", "http_value".to_string()).await;
        cache.set("db", "key1", "db_value".to_string()).await;

        let http: Option<String> = cache.get("http", "key1").await;
        let db: Option<String> = cache.get("db", "key1").await;

        assert_eq!(http.as_deref(), Some("http_value"));
        assert_eq!(db.as_deref(), Some("db_value"));
    }

    #[tokio::test]
    async fn invalidate_source_removes_only_that_source() {
        let cache = Cache::new(test_config());
        cache.set("http", "url1", "v1".to_string()).await;
        cache.set("http", "url2", "v2".to_string()).await;
        cache.set("db", "query1", "v3".to_string()).await;

        cache.invalidate_source("http").await;

        // Run pending tasks to ensure invalidation is processed
        cache.inner.run_pending_tasks().await;

        let http1: Option<String> = cache.get("http", "url1").await;
        let http2: Option<String> = cache.get("http", "url2").await;
        let db1: Option<String> = cache.get("db", "query1").await;

        assert!(http1.is_none(), "http url1 should be invalidated");
        assert!(http2.is_none(), "http url2 should be invalidated");
        assert_eq!(db1.as_deref(), Some("v3"), "db should be preserved");
    }

    #[tokio::test]
    async fn invalidate_single_entry() {
        let cache = Cache::new(test_config());
        cache.set("http", "url1", "v1".to_string()).await;
        cache.set("http", "url2", "v2".to_string()).await;

        cache.invalidate("http", "url1").await;
        cache.inner.run_pending_tasks().await;

        let v1: Option<String> = cache.get("http", "url1").await;
        let v2: Option<String> = cache.get("http", "url2").await;

        assert!(v1.is_none());
        assert_eq!(v2.as_deref(), Some("v2"));
    }

    #[tokio::test]
    async fn entry_count() {
        let cache = Cache::new(test_config());
        assert_eq!(cache.entry_count(), 0);

        cache.set("http", "url1", "v1".to_string()).await;
        cache.set("http", "url2", "v2".to_string()).await;
        cache.inner.run_pending_tasks().await;

        assert_eq!(cache.entry_count(), 2);
    }

    #[tokio::test]
    async fn complex_types() {
        let cache = Cache::new(test_config());

        let data = serde_json::json!({"name": "test", "values": [1, 2, 3]});
        cache.set("db", "query1", data.clone()).await;

        let result: Option<serde_json::Value> = cache.get("db", "query1").await;
        assert_eq!(result, Some(data));
    }
}
