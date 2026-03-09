// Project:   hyperi-rustlib
// File:      src/secrets/vault.rs
// Purpose:   OpenBao/Vault secret provider
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! OpenBao/Vault secret provider using vaultrs.
//!
//! Supports multiple authentication methods:
//! - Token authentication
//! - AppRole authentication
//! - Kubernetes authentication

use serde::{Deserialize, Serialize};
use tracing::debug;
use vaultrs::client::{Client, VaultClient, VaultClientSettingsBuilder};
use vaultrs::kv2;

use super::error::{SecretsError, SecretsResult};
use super::provider::SecretProvider;
use super::types::{SecretMetadata, SecretValue};

/// OpenBao/Vault connection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenBaoConfig {
    /// Vault address (e.g., "https://vault.example.com:8200").
    pub address: String,

    /// Authentication method.
    pub auth: OpenBaoAuth,

    /// Namespace (for Vault Enterprise).
    #[serde(default)]
    pub namespace: Option<String>,

    /// TLS CA certificate path for Vault server.
    #[serde(default)]
    pub ca_cert: Option<String>,

    /// Skip TLS verification (not recommended for production).
    #[serde(default)]
    pub skip_verify: bool,
}

/// OpenBao/Vault authentication method.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum OpenBaoAuth {
    /// Token authentication.
    Token {
        /// Vault token.
        token: String,
    },

    /// AppRole authentication.
    AppRole {
        /// Role ID.
        role_id: String,
        /// Secret ID.
        secret_id: String,
        /// Mount path (default: "approle").
        #[serde(default = "default_approle_mount")]
        mount: String,
    },

    /// Kubernetes authentication.
    Kubernetes {
        /// Role name.
        role: String,
        /// Path to service account token.
        #[serde(default = "default_k8s_token_path")]
        token_path: String,
        /// Mount path (default: "kubernetes").
        #[serde(default = "default_k8s_mount")]
        mount: String,
    },
}

fn default_approle_mount() -> String {
    "approle".to_string()
}

fn default_k8s_token_path() -> String {
    "/var/run/secrets/kubernetes.io/serviceaccount/token".to_string()
}

fn default_k8s_mount() -> String {
    "kubernetes".to_string()
}

impl OpenBaoConfig {
    /// Load configuration from environment variables.
    ///
    /// Uses standard `VAULT_*` environment variables with `OPENBAO_*` and `BAO_*`
    /// as legacy fallbacks (with deprecation warnings).
    ///
    /// ## Environment Variables
    ///
    /// - `VAULT_ADDR` - Vault/OpenBao server address
    /// - `VAULT_TOKEN` - Authentication token (for token auth)
    /// - `VAULT_ROLE_ID` + `VAULT_SECRET_ID` - AppRole authentication
    /// - `VAULT_K8S_ROLE` - Kubernetes authentication role
    /// - `VAULT_NAMESPACE` - Vault namespace (Enterprise)
    /// - `VAULT_CACERT` - Path to CA certificate
    /// - `VAULT_SKIP_VERIFY` - Skip TLS verification
    ///
    /// ## Authentication Priority
    ///
    /// 1. If `VAULT_TOKEN` is set, uses token authentication
    /// 2. If `VAULT_ROLE_ID` and `VAULT_SECRET_ID` are set, uses AppRole
    /// 3. If `VAULT_K8S_ROLE` is set, uses Kubernetes authentication
    /// 4. Otherwise, returns an error
    ///
    /// # Errors
    ///
    /// Returns `None` if `VAULT_ADDR` is not set or no authentication method
    /// can be determined.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        use crate::config::env_compat::vault;

        let address = vault::addr().get()?;

        // Determine authentication method
        let auth = if let Some(token) = vault::token().get() {
            OpenBaoAuth::Token { token }
        } else if let (Some(role_id), Some(secret_id)) = (
            vault::approle_role_id().get(),
            vault::approle_secret_id().get(),
        ) {
            OpenBaoAuth::AppRole {
                role_id,
                secret_id,
                mount: default_approle_mount(),
            }
        } else if let Some(role) = vault::k8s_role().get() {
            OpenBaoAuth::Kubernetes {
                role,
                token_path: default_k8s_token_path(),
                mount: default_k8s_mount(),
            }
        } else {
            // No authentication method configured
            return None;
        };

