// Project:   hs-rustlib
// File:      src/config/postgres.rs
// Purpose:   PostgreSQL configuration source for the config cascade
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! PostgreSQL configuration source.
//!
//! Adds PostgreSQL as a configuration source in the cascade, sitting above
//! file-based config (layer 4).
//!
//! ## Cascade Priority
//!
//! 1. CLI args
//! 2. ENV vars (`MYAPP_*`)
//! 3. `.env` file
//! 4. **PostgreSQL** ← this layer
//! 5. `settings.{env}.yaml`
//! 6. `settings.yaml`
//! 7. `defaults.yaml`
//! 8. Hard-coded defaults
//!
//! ## Usage
//!
//! ```rust,no_run
//! use hs_rustlib::config::postgres::{PostgresConfigSource, PostgresConfig};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let source = PostgresConfigSource {
//!         enabled: true,
//!         url: Some("postgres://user:pass@localhost:5432/config".to_string()),
//!         namespace: "my-app".to_string(),
//!         ..Default::default()
//!     };
//!
//!     if let Some(pg_config) = PostgresConfig::load(&source).await? {
//!         println!("Loaded {} config keys from PostgreSQL", pg_config.values.len());
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Fallback File
//!
//! When PostgreSQL is unavailable, config can be loaded from a fallback file.
//! On successful load, the config is written to the fallback file for future use.
//!
//! ```rust,no_run
//! use hs_rustlib::config::postgres::{PostgresConfigSource, FallbackMode};
//!
//! let source = PostgresConfigSource {
//!     enabled: true,
//!     url: Some("postgres://localhost/config".to_string()),
//!     namespace: "my-app".to_string(),
//!     fallback_enabled: true,
//!     fallback_file: Some("/var/cache/myapp/config-fallback.json".into()),
//!     fallback_mode: FallbackMode::Replace,
//!     ..Default::default()
//! };
//! ```
//!
//! ## Environment Variables
//!
//! Bootstrap PostgreSQL connection via environment:
//!
//! | Variable | Description | Default |
//! |----------|-------------|---------|
//! | `{PREFIX}_CONFIG_POSTGRES_ENABLED` | Enable PostgreSQL config | `false` |
//! | `{PREFIX}_CONFIG_POSTGRES_URL` | Connection URL | None |
//! | `{PREFIX}_CONFIG_POSTGRES_NAMESPACE` | Config namespace | `default` |
//! | `{PREFIX}_CONFIG_POSTGRES_CONNECT_TIMEOUT` | Connect timeout (secs) | `5` |
//! | `{PREFIX}_CONFIG_POSTGRES_QUERY_TIMEOUT` | Query timeout (secs) | `10` |
//! | `{PREFIX}_CONFIG_POSTGRES_RETRY_ATTEMPTS` | Retry attempts | `3` |
//! | `{PREFIX}_CONFIG_POSTGRES_RETRY_DELAY_MS` | Retry delay (ms) | `1000` |
//! | `{PREFIX}_CONFIG_POSTGRES_OPTIONAL` | Continue if unavailable | `true` |
//! | `{PREFIX}_CONFIG_FALLBACK_ENABLED` | Enable fallback file | `false` |
//! | `{PREFIX}_CONFIG_FALLBACK_FILE` | Fallback file path | None |
//! | `{PREFIX}_CONFIG_FALLBACK_MODE` | `replace` or `merge` | `replace` |

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Row;
use thiserror::Error;

#[cfg(feature = "tracing")]
use tracing::{debug, info, warn};

/// Fallback mode for when PostgreSQL is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FallbackMode {
    /// Replace file-based config entirely with fallback.
    #[default]
    Replace,
    /// Merge fallback with file-based config (fallback wins on conflict).
    Merge,
}

