// Project:   hs-rustlib
// File:      src/secrets/aws.rs
// Purpose:   AWS Secrets Manager provider
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! AWS Secrets Manager provider.
//!
//! Uses the AWS SDK with automatic credential chain detection.

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_sdk_secretsmanager::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::error::{SecretsError, SecretsResult};
use super::provider::SecretProvider;
use super::types::{SecretMetadata, SecretValue};

/// AWS Secrets Manager configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AwsConfig {
    /// AWS region.
    pub region: String,

    /// Custom endpoint URL (for LocalStack or other custom endpoints).
    pub endpoint_url: Option<String>,
}

impl Default for AwsConfig {
    fn default() -> Self {
        Self {
            region: "us-east-1".into(),
            endpoint_url: None,
        }
    }
}

impl AwsConfig {
    /// Load configuration from environment variables.
    ///
    /// Uses standard `AWS_*` environment variables:
    /// - `AWS_DEFAULT_REGION` (legacy: `AWS_REGION`)
    /// - `AWS_ENDPOINT_URL` (for LocalStack or custom endpoints)
    ///
    /// Note: AWS credentials (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`)
    /// are automatically loaded by the AWS SDK credential chain.
    #[must_use]
    pub fn from_env() -> Self {
        use crate::config::env_compat::aws;

        Self {
            region: aws::region().get_or("us-east-1"),
            endpoint_url: aws::endpoint_url().get(),
        }
    }

    /// Create a configuration for a specific region.
    #[must_use]
    pub fn with_region(region: &str) -> Self {
        Self {
            region: region.to_string(),
            endpoint_url: None,
        }
    }

    /// Create a configuration for LocalStack.
    #[must_use]
    pub fn for_localstack(endpoint: &str) -> Self {
        Self {
            region: "us-east-1".to_string(),
            endpoint_url: Some(endpoint.to_string()),
        }
    }

    /// Set a custom endpoint URL.
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: &str) -> Self {
        self.endpoint_url = Some(endpoint.to_string());
        self
    }
}

/// AWS Secrets Manager provider.
pub struct AwsProvider {
    client: Client,
}

impl AwsProvider {
    /// Create a new AWS Secrets Manager provider.
    ///
    /// Uses the default AWS credential chain:
    /// 1. Environment variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`)
    /// 2. Shared credentials file (`~/.aws/credentials`)
    /// 3. IAM instance profile (EC2, ECS, Lambda)
    ///
    /// # Errors
    ///
    /// Returns an error if the AWS client cannot be initialized.
    pub fn new(config: &AwsConfig) -> SecretsResult<Self> {
        // Build AWS config synchronously for now
        // In production, this should be async
        let rt = tokio::runtime::Handle::try_current()
            .map_err(|e| SecretsError::ConfigError(format!("tokio runtime required: {e}")))?;

        let client = rt.block_on(async { Self::create_client(config).await })?;

        Ok(Self { client })
    }

    /// Create the AWS client asynchronously.
    async fn create_client(config: &AwsConfig) -> SecretsResult<Client> {
        let mut aws_config = aws_config::defaults(BehaviorVersion::latest())
            .region(aws_config::Region::new(config.region.clone()));

        if let Some(ref endpoint) = config.endpoint_url {
            aws_config = aws_config.endpoint_url(endpoint);
        }

        let aws_config = aws_config.load().await;
        Ok(Client::new(&aws_config))
    }

    /// Get a secret from AWS Secrets Manager.
    ///
    /// # Arguments
    ///
    /// * `secret_id` - Secret name or ARN
    /// * `key` - Optional key to extract from JSON secret
    ///
    /// # Errors
    ///
    /// Returns an error if the secret cannot be fetched.
    pub async fn get(&self, secret_id: &str, key: Option<&str>) -> SecretsResult<SecretValue> {
        let response = self
            .client
            .get_secret_value()
            .secret_id(secret_id)
            .send()
            .await
            .map_err(|e| {
                if e.to_string().contains("ResourceNotFoundException") {
                    SecretsError::NotFound(format!("secret not found: {secret_id}"))
                } else if e.to_string().contains("AccessDenied") {
                    SecretsError::AuthError(format!("access denied to secret: {secret_id}"))
                } else {
                    SecretsError::ProviderError(format!("failed to get secret {secret_id}: {e}"))
                }
            })?;

        // Get the secret string (most common) or binary
        let secret_data = if let Some(secret_string) = response.secret_string() {
            // If a key is specified, parse as JSON and extract
            if let Some(key) = key {
                let json: serde_json::Value = serde_json::from_str(secret_string).map_err(|e| {
                    SecretsError::InvalidData(format!(
                        "secret is not JSON, cannot extract key '{key}': {e}"
                    ))
                })?;

                let value = json.get(key).ok_or_else(|| {
                    SecretsError::NotFound(format!("key '{key}' not found in secret '{secret_id}'"))
                })?;

                match value {
                    serde_json::Value::String(s) => s.as_bytes().to_vec(),
                    _ => serde_json::to_vec(value).map_err(|e| {
                        SecretsError::InvalidData(format!("failed to serialize value: {e}"))
                    })?,
                }
            } else {
                secret_string.as_bytes().to_vec()
            }
        } else if let Some(secret_binary) = response.secret_binary() {
            secret_binary.as_ref().to_vec()
        } else {
            return Err(SecretsError::InvalidData(
                "secret has no string or binary data".into(),
            ));
        };

        let metadata = SecretMetadata {
            version: response.version_id().map(String::from),
            source_path: response.arn().map(String::from),
            provider: Some("aws".into()),
        };

        debug!(
            secret_id = %secret_id,
            version = ?metadata.version,
            "Secret fetched from AWS Secrets Manager"
        );

        Ok(SecretValue::with_metadata(secret_data, metadata))
    }
}

#[async_trait]
impl SecretProvider for AwsProvider {
    async fn get(&self, path: &str, key: Option<&str>) -> SecretsResult<SecretValue> {
        self.get(path, key).await
    }

    async fn health_check(&self) -> SecretsResult<()> {
        // List secrets with max 1 result to verify connectivity
        self.client
            .list_secrets()
            .max_results(1)
            .send()
            .await
            .map_err(|e| SecretsError::ProviderError(format!("AWS health check failed: {e}")))?;

        Ok(())
    }

    fn name(&self) -> &'static str {
        "aws"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aws_config_default() {
        let config = AwsConfig::default();
        assert_eq!(config.region, "us-east-1");
        assert!(config.endpoint_url.is_none());
    }

    #[test]
    fn test_aws_config_serialization() {
        let config = AwsConfig {
            region: "eu-west-1".into(),
            endpoint_url: Some("http://localhost:4566".into()),
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("eu-west-1"));
        assert!(json.contains("localhost:4566"));
    }

    // Integration tests would require LocalStack or real AWS credentials
    // They should be in tests/integration/
}
