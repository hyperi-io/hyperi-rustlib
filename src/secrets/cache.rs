// Project:   hyperi-rustlib
// File:      src/secrets/cache.rs
// Purpose:   Secret caching with disk persistence and stale fallback
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Secret caching with disk persistence and stale fallback.
//!
//! The cache provides resilience when external providers are unavailable:
//!
//! ```text
//! get_secret(key)
//!     │
//!     ├─ Check memory cache
//!     │   └─ Hit + fresh → Return immediately
//!     │
//!     ├─ Check disk cache
//!     │   └─ Hit + fresh → Update memory, return
//!     │
//!     └─ Return None (caller fetches from provider)
//!
//! get_stale(key)  // Called on provider failure
//!     │
//!     ├─ Check memory cache (within grace period)
//!     │   └─ Hit → Return with warning
//!     │
//!     └─ Check disk cache (within grace period)
//!         └─ Hit → Return with warning
//! ```

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use tracing::{debug, warn};

use super::error::{SecretsError, SecretsResult};
use super::types::{CacheConfig, CacheEntry, SecretValue};
use super::CacheStats;

/// Secret cache with memory and disk tiers.
pub struct SecretCache {
    /// In-memory cache.
    memory: HashMap<String, SecretValue>,

    /// Disk cache directory.
    cache_dir: Option<PathBuf>,

    /// Configuration.
    config: CacheConfig,

    /// Statistics.
    hits: AtomicU64,
    misses: AtomicU64,
    stale_hits: AtomicU64,
}

impl SecretCache {
    /// Create a new secret cache.
    ///
    /// # Errors
    ///
    /// Returns an error if the cache directory cannot be created.
    pub fn new(config: &CacheConfig) -> SecretsResult<Self> {
        let cache_dir = if config.enabled {
            let dir = config.directory.clone().unwrap_or_else(|| {
                // Auto-detect cache directory
                dirs::cache_dir()
                    .unwrap_or_else(|| PathBuf::from("/tmp"))
                    .join("hyperi-rustlib")
                    .join("secrets")
            });

            // Create directory if it doesn't exist
            if !dir.exists() {
                std::fs::create_dir_all(&dir).map_err(|e| {
                    SecretsError::CacheError(format!(
                        "failed to create cache directory {}: {e}",
                        dir.display()
                    ))
                })?;
            }

            Some(dir)
        } else {
            None
        };

        Ok(Self {
            memory: HashMap::new(),
            cache_dir,
            config: config.clone(),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            stale_hits: AtomicU64::new(0),
        })
    }

    /// Get a fresh secret from cache.
    ///
    /// Returns `None` if not cached or expired.
    pub fn get(&self, key: &str) -> Option<SecretValue> {
        // Check memory cache
        if let Some(value) = self.memory.get(key) {
            if !value.is_expired(self.config.ttl_secs) {
                self.hits.fetch_add(1, Ordering::Relaxed);
                debug!(key = %key, "Cache hit (memory)");
                return Some(value.clone());
            }
        }

        // Check disk cache
        if let Some(value) = self.load_from_disk(key) {
            if !value.is_expired(self.config.ttl_secs) {
                self.hits.fetch_add(1, Ordering::Relaxed);
                debug!(key = %key, "Cache hit (disk)");
                return Some(value);
            }
        }

        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Get a stale secret from cache (for fallback on provider failure).
    ///
    /// Returns a cached value even if expired, as long as it's within the grace period.
    pub fn get_stale(&self, key: &str) -> Option<SecretValue> {
        // Check memory cache
        if let Some(value) = self.memory.get(key) {
            if value.is_within_grace(self.config.ttl_secs, self.config.stale_grace_secs) {
                self.stale_hits.fetch_add(1, Ordering::Relaxed);
                debug!(key = %key, "Stale cache hit (memory)");
                return Some(value.clone());
            }
        }

        // Check disk cache
        if let Some(value) = self.load_from_disk(key) {
            if value.is_within_grace(self.config.ttl_secs, self.config.stale_grace_secs) {
                self.stale_hits.fetch_add(1, Ordering::Relaxed);
                debug!(key = %key, "Stale cache hit (disk)");
                return Some(value);
            }
        }

        None
    }

    /// Store a secret in cache.
    ///
    /// # Errors
    ///
    /// Returns an error if disk cache write fails.
    pub fn set(&mut self, key: &str, value: &SecretValue) -> SecretsResult<()> {
        if !self.config.enabled {
            return Ok(());
        }

        // Store in memory
        self.memory.insert(key.to_string(), value.clone());

        // Store on disk
        self.save_to_disk(key, value)?;

        debug!(key = %key, "Secret cached");
        Ok(())
    }

    /// Clear all cached secrets.
    pub fn clear(&mut self) {
        self.memory.clear();

        // Clear disk cache
        if let Some(ref dir) = self.cache_dir {
            if let Err(e) = std::fs::remove_dir_all(dir) {
                warn!(error = %e, "Failed to clear disk cache");
            }
            let _ = std::fs::create_dir_all(dir);
        }
    }

    /// Get cache statistics.
    pub fn stats(&self) -> CacheStats {
        let disk_entries = self
            .cache_dir
            .as_ref()
            .and_then(|dir| std::fs::read_dir(dir).ok())
            .map_or(0, |entries| entries.count());

        CacheStats {
            memory_entries: self.memory.len(),
            disk_entries,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            stale_hits: self.stale_hits.load(Ordering::Relaxed),
        }
    }

    /// Load a secret from disk cache.
    fn load_from_disk(&self, key: &str) -> Option<SecretValue> {
        let cache_dir = self.cache_dir.as_ref()?;
        let cache_file = cache_dir.join(Self::key_to_filename(key));

        if !cache_file.exists() {
            return None;
        }

        let content = std::fs::read_to_string(&cache_file).ok()?;
        let entry: CacheEntry = serde_json::from_str(&content).ok()?;
        entry.to_value().ok()
    }

    /// Save a secret to disk cache.
    fn save_to_disk(&self, key: &str, value: &SecretValue) -> SecretsResult<()> {
        let Some(ref cache_dir) = self.cache_dir else {
            return Ok(());
        };

        let cache_file = cache_dir.join(Self::key_to_filename(key));
        let entry = CacheEntry::from_value(value);

        let content = serde_json::to_string_pretty(&entry).map_err(|e| {
            SecretsError::CacheError(format!("failed to serialize cache entry: {e}"))
        })?;

        std::fs::write(&cache_file, content).map_err(|e| {
            SecretsError::CacheError(format!(
                "failed to write cache file {}: {e}",
                cache_file.display()
            ))
        })?;

        Ok(())
    }

    /// Convert a cache key to a safe filename.
    fn key_to_filename(key: &str) -> String {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key);
        format!("{encoded}.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> CacheConfig {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().to_path_buf();
        // Keep the temp dir from being deleted
        std::mem::forget(temp_dir);
        CacheConfig {
            enabled: true,
            directory: Some(path),
            ttl_secs: 3600,
            stale_grace_secs: 86400,
            refresh_interval_secs: 1800,
            refresh_jitter_secs: 300,
            encryption_key: None,
        }
    }

    #[test]
    fn test_cache_new() {
        let config = test_config();
        let cache = SecretCache::new(&config);
        assert!(cache.is_ok());
    }

    #[test]
    fn test_cache_disabled() {
        let config = CacheConfig {
            enabled: false,
            ..Default::default()
        };
        let cache = SecretCache::new(&config).unwrap();
        assert!(cache.cache_dir.is_none());
    }

    #[test]
    fn test_cache_set_get() {
        let config = test_config();
        let mut cache = SecretCache::new(&config).unwrap();

        let value = SecretValue::new(b"secret-data".to_vec());
        cache.set("test-key", &value).unwrap();

        let retrieved = cache.get("test-key");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().as_bytes(), b"secret-data");
    }