/// Configuration for PostgreSQL config source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PostgresConfigSource {
    /// Enable PostgreSQL config source.
    pub enabled: bool,

    /// PostgreSQL connection URL.
    ///
    /// Format: `postgres://user:password@host:port/database`
    pub url: Option<String>,

    /// Config namespace for multi-tenant config.
    ///
    /// Each application can have its own config namespace.
    pub namespace: String,

    /// Connection timeout in seconds.
    pub connect_timeout_secs: u64,

    /// Query timeout in seconds.
    pub query_timeout_secs: u64,

    /// Retry attempts on connection failure.
    pub retry_attempts: u32,

    /// Retry delay in milliseconds.
    pub retry_delay_ms: u64,

    /// Continue startup if PostgreSQL is unavailable.
    ///
    /// If `true`: log warning, use fallback or file/env config only.
    /// If `false`: fail startup.
    pub optional: bool,

    /// Enable fallback file caching.
    ///
    /// When enabled, successful PostgreSQL loads are cached to a file.
    /// If PostgreSQL is unavailable, the cached config is used instead.
    pub fallback_enabled: bool,

    /// Path to fallback file.
    ///
    /// Defaults to `{cache_dir}/hs-config-fallback.json` if not specified.
    pub fallback_file: Option<PathBuf>,

    /// Fallback mode when using cached config.
    pub fallback_mode: FallbackMode,
}

impl Default for PostgresConfigSource {
    fn default() -> Self {
        Self {
            enabled: false,
            url: None,
            namespace: "default".to_string(),
            connect_timeout_secs: 5,
            query_timeout_secs: 10,
            retry_attempts: 3,
            retry_delay_ms: 1000,
            optional: true,
            fallback_enabled: false,
            fallback_file: None,
            fallback_mode: FallbackMode::Replace,
        }
    }
}

impl PostgresConfigSource {
    /// Load PostgreSQL source config from environment variables.
    ///
    /// This solves the bootstrap problem: we need to know where PostgreSQL is
    /// before we can load config from it.
    #[must_use]
    pub fn from_env(prefix: &str) -> Self {
        let prefix = prefix.to_uppercase();

        Self {
            enabled: std::env::var(format!("{prefix}_CONFIG_POSTGRES_ENABLED"))
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false),
            url: std::env::var(format!("{prefix}_CONFIG_POSTGRES_URL")).ok(),
            namespace: std::env::var(format!("{prefix}_CONFIG_POSTGRES_NAMESPACE"))
                .unwrap_or_else(|_| "default".to_string()),
            connect_timeout_secs: std::env::var(format!("{prefix}_CONFIG_POSTGRES_CONNECT_TIMEOUT"))
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(5),
            query_timeout_secs: std::env::var(format!("{prefix}_CONFIG_POSTGRES_QUERY_TIMEOUT"))
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(10),
            retry_attempts: std::env::var(format!("{prefix}_CONFIG_POSTGRES_RETRY_ATTEMPTS"))
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(3),
            retry_delay_ms: std::env::var(format!("{prefix}_CONFIG_POSTGRES_RETRY_DELAY_MS"))
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1000),
            optional: std::env::var(format!("{prefix}_CONFIG_POSTGRES_OPTIONAL"))
                .map(|v| !v.eq_ignore_ascii_case("false") && v != "0")
                .unwrap_or(true),
            fallback_enabled: std::env::var(format!("{prefix}_CONFIG_FALLBACK_ENABLED"))
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false),
            fallback_file: std::env::var(format!("{prefix}_CONFIG_FALLBACK_FILE"))
                .ok()
                .map(PathBuf::from),
            fallback_mode: std::env::var(format!("{prefix}_CONFIG_FALLBACK_MODE"))
                .ok()
                .and_then(|v| match v.to_lowercase().as_str() {
                    "merge" => Some(FallbackMode::Merge),
                    "replace" => Some(FallbackMode::Replace),
                    _ => None,
                })
                .unwrap_or(FallbackMode::Replace),
        }
    }

    /// Get the effective fallback file path.
    ///
    /// Returns the configured path, or a default in the cache directory.
    #[must_use]
    pub fn fallback_path(&self) -> Option<PathBuf> {
        if !self.fallback_enabled {
            return None;
        }

        self.fallback_file.clone().or_else(|| {
            dirs::cache_dir().map(|d| d.join("hs-config-fallback.json"))
        })
    }
}

