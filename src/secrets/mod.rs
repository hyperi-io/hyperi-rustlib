// Project:   hyperi-rustlib
// File:      src/secrets/mod.rs
// Purpose:   Secrets management with multi-provider support and caching
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Secrets management with multi-provider support and resilient caching.
//!
//! Provides a unified interface for loading certificates, credentials, and other
//! sensitive data from multiple sources with automatic caching for resilience.
//!
//! ## Providers
//!
//! - **File**: Local filesystem (always available)
//! - **OpenBao/Vault**: HashiCorp Vault API (requires `secrets-vault` feature)
//! - **AWS Secrets Manager**: AWS SDK (requires `secrets-aws` feature)
//!
//! ## Features
//!
//! - Multi-provider support with unified API
//! - Local disk cache with TTL for resilience
//! - Stale cache fallback when providers are unavailable
//! - Background refresh for proactive secret renewal
//! - Rotation callbacks for application notification
//!
//! ## Example
//!
//! ```rust,no_run
//! use hyperi_rustlib::secrets::{SecretsManager, SecretsConfig, SecretSource};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Simple file-based usage
//!     let secrets = SecretsManager::new(SecretsConfig::default())?;
//!     let cert = secrets.get_file("/etc/ssl/cert.pem").await?;
//!
//!     // With named sources
//!     let config = SecretsConfig {
//!         sources: vec![
//!             ("tls_cert".into(), SecretSource::File { path: "/etc/ssl/cert.pem".into() }),
//!         ].into_iter().collect(),
//!         ..Default::default()
//!     };
//!     let secrets = SecretsManager::new(config)?;
//!     let cert = secrets.get("tls_cert").await?;
//!
//!     Ok(())
//! }
//! ```

mod cache;
mod error;
mod provider;
mod types;

#[cfg(feature = "secrets-vault")]
mod vault;

#[cfg(feature = "secrets-aws")]
mod aws;

pub use cache::SecretCache;
pub use error::{SecretsError, SecretsResult};
pub use provider::{FileProvider, SecretProvider};
pub use types::{
    CacheConfig, RotationEvent, SecretMetadata, SecretSource, SecretValue, SecretsConfig,
};

#[cfg(feature = "secrets-vault")]
pub use vault::{OpenBaoAuth, OpenBaoConfig, OpenBaoProvider};

#[cfg(feature = "secrets-aws")]
pub use aws::{AwsConfig, AwsProvider};

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

/// Secrets manager that coordinates providers and caching.
pub struct SecretsManager {
    config: SecretsConfig,
    cache: Arc<RwLock<SecretCache>>,
    file_provider: FileProvider,
    #[cfg(feature = "secrets-vault")]
    vault_provider: Option<OpenBaoProvider>,
    #[cfg(feature = "secrets-aws")]
    aws_provider: Option<AwsProvider>,
    rotation_tx: broadcast::Sender<RotationEvent>,
}

impl SecretsManager {
    /// Create a new secrets manager from configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if provider initialization fails.
    pub fn new(config: SecretsConfig) -> SecretsResult<Self> {
        let cache = SecretCache::new(&config.cache)?;

        #[cfg(feature = "secrets-vault")]
        let vault_provider = config
            .openbao
            .as_ref()
            .map(OpenBaoProvider::new)
            .transpose()?;

        #[cfg(feature = "secrets-aws")]
        let aws_provider = config.aws.as_ref().map(AwsProvider::new).transpose()?;

        let (rotation_tx, _) = broadcast::channel(16);

        Ok(Self {
            config,
            cache: Arc::new(RwLock::new(cache)),
            file_provider: FileProvider::new(),
            #[cfg(feature = "secrets-vault")]
            vault_provider,
            #[cfg(feature = "secrets-aws")]
            aws_provider,
            rotation_tx,
        })
    }

    /// Get a secret by name (from configured sources).
    ///
    /// Looks up the named source in configuration and fetches from the appropriate provider.
    ///
    /// # Errors
    ///
    /// Returns an error if the secret cannot be fetched.
    pub async fn get(&self, name: &str) -> SecretsResult<SecretValue> {
        let source = self
            .config
            .sources
            .get(name)
            .ok_or_else(|| SecretsError::NotFound(format!("unknown secret source: {name}")))?
            .clone();

        self.get_from_source(name, &source).await
    }