        Some(Self {
            address,
            auth,
            namespace: vault::namespace().get(),
            ca_cert: vault::ca_cert().get(),
            skip_verify: vault::skip_verify().get_bool().unwrap_or(false),
        })
    }

    /// Create a configuration for token authentication.
    #[must_use]
    pub fn with_token(address: &str, token: &str) -> Self {
        Self {
            address: address.to_string(),
            auth: OpenBaoAuth::Token {
                token: token.to_string(),
            },
            namespace: None,
            ca_cert: None,
            skip_verify: false,
        }
    }

    /// Create a configuration for AppRole authentication.
    #[must_use]
    pub fn with_approle(address: &str, role_id: &str, secret_id: &str) -> Self {
        Self {
            address: address.to_string(),
            auth: OpenBaoAuth::AppRole {
                role_id: role_id.to_string(),
                secret_id: secret_id.to_string(),
                mount: default_approle_mount(),
            },
            namespace: None,
            ca_cert: None,
            skip_verify: false,
        }
    }

    /// Set the namespace (for Vault Enterprise).
    #[must_use]
    pub fn with_namespace(mut self, namespace: &str) -> Self {
        self.namespace = Some(namespace.to_string());
        self
    }

    /// Set the CA certificate path.
    #[must_use]
    pub fn with_ca_cert(mut self, path: &str) -> Self {
        self.ca_cert = Some(path.to_string());
        self
    }

    /// Enable TLS skip verification (not recommended for production).
    #[must_use]
    pub fn with_skip_verify(mut self) -> Self {
        self.skip_verify = true;
        self
    }
}

/// OpenBao/Vault secret provider.
pub struct OpenBaoProvider {
    config: OpenBaoConfig,
}

impl OpenBaoProvider {
    /// Create a new OpenBao provider.
    ///
    /// # Errors
    ///
    /// Returns an error if client initialization fails.
    pub fn new(config: &OpenBaoConfig) -> SecretsResult<Self> {
        Ok(Self {
            config: config.clone(),
        })
    }

    /// Get an authenticated Vault client.
    ///
    /// Creates a new client for each request since `VaultClient` is not Clone.
    /// The underlying HTTP client uses connection pooling so this is efficient.
    async fn get_client(&self) -> SecretsResult<VaultClient> {
        self.create_client().await
    }

    /// Create and authenticate a new Vault client.
    async fn create_client(&self) -> SecretsResult<VaultClient> {
        let mut settings = VaultClientSettingsBuilder::default();
        settings.address(&self.config.address);

        if let Some(ref ns) = self.config.namespace {
            settings.namespace(Some(ns.clone()));
        }

        // Note: vaultrs handles TLS configuration via the address URL scheme
        // For custom CA certs, users should configure system trust store or use VAULT_CACERT env var

        let settings = settings.build().map_err(|e| {
            SecretsError::ConfigError(format!("failed to build Vault client settings: {e}"))
        })?;

        let mut client = VaultClient::new(settings).map_err(|e| {
            SecretsError::ProviderError(format!("failed to create Vault client: {e}"))
        })?;

        // Authenticate based on method
        match &self.config.auth {
            OpenBaoAuth::Token { token } => {
                client.set_token(token);
            }
            OpenBaoAuth::AppRole {
                role_id,
                secret_id,
                mount,
            } => {
                self.auth_approle(&mut client, role_id, secret_id, mount)
                    .await?;
            }
            OpenBaoAuth::Kubernetes {
                role,
                token_path,
                mount,
            } => {
                self.auth_kubernetes(&mut client, role, token_path, mount)
                    .await?;
            }
        }

        Ok(client)
    }

    /// Authenticate using AppRole.
    async fn auth_approle(
        &self,
        client: &mut VaultClient,
        role_id: &str,
        secret_id: &str,
        mount: &str,
    ) -> SecretsResult<()> {
        let auth_info = vaultrs::auth::approle::login(client, mount, role_id, secret_id)
            .await
            .map_err(|e| SecretsError::AuthError(format!("AppRole login failed: {e}")))?;

        client.set_token(&auth_info.client_token);
        debug!("AppRole authentication successful");
        Ok(())
    }

    /// Authenticate using Kubernetes service account.
    async fn auth_kubernetes(
        &self,
        client: &mut VaultClient,
        role: &str,
        token_path: &str,
        mount: &str,
    ) -> SecretsResult<()> {
        let jwt = tokio::fs::read_to_string(token_path).await.map_err(|e| {
            SecretsError::AuthError(format!(
                "failed to read K8s service account token from {token_path}: {e}"
            ))
        })?;

        let auth_info = vaultrs::auth::kubernetes::login(client, mount, role, jwt.trim())
            .await
            .map_err(|e| SecretsError::AuthError(format!("Kubernetes login failed: {e}")))?;

        client.set_token(&auth_info.client_token);
        debug!("Kubernetes authentication successful");
        Ok(())
    }