/// Loaded configuration from PostgreSQL.
#[derive(Debug, Clone)]
pub struct PostgresConfig {
    /// Flat map of dot-notation keys to JSON values.
    ///
    /// e.g., `"kafka.brokers"` -> `["broker1:9092"]`
    pub values: HashMap<String, serde_json::Value>,
}

impl PostgresConfig {
    /// Load configuration from PostgreSQL.
    ///
    /// Returns `Ok(None)` if:
    /// - The source is disabled
    /// - No URL is configured and `optional` is true
    /// - Connection fails and `optional` is true (and no fallback available)
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - No URL is configured and `optional` is false
    /// - Connection fails and `optional` is false (and no fallback available)
    /// - Query execution fails
    pub async fn load(source: &PostgresConfigSource) -> Result<Option<Self>, PostgresConfigError> {
        if !source.enabled {
            #[cfg(feature = "tracing")]
            debug!("PostgreSQL config source disabled");
            return Ok(None);
        }

        let Some(url) = &source.url else {
            if source.optional {
                #[cfg(feature = "tracing")]
                debug!("PostgreSQL config URL not configured, skipping");
                return Ok(None);
            }
            return Err(PostgresConfigError::NotConfigured);
        };
        let url = url.clone();

        let pool = match Self::connect_with_retry(&url, source).await {
            Ok(pool) => pool,
            Err(PostgresConfigError::Unavailable) if source.optional => {
                // Try loading from fallback file
                if let Some(config) = Self::load_fallback_file(source)? {
                    #[cfg(feature = "tracing")]
                    info!(
                        keys = config.values.len(),
                        "Loaded configuration from fallback file"
                    );
                    return Ok(Some(config));
                }
                return Ok(None);
            }
            Err(e) => {
                // Try loading from fallback file on connection error
                if source.optional {
                    if let Some(config) = Self::load_fallback_file(source)? {
                        #[cfg(feature = "tracing")]
                        info!(
                            keys = config.values.len(),
                            "Loaded configuration from fallback file (connection error)"
                        );
                        return Ok(Some(config));
                    }
                }
                return Err(e);
            }
        };

        let values = Self::query_config(&pool, &source.namespace).await?;

        #[cfg(feature = "tracing")]
        info!(
            keys = values.len(),
            namespace = %source.namespace,
            "Loaded configuration from PostgreSQL"
        );

        let config = Self { values };

        // Write to fallback file on successful load
        if let Err(e) = config.write_fallback_file(source) {
            #[cfg(feature = "tracing")]
            warn!(error = %e, "Failed to write config fallback file");
        }

        Ok(Some(config))
    }

    /// Connect to PostgreSQL with retry logic.
    async fn connect_with_retry(
        url: &str,
        source: &PostgresConfigSource,
    ) -> Result<PgPool, PostgresConfigError> {
        let mut last_error = None;

        for attempt in 1..=source.retry_attempts {
            match PgPoolOptions::new()
                .max_connections(1)
                .acquire_timeout(Duration::from_secs(source.connect_timeout_secs))
                .connect(url)
                .await
            {
                Ok(pool) => {
                    #[cfg(feature = "tracing")]
                    debug!(attempt, "Connected to PostgreSQL config database");
                    return Ok(pool);
                }
                Err(e) => {
                    #[cfg(feature = "tracing")]
                    warn!(
                        attempt,
                        max_attempts = source.retry_attempts,
                        error = %e,
                        "Failed to connect to PostgreSQL config database"
                    );
                    last_error = Some(e);

                    if attempt < source.retry_attempts {
                        tokio::time::sleep(Duration::from_millis(source.retry_delay_ms)).await;
                    }
                }
            }
        }

        if source.optional {
            #[cfg(feature = "tracing")]
            warn!("PostgreSQL config unavailable, continuing with file/env config only");
            Err(PostgresConfigError::Unavailable)
        } else {
            Err(PostgresConfigError::Connection(
                last_error.map_or_else(String::new, |e| e.to_string()),
            ))
        }
    }