    /// Get a secret directly from a file path.
    ///
    /// This bypasses the configured sources and reads directly from the filesystem.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read.
    pub async fn get_file(&self, path: &str) -> SecretsResult<SecretValue> {
        // Use path as cache key
        let cache_key = format!("file:{path}");

        // Check cache first
        if let Some(cached) = self.cache.read().get(&cache_key) {
            debug!(path = %path, "Secret loaded from cache");
            #[cfg(feature = "metrics")]
            metrics::counter!("dfe_secrets_cache_hits_total").increment(1);
            return Ok(cached);
        }

        #[cfg(feature = "metrics")]
        metrics::counter!("dfe_secrets_cache_misses_total").increment(1);

        // Fetch from file
        let value = self.file_provider.get(path).await?;

        #[cfg(feature = "metrics")]
        metrics::counter!("dfe_secrets_fetch_total").increment(1);

        // Update cache
        if let Err(e) = self.cache.write().set(&cache_key, &value) {
            warn!(error = %e, "Failed to cache secret");
        }

        Ok(value)
    }

    /// Get a secret from a specific source.
    async fn get_from_source(
        &self,
        cache_key: &str,
        source: &SecretSource,
    ) -> SecretsResult<SecretValue> {
        // Check cache first
        if let Some(cached) = self.cache.read().get(cache_key) {
            debug!(key = %cache_key, "Secret loaded from cache");
            #[cfg(feature = "metrics")]
            metrics::counter!("dfe_secrets_cache_hits_total").increment(1);
            return Ok(cached);
        }

        #[cfg(feature = "metrics")]
        metrics::counter!("dfe_secrets_cache_misses_total").increment(1);

        // Fetch from provider
        let result = match source {
            SecretSource::File { path } => self.file_provider.get(path).await,

            #[cfg(feature = "secrets-vault")]
            SecretSource::OpenBao { path, key } => {
                let provider = self
                    .vault_provider
                    .as_ref()
                    .ok_or_else(|| SecretsError::ProviderNotConfigured("openbao".into()))?;
                provider.get(path, key).await
            }

            #[cfg(feature = "secrets-aws")]
            SecretSource::Aws { secret_id, key } => {
                let provider = self
                    .aws_provider
                    .as_ref()
                    .ok_or_else(|| SecretsError::ProviderNotConfigured("aws".into()))?;
                provider.get(secret_id, key.as_deref()).await
            }

            #[cfg(not(feature = "secrets-vault"))]
            SecretSource::OpenBao { .. } => {
                return Err(SecretsError::ProviderNotConfigured(
                    "openbao (enable secrets-vault feature)".into(),
                ));
            }

            #[cfg(not(feature = "secrets-aws"))]
            SecretSource::Aws { .. } => {
                return Err(SecretsError::ProviderNotConfigured(
                    "aws (enable secrets-aws feature)".into(),
                ));
            }
        };

        #[cfg(feature = "metrics")]
        metrics::counter!("dfe_secrets_fetch_total").increment(1);

        match result {
            Ok(value) => {
                // Update cache
                if let Err(e) = self.cache.write().set(cache_key, &value) {
                    warn!(key = %cache_key, error = %e, "Failed to cache secret");
                }
                Ok(value)
            }
            Err(e) => {
                // Try stale cache on provider failure
                if let Some(stale) = self.cache.read().get_stale(cache_key) {
                    warn!(
                        key = %cache_key,
                        error = %e,
                        "Provider failed, using stale cached secret"
                    );
                    return Ok(stale);
                }
                Err(e)
            }
        }
    }

    /// Subscribe to rotation events.
    ///
    /// Returns a receiver that will receive events when secrets are rotated.
    #[must_use]
    pub fn subscribe_rotations(&self) -> broadcast::Receiver<RotationEvent> {
        self.rotation_tx.subscribe()
    }