    #[test]
    fn test_cache_miss() {
        let config = test_config();
        let cache = SecretCache::new(&config).unwrap();

        let retrieved = cache.get("nonexistent");
        assert!(retrieved.is_none());
    }

    #[test]
    fn test_cache_disk_persistence() {
        let config = test_config();

        // Store a secret
        {
            let mut cache = SecretCache::new(&config).unwrap();
            let value = SecretValue::new(b"persistent-secret".to_vec());
            cache.set("persist-key", &value).unwrap();
        }

        // Create a new cache instance and retrieve
        {
            let cache = SecretCache::new(&config).unwrap();
            let retrieved = cache.get("persist-key");
            assert!(retrieved.is_some());
            assert_eq!(retrieved.unwrap().as_bytes(), b"persistent-secret");
        }
    }

    #[test]
    fn test_cache_stale_fallback() {
        let config = CacheConfig {
            ttl_secs: 0,             // Immediately expired
            stale_grace_secs: 86400, // But within grace
            ..test_config()
        };
        let mut cache = SecretCache::new(&config).unwrap();

        let value = SecretValue::new(b"stale-secret".to_vec());
        cache.set("stale-key", &value).unwrap();

        // get() should return None (expired)
        assert!(cache.get("stale-key").is_none());

        // get_stale() should return the value (within grace)
        let stale = cache.get_stale("stale-key");
        assert!(stale.is_some());
        assert_eq!(stale.unwrap().as_bytes(), b"stale-secret");
    }

    #[test]
    fn test_cache_clear() {
        let config = test_config();
        let mut cache = SecretCache::new(&config).unwrap();

        let value = SecretValue::new(b"secret".to_vec());
        cache.set("key1", &value).unwrap();
        cache.set("key2", &value).unwrap();

        cache.clear();

        assert!(cache.get("key1").is_none());
        assert!(cache.get("key2").is_none());
        assert_eq!(cache.stats().memory_entries, 0);
    }

    #[test]
    fn test_cache_stats() {
        let config = test_config();
        let mut cache = SecretCache::new(&config).unwrap();

        let value = SecretValue::new(b"secret".to_vec());
        cache.set("key", &value).unwrap();

        // Hit
        let _ = cache.get("key");
        // Miss
        let _ = cache.get("nonexistent");

        let stats = cache.stats();
        assert_eq!(stats.memory_entries, 1);
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
    }

    #[test]
    fn test_key_to_filename() {
        let filename = SecretCache::key_to_filename("test/key:with/special");
        assert!(std::path::Path::new(&filename)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json")));
        assert!(!filename.contains('/'));
        assert!(!filename.contains(':'));
    }
}