    /// Query configuration from the database.
    async fn query_config(
        pool: &PgPool,
        namespace: &str,
    ) -> Result<HashMap<String, serde_json::Value>, PostgresConfigError> {
        let rows = sqlx::query(
            r"
            SELECT key, value
            FROM config_values
            WHERE namespace = $1
            ORDER BY key
            ",
        )
        .bind(namespace)
        .fetch_all(pool)
        .await
        .map_err(|e| PostgresConfigError::Query(e.to_string()))?;

        let mut values = HashMap::with_capacity(rows.len());
        for row in rows {
            let key: String = row
                .try_get("key")
                .map_err(|e| PostgresConfigError::Query(e.to_string()))?;
            let value: serde_json::Value = row
                .try_get("value")
                .map_err(|e| PostgresConfigError::Query(e.to_string()))?;
            values.insert(key, value);
        }

        Ok(values)
    }

    /// Load configuration from fallback file.
    fn load_fallback_file(
        source: &PostgresConfigSource,
    ) -> Result<Option<Self>, PostgresConfigError> {
        let Some(path) = source.fallback_path() else {
            return Ok(None);
        };

        if !path.exists() {
            #[cfg(feature = "tracing")]
            debug!(path = %path.display(), "Fallback file does not exist");
            return Ok(None);
        }

        let content = std::fs::read_to_string(&path)
            .map_err(|e| PostgresConfigError::Fallback(format!("read error: {e}")))?;

        let values: HashMap<String, serde_json::Value> = serde_json::from_str(&content)
            .map_err(|e| PostgresConfigError::Fallback(format!("parse error: {e}")))?;

        #[cfg(feature = "tracing")]
        debug!(
            path = %path.display(),
            keys = values.len(),
            "Loaded fallback config file"
        );

        Ok(Some(Self { values }))
    }

    /// Write configuration to fallback file.
    fn write_fallback_file(&self, source: &PostgresConfigSource) -> Result<(), PostgresConfigError> {
        let Some(path) = source.fallback_path() else {
            return Ok(());
        };

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| PostgresConfigError::Fallback(format!("mkdir error: {e}")))?;
            }
        }

        let content = serde_json::to_string_pretty(&self.values)
            .map_err(|e| PostgresConfigError::Fallback(format!("serialize error: {e}")))?;

        let mut file = std::fs::File::create(&path)
            .map_err(|e| PostgresConfigError::Fallback(format!("create error: {e}")))?;

        file.write_all(content.as_bytes())
            .map_err(|e| PostgresConfigError::Fallback(format!("write error: {e}")))?;

        #[cfg(feature = "tracing")]
        debug!(
            path = %path.display(),
            keys = self.values.len(),
            "Wrote fallback config file"
        );

        Ok(())
    }

    /// Get the fallback mode from the source.
    #[must_use]
    pub fn fallback_mode(source: &PostgresConfigSource) -> FallbackMode {
        source.fallback_mode
    }

    /// Convert to a nested structure suitable for figment.
    ///
    /// Transforms flat dot-notation keys into nested maps:
    /// `"kafka.brokers"` -> `{"kafka": {"brokers": [...]}}`
    #[must_use]
    pub fn to_nested(&self) -> HashMap<String, serde_json::Value> {
        let mut root: HashMap<String, serde_json::Value> = HashMap::new();

        for (key, value) in &self.values {
            insert_nested(&mut root, key, value.clone());
        }

        root
    }

    /// Get a value by key (dot-notation).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.values.get(key)
    }

    /// Check if a key exists.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.values.contains_key(key)
    }

    /// Get all keys.
    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.values.keys()
    }
}

/// Insert a dot-notation key into a nested map.
fn insert_nested(
    map: &mut HashMap<String, serde_json::Value>,
    key: &str,
    value: serde_json::Value,
) {
    let parts: Vec<&str> = key.split('.').collect();

    if parts.len() == 1 {
        map.insert(key.to_string(), value);
        return;
    }

    let first = parts[0];
    let rest = parts[1..].join(".");

    let entry = map
        .entry(first.to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));

    if let serde_json::Value::Object(ref mut obj) = entry {
        let mut inner: HashMap<String, serde_json::Value> =
            obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        insert_nested(&mut inner, &rest, value);
        *obj = inner.into_iter().collect();
    }
}

