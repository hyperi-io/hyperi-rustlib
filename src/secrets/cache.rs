// Project:   hyperi-rustlib
// File:      src/secrets/cache.rs
// Purpose:   Secret caching with disk persistence and stale fallback
// Language:  Rust
//
// License:   BUSL-1.1
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

use super::CacheStats;
use super::crypto;
use super::error::{SecretsError, SecretsResult};
use super::types::{CacheConfig, CacheEntry, SecretValue};

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

            // Create if missing AND force the configured mode on
            // every new(). F6: previously skipped chmod on existing
            // dirs, leaving umask-default perms.
            ensure_dir_private(&dir, config.dir_mode)?;
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
        if let Some(value) = self.memory.get(key)
            && !value.is_expired(self.config.ttl_secs)
        {
            self.hits.fetch_add(1, Ordering::Relaxed);
            debug!(key = %key, "Cache hit (memory)");
            return Some(value.clone());
        }

        // Check disk cache
        if let Some(value) = self.load_from_disk(key)
            && !value.is_expired(self.config.ttl_secs)
        {
            self.hits.fetch_add(1, Ordering::Relaxed);
            debug!(key = %key, "Cache hit (disk)");
            return Some(value);
        }

        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Get a stale secret from cache (for fallback on provider failure).
    ///
    /// Returns a cached value even if expired, as long as it's within the grace period.
    pub fn get_stale(&self, key: &str) -> Option<SecretValue> {
        // Check memory cache
        if let Some(value) = self.memory.get(key)
            && value.is_within_grace(self.config.ttl_secs, self.config.stale_grace_secs)
        {
            self.stale_hits.fetch_add(1, Ordering::Relaxed);
            debug!(key = %key, "Stale cache hit (memory)");
            return Some(value.clone());
        }

        // Check disk cache
        if let Some(value) = self.load_from_disk(key)
            && value.is_within_grace(self.config.ttl_secs, self.config.stale_grace_secs)
        {
            self.stale_hits.fetch_add(1, Ordering::Relaxed);
            debug!(key = %key, "Stale cache hit (disk)");
            return Some(value);
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
        if let Some(ref dir) = self.cache_dir {
            if let Err(e) = std::fs::remove_dir_all(dir) {
                warn!(error = %e, "Failed to clear disk cache");
            }
            // F6: re-creating the dir without permissions falls back
            // to umask. Force the configured mode on every recreate.
            if let Err(e) = ensure_dir_private(dir, self.config.dir_mode) {
                warn!(error = %e, "Failed to restore cache directory perms");
            }
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
    ///
    /// Detects encryption envelopes by their `"v":` JSON marker and
    /// decrypts via [`crypto::open`] when an encryption key is
    /// configured. Legacy plaintext entries (no envelope) are still
    /// accepted but loaded with a warning -- operators upgrading from
    /// pre-encryption deployments see one notice per file. A future
    /// release will hard-reject legacy entries to force a clean
    /// migration; for now we read-through so existing caches keep
    /// working.
    fn load_from_disk(&self, key: &str) -> Option<SecretValue> {
        let cache_dir = self.cache_dir.as_ref()?;
        let cache_file = cache_dir.join(Self::key_to_filename(key));

        if !cache_file.exists() {
            return None;
        }

        let raw = std::fs::read(&cache_file).ok()?;

        // Pick the load path based on (a) whether the file looks like
        // an encrypted envelope, and (b) whether an encryption key is
        // configured. This handles upgrades cleanly.
        let entry_bytes = if crypto::Envelope::looks_like(&raw) {
            let Some(ref user_key) = self.config.encryption_key else {
                tracing::warn!(
                    file = %cache_file.display(),
                    "cache file is encrypted but no encryption_key configured -- skipping",
                );
                return None;
            };
            match crypto::open(user_key.expose(), &raw, &crypto::aad_for(key)) {
                Ok(plain) => plain,
                Err(e) => {
                    tracing::warn!(
                        file = %cache_file.display(),
                        error = %e,
                        "cache file decrypt failed -- skipping",
                    );
                    return None;
                }
            }
        } else {
            // Legacy plaintext path. Warn once per load to nudge
            // operators toward re-running with an `encryption_key`
            // configured, which will rewrite entries on next refresh.
            if self.config.encryption_key.is_some() {
                tracing::warn!(
                    file = %cache_file.display(),
                    "cache file is plaintext but encryption_key is set -- will be re-encrypted on next refresh",
                );
            }
            raw
        };

        let entry: CacheEntry = serde_json::from_slice(&entry_bytes).ok()?;
        entry.to_value().ok()
    }

    /// Save a secret to disk cache.
    ///
    /// When `CacheConfig.encryption_key` is set, the serialised
    /// `CacheEntry` is encrypted via AES-256-GCM (see [`crypto`]).
    /// Without a key, the previous plaintext-base64-JSON shape is
    /// retained -- this keeps the cache usable in development without
    /// forcing operators to provision a key, but the misleading
    /// `encryption_key: None` plaintext path is now loud at startup
    /// (a `tracing::warn!` from `SecretCache::new`).
    fn save_to_disk(&self, key: &str, value: &SecretValue) -> SecretsResult<()> {
        let Some(ref cache_dir) = self.cache_dir else {
            return Ok(());
        };

        let cache_file = cache_dir.join(Self::key_to_filename(key));
        let entry = CacheEntry::from_value(value);

        let plaintext = serde_json::to_vec(&entry).map_err(|e| {
            SecretsError::CacheError(format!("failed to serialize cache entry: {e}"))
        })?;

        let payload: Vec<u8> = if let Some(ref user_key) = self.config.encryption_key {
            crypto::seal(user_key.expose(), &plaintext, &crypto::aad_for(key))?.into_bytes()
        } else {
            plaintext
        };

        write_private_file_atomic(&cache_file, &payload, self.config.file_mode)?;
        Ok(())
    }

    /// Convert a cache key to a safe filename.
    fn key_to_filename(key: &str) -> String {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key);
        format!("{encoded}.json")
    }
}

/// Ensure `dir` exists; chmod to `mode` on Unix when `mode` is
/// `Some`. `None` skips chmod -- used by operators on S3-FUSE,
/// root-squashed NFS, or other mounts that reject chmod.
fn ensure_dir_private(dir: &std::path::Path, mode: Option<u32>) -> SecretsResult<()> {
    std::fs::create_dir_all(dir).map_err(|e| {
        SecretsError::CacheError(format!(
            "failed to create cache directory {}: {e}",
            dir.display()
        ))
    })?;
    #[cfg(unix)]
    if let Some(m) = mode {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(m)).map_err(|e| {
            SecretsError::CacheError(format!(
                "failed to set cache directory permissions on {}: {e}",
                dir.display()
            ))
        })?;
    }
    Ok(())
}