    /// Refresh all configured secrets from their providers.
    ///
    /// This is useful for proactive refresh before TTL expiry.
    ///
    /// # Errors
    ///
    /// Returns an error if any secret refresh fails (but continues with others).
    pub async fn refresh_all(&self) -> SecretsResult<()> {
        let mut errors = Vec::new();

        for (name, source) in &self.config.sources {
            // Get old version for rotation detection
            let old_version = self
                .cache
                .read()
                .get(name)
                .and_then(|v| v.metadata.version.clone());

            match self.get_from_source(name, source).await {
                Ok(new_value) => {
                    // Check for rotation
                    if let Some(ref new_version) = new_value.metadata.version
                        && old_version.as_ref() != Some(new_version)
                    {
                        let event = RotationEvent {
                            name: name.clone(),
                            old_version,
                            new_version: new_version.clone(),
                            rotated_at: std::time::SystemTime::now(),
                        };
                        let _ = self.rotation_tx.send(event);
                        info!(name = %name, new_version = %new_version, "Secret rotated");
                    }
                }
                Err(e) => {
                    warn!(name = %name, error = %e, "Failed to refresh secret");
                    errors.push(format!("{name}: {e}"));
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(SecretsError::RefreshFailed(errors.join("; ")))
        }
    }

    /// Check health of all configured providers.
    ///
    /// Returns a map of provider names to their health status.
    pub async fn health_check(&self) -> HashMap<String, bool> {
        let mut health = HashMap::new();

        // File provider is always healthy
        health.insert("file".into(), true);

        #[cfg(feature = "secrets-vault")]
        if let Some(ref provider) = self.vault_provider {
            health.insert("openbao".into(), provider.health_check().await.is_ok());
        }

        #[cfg(feature = "secrets-aws")]
        if let Some(ref provider) = self.aws_provider {
            health.insert("aws".into(), provider.health_check().await.is_ok());
        }

        health
    }

    /// Clear all cached secrets.
    pub fn clear_cache(&self) {
        self.cache.write().clear();
    }

    /// Get cache statistics.
    #[must_use]
    pub fn cache_stats(&self) -> CacheStats {
        self.cache.read().stats()
    }
}

/// Cache statistics.
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Number of entries in memory cache.
    pub memory_entries: usize,
    /// Number of entries in disk cache.
    pub disk_entries: usize,
    /// Total cache hits.
    pub hits: u64,
    /// Total cache misses.
    pub misses: u64,
    /// Total stale hits (fallback).
    pub stale_hits: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secrets_config_default() {
        let config = SecretsConfig::default();
        assert!(config.cache.enabled);
        assert_eq!(config.cache.ttl_secs, 3600);
        assert!(config.sources.is_empty());
    }

    #[test]
    fn test_secrets_manager_new() {
        let manager = SecretsManager::new(SecretsConfig::default());
        assert!(manager.is_ok());
    }

    #[tokio::test]
    async fn test_file_provider_missing_file() {
        let manager = SecretsManager::new(SecretsConfig::default()).unwrap();
        let result = manager.get_file("/nonexistent/path/secret.txt").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_file_provider_read_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let secret_path = temp_dir.path().join("test-secret.txt");
        std::fs::write(&secret_path, "my-secret-value").unwrap();

        let manager = SecretsManager::new(SecretsConfig::default()).unwrap();
        let result = manager.get_file(secret_path.to_str().unwrap()).await;

        assert!(result.is_ok());
        let value = result.unwrap();
        assert_eq!(value.as_str().unwrap(), "my-secret-value");
    }

    #[tokio::test]
    async fn test_named_source_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let secret_path = temp_dir.path().join("api-key.txt");
        std::fs::write(&secret_path, "super-secret-key").unwrap();

        let config = SecretsConfig {
            sources: [(
                "api_key".into(),
                SecretSource::File {
                    path: secret_path.to_str().unwrap().into(),
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };

        let manager = SecretsManager::new(config).unwrap();
        let value = manager.get("api_key").await.unwrap();
        assert_eq!(value.as_str().unwrap(), "super-secret-key");
    }

    #[tokio::test]
    async fn test_unknown_source() {
        let manager = SecretsManager::new(SecretsConfig::default()).unwrap();
        let result = manager.get("nonexistent").await;
        assert!(matches!(result, Err(SecretsError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_health_check() {
        let manager = SecretsManager::new(SecretsConfig::default()).unwrap();
        let health = manager.health_check().await;
        assert!(health.get("file").copied().unwrap_or(false));
    }
}
