// Project:   hyperi-rustlib
// File:      src/secrets/types.rs
// Purpose:   Secrets type definitions
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Type definitions for the secrets module.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use super::error::SecretsResult;

/// Main configuration for the secrets manager.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SecretsConfig {
    /// Cache configuration.
    pub cache: CacheConfig,

    /// OpenBao/Vault configuration.
    #[cfg(feature = "secrets-vault")]
    pub openbao: Option<super::OpenBaoConfig>,

    /// AWS Secrets Manager configuration.
    #[cfg(feature = "secrets-aws")]
    pub aws: Option<super::AwsConfig>,

    /// Placeholder for vault config when feature disabled.
    #[cfg(not(feature = "secrets-vault"))]
    #[serde(skip)]
    pub openbao: Option<()>,

    /// Placeholder for AWS config when feature disabled.
    #[cfg(not(feature = "secrets-aws"))]
    #[serde(skip)]
    pub aws: Option<()>,

    /// Named secret sources.
    pub sources: HashMap<String, SecretSource>,
}

impl SecretsConfig {
    /// Load from the config cascade under the `secrets` key.
    #[must_use]
    pub fn from_cascade() -> Self {
        #[cfg(feature = "config")]
        {
            if let Some(cfg) = crate::config::try_get()
                && let Ok(secrets) = cfg.unmarshal_key_registered::<Self>("secrets")
            {
                return secrets;
            }
        }
        Self::default()
    }
}

/// Cache configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheConfig {
    /// Enable caching.
    pub enabled: bool,

    /// Cache directory path.
    pub directory: Option<PathBuf>,

    /// Cache TTL in seconds (how long cached secrets are considered fresh).
    pub ttl_secs: u64,

    /// Stale cache grace period in seconds (how long to use expired cache on provider failure).
    pub stale_grace_secs: u64,

    /// Background refresh interval in seconds.
    pub refresh_interval_secs: u64,

    /// Refresh jitter in seconds (randomize to avoid thundering herd).
    pub refresh_jitter_secs: u64,

    /// Optional encryption key for cache at rest (base64-encoded).
    /// If not set, cache is stored in plaintext.
    pub encryption_key: Option<String>,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            directory: None,             // Auto-detect
            ttl_secs: 3600,              // 1 hour
            stale_grace_secs: 86400,     // 24 hours
            refresh_interval_secs: 1800, // 30 minutes
            refresh_jitter_secs: 300,    // 5 minutes
            encryption_key: None,
        }
    }
}

/// Configuration for a secret source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum SecretSource {
    /// Load from local file.
    File {
        /// Path to the secret file.
        path: String,
    },

    /// Load from OpenBao/Vault.
    OpenBao {
        /// Secret path in Vault (e.g., "secret/data/myapp/tls").
        path: String,
        /// Key within the secret (e.g., "certificate").
        key: String,
    },

    /// Load from AWS Secrets Manager.
    Aws {
        /// Secret name or ARN.
        secret_id: String,
        /// Key within the JSON secret (optional for plaintext secrets).
        key: Option<String>,
    },
}

/// Value retrieved from a secrets provider.
#[derive(Debug, Clone)]
pub struct SecretValue {
    /// The secret data (may be binary or text).
    pub data: Vec<u8>,

    /// When this secret was fetched.
    pub fetched_at: SystemTime,

    /// Metadata from the provider.
    pub metadata: SecretMetadata,
}