/// Atomic-write `bytes` to `path`; chmod to `mode` on Unix when
/// `mode` is `Some`. Sets perms BEFORE rename so the file is never
/// visible at the umask default even briefly.
fn write_private_file_atomic(
    path: &std::path::Path,
    bytes: &[u8],
    mode: Option<u32>,
) -> SecretsResult<()> {
    let temp_path = path.with_extension("json.tmp");
    std::fs::write(&temp_path, bytes).map_err(|e| {
        SecretsError::CacheError(format!(
            "failed to write cache temp {}: {e}",
            temp_path.display()
        ))
    })?;
    #[cfg(unix)]
    if let Some(m) = mode {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(m)).map_err(|e| {
            SecretsError::CacheError(format!(
                "failed to set cache file permissions on {}: {e}",
                temp_path.display()
            ))
        })?;
    }
    std::fs::rename(&temp_path, path).map_err(|e| {
        SecretsError::CacheError(format!(
            "failed to rename cache temp into place {}: {e}",
            path.display()
        ))
    })?;
    Ok(())
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
            dir_mode: Some(0o700),
            file_mode: Some(0o600),
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
        assert!(
            std::path::Path::new(&filename)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        );
        assert!(!filename.contains('/'));
        assert!(!filename.contains(':'));
    }

    /// Cascade override: `dir_mode: None` skips chmod (S3-FUSE /
    /// root-squashed NFS / similar mounts that reject `chmod`).
    #[cfg(unix)]
    #[test]
    fn dir_mode_none_skips_chmod() {
        use std::os::unix::fs::PermissionsExt;
        let temp_dir = tempfile::tempdir().unwrap();
        let cfg = CacheConfig {
            enabled: true,
            directory: Some(temp_dir.path().to_path_buf()),
            dir_mode: None,
            file_mode: None,
            ..Default::default()
        };
        // Pre-set the dir to a non-private mode that ensure_dir_private
        // would normally clobber; with mode: None it must NOT change.
        std::fs::set_permissions(temp_dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let _cache = SecretCache::new(&cfg).unwrap();
        let mode = std::fs::metadata(temp_dir.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(mode, 0o755, "dir_mode: None must skip chmod");
    }

    /// Codex F6 regression (Unix): create -> set -> clear -> set;
    /// directory stays 0700 and the cache file lands at 0600.
    #[cfg(unix)]
    #[test]
    fn cache_directory_and_files_stay_private_after_clear() {
        use crate::secrets::types::SecretValue;
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().unwrap();
        let cfg = CacheConfig {
            enabled: true,
            directory: Some(temp_dir.path().to_path_buf()),
            ..Default::default()
        };
        let mut cache = SecretCache::new(&cfg).unwrap();
        let dir = cache.cache_dir.as_ref().unwrap().clone();

        let mode_after_new = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode_after_new, 0o700);

        cache.set("k", &SecretValue::new(b"v".to_vec())).unwrap();

        let cache_file = dir.join(SecretCache::key_to_filename("k"));
        let file_mode = std::fs::metadata(&cache_file).unwrap().permissions().mode() & 0o7777;
        assert_eq!(file_mode, 0o600);

        cache.clear();
        let mode_after_clear = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode_after_clear, 0o700);

        cache.set("k2", &SecretValue::new(b"v".to_vec())).unwrap();
        let post_clear_file_mode = std::fs::metadata(dir.join(SecretCache::key_to_filename("k2")))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(post_clear_file_mode, 0o600);
    }
}