    /// Get a secret from Vault KV v2.
    ///
    /// # Errors
    ///
    /// Returns an error if the secret cannot be fetched.
    pub async fn get(&self, path: &str, key: &str) -> SecretsResult<SecretValue> {
        let client = self.get_client().await?;

        // Parse path to extract mount and secret path
        // Expected format: "secret/data/myapp/tls" or "myapp/tls" (assumes "secret" mount)
        let (mount, secret_path) = Self::parse_path(path);

        // Read the secret
        let secret: std::collections::HashMap<String, String> =
            kv2::read(&client, &mount, &secret_path)
                .await
                .map_err(|e| {
                    // Check for auth errors (token expired)
                    if e.to_string().contains("403") || e.to_string().contains("permission denied")
                    {
                        SecretsError::AuthError("Vault token expired or invalid".into())
                    } else {
                        SecretsError::ProviderError(format!("failed to read secret {path}: {e}"))
                    }
                })?;

        // Extract the requested key
        let value = secret.get(key).ok_or_else(|| {
            SecretsError::NotFound(format!("key '{key}' not found in secret '{path}'"))
        })?;

        let metadata = SecretMetadata {
            version: None, // KV v2 version would require additional API call
            source_path: Some(path.to_string()),
            provider: Some("openbao".into()),
        };

        Ok(SecretValue::with_metadata(
            value.as_bytes().to_vec(),
            metadata,
        ))
    }

    /// Parse a Vault path into mount and secret path.
    ///
    /// Handles formats:
    /// - "secret/data/myapp/tls" -> ("secret", "myapp/tls")
    /// - "myapp/tls" -> ("secret", "myapp/tls") (default mount)
    fn parse_path(path: &str) -> (String, String) {
        // Check for KV v2 "data" in path
        if let Some(rest) = path.strip_prefix("secret/data/") {
            return ("secret".into(), rest.into());
        }

        // Check for custom mount with "data" segment
        let parts: Vec<&str> = path.splitn(3, '/').collect();
        if parts.len() >= 3 && parts[1] == "data" {
            return (parts[0].into(), parts[2..].join("/"));
        }

        // Default to "secret" mount
        ("secret".into(), path.into())
    }
}

impl SecretProvider for OpenBaoProvider {
    async fn get(&self, path: &str, key: Option<&str>) -> SecretsResult<SecretValue> {
        let key = key.ok_or_else(|| {
            SecretsError::ConfigError("key is required for OpenBao secrets".into())
        })?;
        self.get(path, key).await
    }

    async fn health_check(&self) -> SecretsResult<()> {
        let client = self.get_client().await?;

        // Check sys/health endpoint
        vaultrs::sys::health(&client)
            .await
            .map_err(|e| SecretsError::ProviderError(format!("Vault health check failed: {e}")))?;

        Ok(())
    }

    fn name(&self) -> &'static str {
        "openbao"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_path_with_mount() {
        let (mount, path) = OpenBaoProvider::parse_path("secret/data/myapp/tls");
        assert_eq!(mount, "secret");
        assert_eq!(path, "myapp/tls");
    }

    #[test]
    fn test_parse_path_custom_mount() {
        let (mount, path) = OpenBaoProvider::parse_path("kv/data/myapp/creds");
        assert_eq!(mount, "kv");
        assert_eq!(path, "myapp/creds");
    }

    #[test]
    fn test_parse_path_default_mount() {
        let (mount, path) = OpenBaoProvider::parse_path("myapp/tls");
        assert_eq!(mount, "secret");
        assert_eq!(path, "myapp/tls");
    }

    #[test]
    fn test_openbao_auth_token_serialization() {
        let auth = OpenBaoAuth::Token {
            token: "test-token".into(),
        };
        let json = serde_json::to_string(&auth).unwrap();
        assert!(json.contains("\"method\":\"token\""));
    }

    #[test]
    fn test_openbao_auth_approle_serialization() {
        let auth = OpenBaoAuth::AppRole {
            role_id: "role123".into(),
            secret_id: "secret456".into(),
            mount: "approle".into(),
        };
        let json = serde_json::to_string(&auth).unwrap();
        assert!(json.contains("\"method\":\"app_role\""));
        assert!(json.contains("role_id"));
    }

    #[test]
    fn test_openbao_config_serialization() {
        let config = OpenBaoConfig {
            address: "https://vault.example.com:8200".into(),
            auth: OpenBaoAuth::Token {
                token: "test".into(),
            },
            namespace: Some("hypersec".into()),
            ca_cert: None,
            skip_verify: false,
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("vault.example.com"));
    }
}