/// Flatten a nested map to dot-notation keys.
///
/// This is the inverse of `insert_nested`.
#[must_use]
pub fn flatten_nested<S: std::hash::BuildHasher>(
    nested: &HashMap<String, serde_json::Value, S>,
) -> HashMap<String, serde_json::Value> {
    let mut result = HashMap::new();
    flatten_value(&mut result, String::new(), &serde_json::Value::Object(
        nested.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    ));
    result
}

fn flatten_value(
    result: &mut HashMap<String, serde_json::Value>,
    prefix: String,
    value: &serde_json::Value,
) {
    match value {
        serde_json::Value::Object(obj) => {
            for (k, v) in obj {
                let new_key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_value(result, new_key, v);
            }
        }
        _ => {
            if !prefix.is_empty() {
                result.insert(prefix, value.clone());
            }
        }
    }
}

/// Errors from PostgreSQL config loading.
#[derive(Debug, Error)]
pub enum PostgresConfigError {
    /// PostgreSQL config source not configured.
    #[error("PostgreSQL config source not configured")]
    NotConfigured,

    /// PostgreSQL config unavailable (optional, continuing).
    #[error("PostgreSQL config unavailable (optional, continuing)")]
    Unavailable,

    /// PostgreSQL connection error.
    #[error("PostgreSQL connection error: {0}")]
    Connection(String),

    /// PostgreSQL query error.
    #[error("PostgreSQL query error: {0}")]
    Query(String),