impl SecretValue {
    /// Create a new secret value.
    #[must_use]
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            data,
            fetched_at: SystemTime::now(),
            metadata: SecretMetadata::default(),
        }
    }

    /// Create a new secret value with metadata.
    #[must_use]
    pub fn with_metadata(data: Vec<u8>, metadata: SecretMetadata) -> Self {
        Self {
            data,
            fetched_at: SystemTime::now(),
            metadata,
        }
    }

    /// Get the secret as a UTF-8 string.
    ///
    /// # Errors
    ///
    /// Returns an error if the data is not valid UTF-8.
    pub fn as_str(&self) -> SecretsResult<&str> {
        std::str::from_utf8(&self.data)
            .map_err(|e| super::error::SecretsError::InvalidData(format!("not valid UTF-8: {e}")))
    }

    /// Get the secret as bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Check if the secret has expired based on TTL.
    #[must_use]
    pub fn is_expired(&self, ttl_secs: u64) -> bool {
        self.fetched_at
            .elapsed()
            .map(|d| d.as_secs() >= ttl_secs)
            .unwrap_or(true)
    }

    /// Check if the secret is within the stale grace period.
    #[must_use]
    pub fn is_within_grace(&self, ttl_secs: u64, grace_secs: u64) -> bool {
        self.fetched_at
            .elapsed()
            .map(|d| d.as_secs() <= ttl_secs + grace_secs)
            .unwrap_or(false)
    }
}

/// Metadata about a secret.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecretMetadata {
    /// Version identifier from the provider.
    pub version: Option<String>,

    /// Provider-specific ARN or path.
    pub source_path: Option<String>,

    /// Provider name.
    pub provider: Option<String>,
}

/// Event emitted when a secret is rotated.
#[derive(Debug, Clone)]
pub struct RotationEvent {
    /// Secret name.
    pub name: String,

    /// Previous version (if known).
    pub old_version: Option<String>,

    /// New version.
    pub new_version: String,

    /// When the rotation was detected.
    pub rotated_at: SystemTime,
}

/// Serializable cache entry for disk storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CacheEntry {
    /// Base64-encoded secret data.
    pub data: String,

    /// When this secret was fetched (Unix timestamp).
    pub fetched_at_secs: u64,

    /// Metadata.
    pub metadata: SecretMetadata,
}

impl CacheEntry {
    /// Create a cache entry from a secret value.
    pub fn from_value(value: &SecretValue) -> Self {
        use base64::Engine;
        Self {
            data: base64::engine::general_purpose::STANDARD.encode(&value.data),
            fetched_at_secs: value
                .fetched_at
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            metadata: value.metadata.clone(),
        }
    }

    /// Convert to a secret value.
    pub fn to_value(&self) -> SecretsResult<SecretValue> {
        use base64::Engine;
        let data = base64::engine::general_purpose::STANDARD
            .decode(&self.data)
            .map_err(|e| super::error::SecretsError::CacheError(format!("invalid base64: {e}")))?;

        let fetched_at =
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(self.fetched_at_secs);

        Ok(SecretValue {
            data,
            fetched_at,
            metadata: self.metadata.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_value_new() {
        let value = SecretValue::new(b"test-secret".to_vec());
        assert_eq!(value.as_bytes(), b"test-secret");
        assert_eq!(value.as_str().unwrap(), "test-secret");
    }

    #[test]
    fn test_secret_value_expiry() {
        let value = SecretValue::new(b"test".to_vec());
        // Fresh secret should not be expired
        assert!(!value.is_expired(3600));
        assert!(value.is_within_grace(3600, 86400));
    }

    #[test]
    fn test_cache_entry_roundtrip() {
        let value = SecretValue::new(b"secret-data".to_vec());
        let entry = CacheEntry::from_value(&value);
        let restored = entry.to_value().unwrap();
        assert_eq!(restored.data, value.data);
    }

    #[test]
    fn test_secret_source_file_serialization() {
        let source = SecretSource::File {
            path: "/etc/ssl/cert.pem".to_string(),
        };
        let json = serde_json::to_string(&source).unwrap();
        assert!(json.contains("\"provider\":\"file\""));
    }

    #[test]
    fn test_cache_config_default() {
        let config = CacheConfig::default();
        assert!(config.enabled);
        assert_eq!(config.ttl_secs, 3600);
        assert_eq!(config.stale_grace_secs, 86400);
        assert!(config.encryption_key.is_none());
    }
}
