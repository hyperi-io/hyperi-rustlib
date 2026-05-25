// Project:   hyperi-rustlib
// File:      src/credential.rs
// Purpose:   Credential spec resolution (env, vault, literal)
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Credential specification resolution.
//!
//! Resolves credential specs in three formats:
//! - `vault:path:key` — fetch from OpenBao via [`crate::secrets`] (requires `secrets` feature)
//! - `env:VAR_NAME`   — read from the environment; hard error if unset
//! - any other string — used as a literal value
//!
//! Extracted from `dfe-fetcher/src/credential.rs` so that other DFE
//! services (dfe-loader, etc.) can share the same syntax.

use thiserror::Error;

/// Errors that can arise resolving a credential spec.
#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("environment variable '{name}' is not set")]
    MissingEnvVar { name: String },

    #[error("vault resolution failed: {0}")]
    Vault(String),

    #[error("invalid credential spec: {0}")]
    BadSpec(String),

    #[error("vault: spec requires the `secrets` feature to be enabled")]
    VaultUnsupported,
}

/// Resolve a credential spec to its plaintext value.
pub async fn resolve(spec: &str) -> Result<String, CredentialError> {
    if let Some(rest) = spec.strip_prefix("vault:") {
        resolve_vault(rest).await
    } else if let Some(var_name) = spec.strip_prefix("env:") {
        resolve_env(var_name)
    } else {
        Ok(spec.to_string())
    }
}

/// Resolve an optional credential spec — returns `None` for `None`/empty.
pub async fn resolve_optional(spec: Option<&str>) -> Result<Option<String>, CredentialError> {
    match spec {
        Some("") | None => Ok(None),
        Some(s) => Ok(Some(resolve(s).await?)),
    }
}

fn resolve_env(var_name: &str) -> Result<String, CredentialError> {
    std::env::var(var_name).map_err(|_| CredentialError::MissingEnvVar {
        name: var_name.to_string(),
    })
}

#[cfg(feature = "secrets")]
async fn resolve_vault(path_key: &str) -> Result<String, CredentialError> {
    use crate::secrets::{SecretSource, SecretsConfig, SecretsManager};
    use std::collections::HashMap;

    let parts: Vec<&str> = path_key.splitn(2, ':').collect();
    if parts.len() != 2 {
        return Err(CredentialError::BadSpec(format!(
            "invalid vault spec '{path_key}', expected 'path:key'"
        )));
    }
    let path = parts[0];
    let key = parts[1];

    let mut sources = HashMap::new();
    sources.insert(
        "_vault_lookup".to_string(),
        SecretSource::OpenBao {
            path: path.to_string(),
            key: key.to_string(),
        },
    );
    let config = SecretsConfig {
        sources,
        ..Default::default()
    };

    let secrets = SecretsManager::new(config)
        .map_err(|e| CredentialError::Vault(format!("init failed: {e}")))?;
    let value = secrets
        .get("_vault_lookup")
        .await
        .map_err(|e| CredentialError::Vault(format!("lookup failed for {path}:{key}: {e}")))?;
    let text = value
        .as_str()
        .map_err(|e| CredentialError::Vault(format!("not UTF-8: {e}")))?;
    tracing::debug!(path = path, key = key, "resolved vault credential");
    Ok(text.to_string())
}

#[cfg(not(feature = "secrets"))]
async fn resolve_vault(_path_key: &str) -> Result<String, CredentialError> {
    Err(CredentialError::VaultUnsupported)
}

#[cfg(test)]
#[allow(unsafe_code, clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolve_literal() {
        let v = resolve("my-secret-value").await.unwrap();
        assert_eq!(v, "my-secret-value");
    }

    #[tokio::test]
    async fn resolve_env_set() {
        // SAFETY: test-only, single-threaded
        unsafe { std::env::set_var("HYPERI_RUSTLIB_TEST_CRED", "value-123") };
        let v = resolve("env:HYPERI_RUSTLIB_TEST_CRED").await.unwrap();
        assert_eq!(v, "value-123");
        unsafe { std::env::remove_var("HYPERI_RUSTLIB_TEST_CRED") };
    }

    #[tokio::test]
    async fn resolve_env_missing() {
        let err = resolve("env:HYPERI_RUSTLIB_NONEXISTENT_XYZ").await.unwrap_err();
        match err {
            CredentialError::MissingEnvVar { name } => assert_eq!(name, "HYPERI_RUSTLIB_NONEXISTENT_XYZ"),
            other => panic!("expected MissingEnvVar, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resolve_optional_none_returns_none() {
        assert!(resolve_optional(None).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn resolve_optional_empty_returns_none() {
        assert!(resolve_optional(Some("")).await.unwrap().is_none());
    }

    #[tokio::test]
    #[cfg(not(feature = "secrets"))]
    async fn vault_without_feature_returns_clear_error() {
        let err = resolve("vault:secret/x:k").await.unwrap_err();
        assert!(matches!(err, CredentialError::VaultUnsupported));
    }
}