    /// Fallback file error.
    #[error("fallback file error: {0}")]
    Fallback(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_postgres_source_default() {
        let source = PostgresConfigSource::default();
        assert!(!source.enabled);
        assert!(source.optional);
        assert_eq!(source.namespace, "default");
        assert_eq!(source.connect_timeout_secs, 5);
        assert_eq!(source.query_timeout_secs, 10);
        assert_eq!(source.retry_attempts, 3);
        assert_eq!(source.retry_delay_ms, 1000);
        assert!(!source.fallback_enabled);
        assert!(source.fallback_file.is_none());
        assert_eq!(source.fallback_mode, FallbackMode::Replace);
    }

    #[test]
    fn test_postgres_source_from_env() {
        std::env::set_var("TESTAPP_CONFIG_POSTGRES_ENABLED", "true");
        std::env::set_var(
            "TESTAPP_CONFIG_POSTGRES_URL",
            "postgres://user:pass@localhost/db",
        );
        std::env::set_var("TESTAPP_CONFIG_POSTGRES_NAMESPACE", "my-app");
        std::env::set_var("TESTAPP_CONFIG_POSTGRES_CONNECT_TIMEOUT", "10");
        std::env::set_var("TESTAPP_CONFIG_POSTGRES_RETRY_ATTEMPTS", "5");
        std::env::set_var("TESTAPP_CONFIG_POSTGRES_OPTIONAL", "false");
        std::env::set_var("TESTAPP_CONFIG_FALLBACK_ENABLED", "true");
        std::env::set_var("TESTAPP_CONFIG_FALLBACK_FILE", "/tmp/fallback.json");
        std::env::set_var("TESTAPP_CONFIG_FALLBACK_MODE", "merge");

        let source = PostgresConfigSource::from_env("TESTAPP");

        assert!(source.enabled);
        assert_eq!(
            source.url,
            Some("postgres://user:pass@localhost/db".to_string())
        );
        assert_eq!(source.namespace, "my-app");
        assert_eq!(source.connect_timeout_secs, 10);
        assert_eq!(source.retry_attempts, 5);
        assert!(!source.optional);
        assert!(source.fallback_enabled);
        assert_eq!(source.fallback_file, Some(PathBuf::from("/tmp/fallback.json")));
        assert_eq!(source.fallback_mode, FallbackMode::Merge);

        std::env::remove_var("TESTAPP_CONFIG_POSTGRES_ENABLED");
        std::env::remove_var("TESTAPP_CONFIG_POSTGRES_URL");
        std::env::remove_var("TESTAPP_CONFIG_POSTGRES_NAMESPACE");
        std::env::remove_var("TESTAPP_CONFIG_POSTGRES_CONNECT_TIMEOUT");
        std::env::remove_var("TESTAPP_CONFIG_POSTGRES_RETRY_ATTEMPTS");
        std::env::remove_var("TESTAPP_CONFIG_POSTGRES_OPTIONAL");
        std::env::remove_var("TESTAPP_CONFIG_FALLBACK_ENABLED");
        std::env::remove_var("TESTAPP_CONFIG_FALLBACK_FILE");
        std::env::remove_var("TESTAPP_CONFIG_FALLBACK_MODE");
    }

    #[test]
    fn test_insert_nested_single_level() {
        let mut map = HashMap::new();
        insert_nested(&mut map, "key", serde_json::json!("value"));
        assert_eq!(map.get("key"), Some(&serde_json::json!("value")));
    }

    #[test]
    fn test_insert_nested_two_levels() {
        let mut map = HashMap::new();
        insert_nested(&mut map, "kafka.brokers", serde_json::json!(["a:9092"]));

        let kafka = map.get("kafka").unwrap().as_object().unwrap();
        assert_eq!(kafka.get("brokers"), Some(&serde_json::json!(["a:9092"])));
    }

    #[test]
    fn test_insert_nested_multiple_keys_same_parent() {
        let mut map = HashMap::new();
        insert_nested(&mut map, "kafka.brokers", serde_json::json!(["a:9092"]));
        insert_nested(&mut map, "kafka.group", serde_json::json!("my-group"));

        let kafka = map.get("kafka").unwrap().as_object().unwrap();
        assert_eq!(kafka.get("brokers"), Some(&serde_json::json!(["a:9092"])));
        assert_eq!(kafka.get("group"), Some(&serde_json::json!("my-group")));
    }

    #[test]
    fn test_insert_nested_deep() {
        let mut map = HashMap::new();
        insert_nested(&mut map, "a.b.c.d", serde_json::json!(42));

        let a = map.get("a").unwrap().as_object().unwrap();
        let b = a.get("b").unwrap().as_object().unwrap();
        let c = b.get("c").unwrap().as_object().unwrap();
        assert_eq!(c.get("d"), Some(&serde_json::json!(42)));
    }

    #[test]
    fn test_postgres_config_to_nested() {
        let mut values = HashMap::new();
        values.insert("kafka.brokers".to_string(), serde_json::json!(["a:9092"]));
        values.insert("kafka.group".to_string(), serde_json::json!("my-group"));
        values.insert("buffer.flush_rows".to_string(), serde_json::json!(5000));
        values.insert("simple_key".to_string(), serde_json::json!("simple_value"));

        let config = PostgresConfig { values };
        let nested = config.to_nested();

        // Check kafka nested
        let kafka = nested.get("kafka").unwrap().as_object().unwrap();
        assert_eq!(kafka.get("brokers"), Some(&serde_json::json!(["a:9092"])));
        assert_eq!(kafka.get("group"), Some(&serde_json::json!("my-group")));

        // Check buffer nested
        let buffer = nested.get("buffer").unwrap().as_object().unwrap();
        assert_eq!(buffer.get("flush_rows"), Some(&serde_json::json!(5000)));

        // Check simple key
        assert_eq!(
            nested.get("simple_key"),
            Some(&serde_json::json!("simple_value"))
        );
    }

    #[test]
    fn test_postgres_config_get() {
        let mut values = HashMap::new();
        values.insert("kafka.brokers".to_string(), serde_json::json!(["a:9092"]));

        let config = PostgresConfig { values };

        assert_eq!(
            config.get("kafka.brokers"),
            Some(&serde_json::json!(["a:9092"]))
        );
        assert!(config.contains("kafka.brokers"));
        assert!(!config.contains("nonexistent"));
    }

    #[tokio::test]
    async fn test_disabled_returns_none() {
        let source = PostgresConfigSource {
            enabled: false,
            ..Default::default()
        };

        let result = PostgresConfig::load(&source).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_no_url_optional_returns_none() {
        let source = PostgresConfigSource {
            enabled: true,
            url: None,
            optional: true,
            ..Default::default()
        };

        let result = PostgresConfig::load(&source).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_no_url_required_returns_error() {
        let source = PostgresConfigSource {
            enabled: true,
            url: None,
            optional: false,
            ..Default::default()
        };

        let result = PostgresConfig::load(&source).await;
        assert!(matches!(result, Err(PostgresConfigError::NotConfigured)));
    }

    #[tokio::test]
    async fn test_invalid_url_optional_returns_none() {
        let source = PostgresConfigSource {
            enabled: true,
            url: Some("postgres://invalid:invalid@localhost:59999/nonexistent".to_string()),
            optional: true,
            retry_attempts: 1,
            connect_timeout_secs: 1,
            ..Default::default()
        };

        let result = PostgresConfig::load(&source).await.unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_flatten_nested_simple() {
        let mut nested = HashMap::new();
        nested.insert("key".to_string(), serde_json::json!("value"));

        let flat = flatten_nested(&nested);
        assert_eq!(flat.get("key"), Some(&serde_json::json!("value")));
    }

    #[test]
    fn test_flatten_nested_deep() {
        let mut nested = HashMap::new();
        nested.insert(
            "kafka".to_string(),
            serde_json::json!({
                "brokers": ["a:9092"],
                "group": "my-group"
            }),
        );
        nested.insert(
            "buffer".to_string(),
            serde_json::json!({
                "flush_rows": 5000
            }),
        );

        let flat = flatten_nested(&nested);
        assert_eq!(
            flat.get("kafka.brokers"),
            Some(&serde_json::json!(["a:9092"]))
        );
        assert_eq!(
            flat.get("kafka.group"),
            Some(&serde_json::json!("my-group"))
        );
        assert_eq!(flat.get("buffer.flush_rows"), Some(&serde_json::json!(5000)));
    }

    #[test]
    fn test_fallback_mode_serde() {
        assert_eq!(
            serde_json::from_str::<FallbackMode>("\"replace\"").unwrap(),
            FallbackMode::Replace
        );
        assert_eq!(
            serde_json::from_str::<FallbackMode>("\"merge\"").unwrap(),
            FallbackMode::Merge
        );
    }

    #[test]
    fn test_fallback_path_disabled() {
        let source = PostgresConfigSource {
            fallback_enabled: false,
            fallback_file: Some(PathBuf::from("/tmp/test.json")),
            ..Default::default()
        };
        assert!(source.fallback_path().is_none());
    }

    #[test]
    fn test_fallback_path_enabled_explicit() {
        let source = PostgresConfigSource {
            fallback_enabled: true,
            fallback_file: Some(PathBuf::from("/tmp/test.json")),
            ..Default::default()
        };
        assert_eq!(source.fallback_path(), Some(PathBuf::from("/tmp/test.json")));
    }

    #[test]
    fn test_fallback_file_roundtrip() {
        let temp_dir = std::env::temp_dir();
        let fallback_path = temp_dir.join("hs-test-fallback.json");

        // Clean up from previous runs
        let _ = std::fs::remove_file(&fallback_path);

        let source = PostgresConfigSource {
            fallback_enabled: true,
            fallback_file: Some(fallback_path.clone()),
            ..Default::default()
        };

        // Create config and write fallback
        let mut values = HashMap::new();
        values.insert("kafka.brokers".to_string(), serde_json::json!(["a:9092"]));
        values.insert("setting".to_string(), serde_json::json!("value"));

        let config = PostgresConfig { values };
        config.write_fallback_file(&source).unwrap();

        // Read it back
        let loaded = PostgresConfig::load_fallback_file(&source).unwrap().unwrap();
        assert_eq!(loaded.values.len(), 2);
        assert_eq!(
            loaded.get("kafka.brokers"),
            Some(&serde_json::json!(["a:9092"]))
        );
        assert_eq!(loaded.get("setting"), Some(&serde_json::json!("value")));

        // Clean up
        let _ = std::fs::remove_file(&fallback_path);
    }
}
