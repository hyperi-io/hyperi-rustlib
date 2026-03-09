// Project:   hyperi-rustlib
// File:      src/secrets/provider.rs
// Purpose:   Secret provider trait and file provider implementation
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Secret provider trait and implementations.

use std::future::Future;
use std::path::Path;

use super::error::{SecretsError, SecretsResult};
use super::types::{SecretMetadata, SecretValue};

/// Trait for secret providers.
pub trait SecretProvider: Send + Sync {
    /// Get a secret by path/key.
    fn get(
        &self,
        path: &str,
        key: Option<&str>,
    ) -> impl Future<Output = SecretsResult<SecretValue>> + Send;

    /// Check if the provider is healthy/reachable.
    fn health_check(&self) -> impl Future<Output = SecretsResult<()>> + Send;

    /// Provider name for logging.
    fn name(&self) -> &'static str;
}

// ============================================================================
// File Provider
// ============================================================================

/// Provider that loads secrets from local filesystem.
///
/// This provider is always available and requires no additional features.
/// It reads secrets directly from files, making it compatible with:
/// - Kubernetes secrets mounted as files
/// - Docker secrets in `/run/secrets`
/// - External Secrets Operator (ESO) synced files
/// - Local development with file-based credentials
#[derive(Debug, Default)]
pub struct FileProvider;

impl FileProvider {
    /// Create a new file provider.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Get a secret from a file path.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read.
    pub async fn get(&self, path: &str) -> SecretsResult<SecretValue> {
        let path = Path::new(path);

        if !path.exists() {
            return Err(SecretsError::NotFound(format!(
                "file not found: {}",
                path.display()
            )));
        }

        let data = tokio::fs::read(path).await.map_err(|e| {
            SecretsError::IoError(std::io::Error::new(
                e.kind(),
                format!("failed to read secret file {}: {e}", path.display()),
            ))
        })?;

        let metadata = SecretMetadata {
            version: None,
            source_path: Some(path.display().to_string()),
            provider: Some("file".into()),
        };

        Ok(SecretValue::with_metadata(data, metadata))
    }
}

impl SecretProvider for FileProvider {
    async fn get(&self, path: &str, _key: Option<&str>) -> SecretsResult<SecretValue> {
        self.get(path).await
    }

    async fn health_check(&self) -> SecretsResult<()> {
        // File provider is always healthy
        Ok(())
    }

    fn name(&self) -> &'static str {
        "file"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_file_provider_missing_file() {
        let provider = FileProvider::new();
        let result = provider.get("/nonexistent/path/secret.txt").await;
        assert!(result.is_err());
        assert!(matches!(result, Err(SecretsError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_file_provider_read_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let secret_path = temp_dir.path().join("test-secret.txt");
        std::fs::write(&secret_path, "my-secret-value").unwrap();

        let provider = FileProvider::new();
        let result = provider.get(secret_path.to_str().unwrap()).await;

        assert!(result.is_ok());
        let value = result.unwrap();
        assert_eq!(value.as_str().unwrap(), "my-secret-value");
        assert_eq!(value.metadata.provider.as_deref(), Some("file"));
    }

    #[tokio::test]
    async fn test_file_provider_binary_content() {
        let temp_dir = tempfile::tempdir().unwrap();
        let secret_path = temp_dir.path().join("binary-secret");
        let binary_data: Vec<u8> = vec![0x00, 0x01, 0x02, 0xFF, 0xFE];
        std::fs::write(&secret_path, &binary_data).unwrap();

        let provider = FileProvider::new();
        let result = provider.get(secret_path.to_str().unwrap()).await;

        assert!(result.is_ok());
        let value = result.unwrap();
        assert_eq!(value.as_bytes(), &binary_data);
        // as_str should fail for binary data
        assert!(value.as_str().is_err());
    }

    #[tokio::test]
    async fn test_file_provider_health_check() {
        let provider = FileProvider::new();
        assert!(provider.health_check().await.is_ok());
    }

    #[test]
    fn test_file_provider_name() {
        let provider = FileProvider::new();
        assert_eq!(provider.name(), "file");
    }
}
