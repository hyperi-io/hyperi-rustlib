// Project:   hyperi-rustlib
// File:      src/config/env_compat.rs
// Purpose:   Environment variable compatibility layer with deprecation warnings
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Environment variable compatibility layer.
//!
//! Provides utilities for reading environment variables with support for:
//! - Legacy variable name aliases with deprecation warnings
//! - Standard naming conventions (PG*, KAFKA_*, VAULT_*, AWS_*)
//! - Graceful migration from old to new variable names
//!
//! ## How it works
//!
//! When reading an environment variable:
//! 1. First try the **standard** (preferred) name
//! 2. If not set, try **legacy** (deprecated) names
//! 3. If a legacy name is used, log a deprecation warning
//! 4. Standard name always takes precedence if both are set
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::config::env_compat::EnvVar;
//!
//! // Define a variable with legacy aliases
//! let host = EnvVar::new("PGHOST")
//!     .with_legacy("POSTGRESQL_HOST")
//!     .with_legacy("PG_HOST")
//!     .get();
//!
//! // Or use the builder for multiple variables
//! let vars = EnvVarSet::new("KAFKA")
//!     .var("BOOTSTRAP_SERVERS", &["BROKERS"])
//!     .var("SASL_USERNAME", &["SASL_USER"])
//!     .build();
//! ```

// Allow must_use_candidate for the env var factory functions - they return
// builders that are always meant to be used with .get() or similar methods.
#![allow(clippy::must_use_candidate)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing::warn;

/// Global flag to track if deprecation warnings have been shown.
/// This prevents spamming logs with repeated warnings.
static DEPRECATION_WARNED: AtomicBool = AtomicBool::new(false);

/// Environment variable with optional legacy aliases.
#[derive(Debug, Clone)]
pub struct EnvVar {
    /// Standard (preferred) variable name.
    pub standard: String,
    /// Legacy (deprecated) variable names.
    pub legacy: Vec<String>,
    /// Description for documentation/error messages.
    pub description: Option<String>,
}

impl EnvVar {
    /// Create a new environment variable definition.
    #[must_use]
    pub fn new(standard: &str) -> Self {
        Self {
            standard: standard.to_string(),
            legacy: Vec::new(),
            description: None,
        }
    }

    /// Add a legacy (deprecated) alias.
    #[must_use]
    pub fn with_legacy(mut self, name: &str) -> Self {
        self.legacy.push(name.to_string());
        self
    }

    /// Add multiple legacy aliases.
    #[must_use]
    pub fn with_legacy_names(mut self, names: &[&str]) -> Self {
        for name in names {
            self.legacy.push((*name).to_string());
        }
        self
    }

    /// Add a description.
    #[must_use]
    pub fn with_description(mut self, desc: &str) -> Self {
        self.description = Some(desc.to_string());
        self
    }

    /// Get the value, checking standard name first, then legacy names.
    ///
    /// If a legacy name is used, logs a deprecation warning.
    #[must_use]
    pub fn get(&self) -> Option<String> {
        // Try standard name first
        if let Ok(value) = std::env::var(&self.standard) {
            return Some(value);
        }

        // Try legacy names
        for legacy_name in &self.legacy {
            if let Ok(value) = std::env::var(legacy_name) {
                log_deprecation_warning(legacy_name, &self.standard);
                return Some(value);
            }
        }

        None
    }

    /// Get the value with a default.
    #[must_use]
    pub fn get_or(&self, default: &str) -> String {
        self.get().unwrap_or_else(|| default.to_string())
    }

    /// Get the value, parsing to a specific type.
    pub fn get_parsed<T: std::str::FromStr>(&self) -> Option<T> {
        self.get().and_then(|v| v.parse().ok())
    }

    /// Get the value as a boolean.
    ///
    /// Accepts: "true", "1", "yes", "on" (case-insensitive) as true.
    #[must_use]
    pub fn get_bool(&self) -> Option<bool> {
        self.get().map(|v| {
            let v = v.to_lowercase();
            v == "true" || v == "1" || v == "yes" || v == "on"
        })
    }

    /// Get the value as a comma-separated list.
    #[must_use]
    pub fn get_list(&self) -> Option<Vec<String>> {
        self.get()
            .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
    }

    /// Check which name was used (for debugging).
    #[must_use]
    pub fn which_name_used(&self) -> Option<&str> {
        if std::env::var(&self.standard).is_ok() {
            return Some(&self.standard);
        }
        self.legacy
            .iter()
            .find(|name| std::env::var(name).is_ok())
            .map(String::as_str)
    }
}

/// Log a deprecation warning for a legacy environment variable.
fn log_deprecation_warning(legacy_name: &str, standard_name: &str) {
    // Only warn once per session to avoid log spam
    // Use swap to atomically check and set - returns the previous value
    let already_warned = DEPRECATION_WARNED.swap(true, Ordering::Relaxed);

    if already_warned {
        // Subsequent warnings at debug level
        tracing::debug!(
            legacy = %legacy_name,
            standard = %standard_name,
            "Deprecated environment variable used"
        );
    } else {
        // First warning at warn level
        warn!(
            legacy = %legacy_name,
            standard = %standard_name,
            "Using deprecated environment variable. Please migrate to the standard name."
        );
    }
}

/// Reset the deprecation warning flag (for testing).
#[cfg(test)]
pub fn reset_deprecation_warnings() {
    DEPRECATION_WARNED.store(false, Ordering::Relaxed);
}

// =============================================================================
// Standard Environment Variable Definitions
// =============================================================================

/// PostgreSQL environment variables (libpq standard).
///
/// Uses the standard libpq naming (PGHOST, PGPORT, etc.) with legacy
/// POSTGRESQL_* aliases for backward compatibility.
pub mod postgres {
    use super::EnvVar;

    /// PostgreSQL host.
    pub fn host() -> EnvVar {
        EnvVar::new("PGHOST")
            .with_legacy_names(&["POSTGRESQL_HOST", "PG_HOST", "POSTGRES_HOST"])
            .with_description("PostgreSQL server hostname")
    }

    /// PostgreSQL port.
    pub fn port() -> EnvVar {
        EnvVar::new("PGPORT")
            .with_legacy_names(&["POSTGRESQL_PORT", "PG_PORT", "POSTGRES_PORT"])
            .with_description("PostgreSQL server port")
    }

    /// PostgreSQL user.
    pub fn user() -> EnvVar {
        EnvVar::new("PGUSER")
            .with_legacy_names(&["POSTGRESQL_USER", "PG_USER", "POSTGRES_USER"])
            .with_description("PostgreSQL username")
    }

    /// PostgreSQL password.
    pub fn password() -> EnvVar {
        EnvVar::new("PGPASSWORD")
            .with_legacy_names(&["POSTGRESQL_PASSWORD", "PG_PASSWORD", "POSTGRES_PASSWORD"])
            .with_description("PostgreSQL password")
    }

    /// PostgreSQL database.
    pub fn database() -> EnvVar {
        EnvVar::new("PGDATABASE")
            .with_legacy_names(&[
                "POSTGRESQL_DATABASE",
                "PG_DATABASE",
                "POSTGRES_DATABASE",
                "POSTGRES_DB",
            ])
            .with_description("PostgreSQL database name")
    }

    /// PostgreSQL SSL mode.
    pub fn sslmode() -> EnvVar {
        EnvVar::new("PGSSLMODE")
            .with_legacy_names(&["POSTGRESQL_SSLMODE", "PG_SSLMODE"])
            .with_description("PostgreSQL SSL mode")
    }
}

/// Kafka environment variables.
///
/// Uses KAFKA_* prefix following Confluent conventions.
pub mod kafka {
    use super::EnvVar;

    /// Create a Kafka env var with optional prefix.
    fn kafka_var(name: &str, legacy: &[&str]) -> EnvVar {
        let standard = format!("KAFKA_{name}");
        let mut var = EnvVar::new(&standard);
        for l in legacy {
            var = var.with_legacy(l);
        }
        var
    }

    /// Kafka bootstrap servers.
    pub fn bootstrap_servers() -> EnvVar {
        kafka_var("BOOTSTRAP_SERVERS", &["KAFKA_BROKERS"])
            .with_description("Kafka broker addresses (comma-separated)")
    }

    /// Kafka security protocol.
    pub fn security_protocol() -> EnvVar {
        kafka_var("SECURITY_PROTOCOL", &[])
            .with_description("Security protocol (PLAINTEXT, SSL, SASL_PLAINTEXT, SASL_SSL)")
    }

    /// Kafka SASL mechanism.
    pub fn sasl_mechanism() -> EnvVar {
        kafka_var("SASL_MECHANISM", &[])
            .with_description("SASL mechanism (PLAIN, SCRAM-SHA-256, SCRAM-SHA-512)")
    }

    /// Kafka SASL username.
    pub fn sasl_username() -> EnvVar {
        kafka_var("SASL_USERNAME", &["KAFKA_SASL_USER"]).with_description("SASL username")
    }

    /// Kafka SASL password.
    pub fn sasl_password() -> EnvVar {
        kafka_var("SASL_PASSWORD", &[]).with_description("SASL password")
    }

    /// Kafka consumer group ID.
    pub fn group_id() -> EnvVar {
        kafka_var("GROUP_ID", &["KAFKA_GROUP", "KAFKA_CONSUMER_GROUP"])
            .with_description("Consumer group ID")
    }

    /// Kafka client ID.
    pub fn client_id() -> EnvVar {
        kafka_var("CLIENT_ID", &[]).with_description("Client ID for broker logs")
    }

    /// Kafka topics (comma-separated).
    pub fn topics() -> EnvVar {
        kafka_var("TOPICS", &["KAFKA_TOPIC"])
            .with_description("Topics to subscribe to (comma-separated)")
    }

    /// Kafka SSL CA location.
    pub fn ssl_ca_location() -> EnvVar {
        kafka_var("SSL_CA_LOCATION", &["KAFKA_CA_CERT", "KAFKA_SSL_CA"])
            .with_description("Path to SSL CA certificate")
    }

    /// Kafka SSL skip verify.
    pub fn ssl_skip_verify() -> EnvVar {
        kafka_var("SSL_SKIP_VERIFY", &["KAFKA_SSL_INSECURE", "KAFKA_INSECURE"])
            .with_description("Skip SSL certificate verification")
    }

    /// Kafka profile (production, devtest).
    pub fn profile() -> EnvVar {
        kafka_var("PROFILE", &[]).with_description("Kafka profile (production, devtest)")
    }

    /// Create a prefixed Kafka env var.
    ///
    /// For custom prefixes like `MYAPP_KAFKA_BOOTSTRAP_SERVERS`.
    pub fn with_prefix(prefix: &str, name: &str) -> EnvVar {
        EnvVar::new(&format!("{prefix}_KAFKA_{name}")).with_legacy(&format!("{prefix}_{name}"))
    }
}

/// Vault/OpenBao environment variables.
///
/// Uses standard VAULT_* naming (HashiCorp convention).
pub mod vault {
    use super::EnvVar;

    /// Vault address.
    pub fn addr() -> EnvVar {
        EnvVar::new("VAULT_ADDR")
            .with_legacy_names(&["OPENBAO_ADDR", "BAO_ADDR"])
            .with_description("Vault/OpenBao server address")
    }

    /// Vault token.
    pub fn token() -> EnvVar {
        EnvVar::new("VAULT_TOKEN")
            .with_legacy_names(&["OPENBAO_TOKEN", "BAO_TOKEN", "OPENBAO_ROOT_TOKEN"])
            .with_description("Vault/OpenBao authentication token")
    }

    /// Vault namespace (Enterprise feature).
    pub fn namespace() -> EnvVar {
        EnvVar::new("VAULT_NAMESPACE")
            .with_legacy_names(&["OPENBAO_NAMESPACE", "BAO_NAMESPACE"])
            .with_description("Vault namespace (Enterprise)")
    }

    /// Vault skip TLS verification.
    pub fn skip_verify() -> EnvVar {
        EnvVar::new("VAULT_SKIP_VERIFY")
            .with_legacy_names(&[
                "OPENBAO_SKIP_VERIFY",
                "BAO_SKIP_VERIFY",
                "VAULT_TLS_SKIP_VERIFY",
            ])
            .with_description("Skip TLS certificate verification")
    }

    /// Vault CA certificate path.
    pub fn ca_cert() -> EnvVar {
        EnvVar::new("VAULT_CACERT")
            .with_legacy_names(&["OPENBAO_CACERT", "BAO_CACERT", "VAULT_CA_CERT"])
            .with_description("Path to CA certificate for Vault TLS")
    }

    /// AppRole role ID.
    pub fn approle_role_id() -> EnvVar {
        EnvVar::new("VAULT_ROLE_ID")
            .with_legacy_names(&["OPENBAO_ROLE_ID", "BAO_ROLE_ID"])
            .with_description("AppRole role ID")
    }

    /// AppRole secret ID.
    pub fn approle_secret_id() -> EnvVar {
        EnvVar::new("VAULT_SECRET_ID")
            .with_legacy_names(&["OPENBAO_SECRET_ID", "BAO_SECRET_ID"])
            .with_description("AppRole secret ID")
    }

    /// Kubernetes auth role.
    pub fn k8s_role() -> EnvVar {
        EnvVar::new("VAULT_K8S_ROLE")
            .with_legacy_names(&["OPENBAO_K8S_ROLE", "BAO_K8S_ROLE"])
            .with_description("Kubernetes auth role name")
    }
}

/// AWS environment variables (official SDK naming).
pub mod aws {
    use super::EnvVar;

    /// AWS access key ID.
    pub fn access_key_id() -> EnvVar {
        EnvVar::new("AWS_ACCESS_KEY_ID")
            .with_legacy_names(&["AWS_ACCESS_KEY"])
            .with_description("AWS access key ID")
    }

    /// AWS secret access key.
    pub fn secret_access_key() -> EnvVar {
        EnvVar::new("AWS_SECRET_ACCESS_KEY")
            .with_legacy_names(&["AWS_SECRET_KEY"])
            .with_description("AWS secret access key")
    }

    /// AWS session token.
    pub fn session_token() -> EnvVar {
        EnvVar::new("AWS_SESSION_TOKEN")
            .with_legacy_names(&["AWS_SECURITY_TOKEN"])
            .with_description("AWS session token (for temporary credentials)")
    }

    /// AWS region.
    pub fn region() -> EnvVar {
        EnvVar::new("AWS_DEFAULT_REGION")
            .with_legacy_names(&["AWS_REGION"])
            .with_description("AWS region")
    }

    /// AWS endpoint URL (for LocalStack or custom endpoints).
    pub fn endpoint_url() -> EnvVar {
        EnvVar::new("AWS_ENDPOINT_URL")
            .with_legacy_names(&["AWS_ENDPOINT", "LOCALSTACK_ENDPOINT"])
            .with_description("Custom AWS endpoint URL")
    }
}

/// ClickHouse environment variables.
pub mod clickhouse {
    use super::EnvVar;

    /// ClickHouse host.
    pub fn host() -> EnvVar {
        EnvVar::new("CLICKHOUSE_HOST")
            .with_legacy_names(&["CH_HOST"])
            .with_description("ClickHouse server hostname")
    }

    /// ClickHouse native protocol port.
    pub fn native_port() -> EnvVar {
        EnvVar::new("CLICKHOUSE_NATIVE_PORT")
            .with_legacy_names(&["CLICKHOUSE_PORT", "CH_PORT"])
            .with_description("ClickHouse native protocol port (default: 9000)")
    }

    /// ClickHouse HTTP port.
    pub fn http_port() -> EnvVar {
        EnvVar::new("CLICKHOUSE_HTTP_PORT")
            .with_legacy_names(&["CH_HTTP_PORT"])
            .with_description("ClickHouse HTTP port (default: 8123)")
    }

    /// ClickHouse user.
    pub fn user() -> EnvVar {
        EnvVar::new("CLICKHOUSE_USER")
            .with_legacy_names(&["CH_USER", "CLICKHOUSE_USERNAME"])
            .with_description("ClickHouse username")
    }

    /// ClickHouse password.
    pub fn password() -> EnvVar {
        EnvVar::new("CLICKHOUSE_PASSWORD")
            .with_legacy_names(&["CH_PASSWORD"])
            .with_description("ClickHouse password")
    }

    /// ClickHouse database.
    pub fn database() -> EnvVar {
        EnvVar::new("CLICKHOUSE_DATABASE")
            .with_legacy_names(&["CH_DATABASE", "CLICKHOUSE_DB"])
            .with_description("ClickHouse database name")
    }
}

/// Load all standard environment variables into a HashMap.
///
/// This is useful for debugging or logging which variables are set.
#[must_use]
pub fn load_all_standard() -> HashMap<String, Option<String>> {
    let mut vars = HashMap::new();

    // PostgreSQL
    vars.insert("pg.host".into(), postgres::host().get());
    vars.insert("pg.port".into(), postgres::port().get());
    vars.insert("pg.user".into(), postgres::user().get());
    vars.insert("pg.database".into(), postgres::database().get());

    // Kafka
    vars.insert(
        "kafka.bootstrap_servers".into(),
        kafka::bootstrap_servers().get(),
    );
    vars.insert(
        "kafka.security_protocol".into(),
        kafka::security_protocol().get(),
    );
    vars.insert("kafka.sasl_mechanism".into(), kafka::sasl_mechanism().get());
    vars.insert("kafka.sasl_username".into(), kafka::sasl_username().get());

    // Vault
    vars.insert("vault.addr".into(), vault::addr().get());
    vars.insert("vault.namespace".into(), vault::namespace().get());

    // AWS
    vars.insert("aws.region".into(), aws::region().get());

    // ClickHouse
    vars.insert("clickhouse.host".into(), clickhouse::host().get());
    vars.insert("clickhouse.database".into(), clickhouse::database().get());

    vars
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Env var mutations are not thread-safe. Serialise all tests that
    // call set_var/remove_var to prevent parallel test interference.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn setup() {
        reset_deprecation_warnings();
    }

    #[test]
    fn test_env_var_standard_name() {
        let _lock = ENV_LOCK.lock().unwrap();
        setup();
        temp_env::with_var("TEST_STANDARD_VAR", Some("standard_value"), || {
            let var = EnvVar::new("TEST_STANDARD_VAR").with_legacy("TEST_LEGACY_VAR");
            assert_eq!(var.get(), Some("standard_value".to_string()));
        });
    }

    #[test]
    fn test_env_var_legacy_fallback() {
        let _lock = ENV_LOCK.lock().unwrap();
        setup();
        temp_env::with_var("TEST_LEGACY_VAR2", Some("legacy_value"), || {
            let var = EnvVar::new("TEST_STANDARD_VAR2").with_legacy("TEST_LEGACY_VAR2");
            assert_eq!(var.get(), Some("legacy_value".to_string()));
        });
    }

    #[test]
    fn test_env_var_standard_takes_precedence() {
        let _lock = ENV_LOCK.lock().unwrap();
        setup();
        temp_env::with_vars(
            [
                ("TEST_STANDARD_VAR3", Some("standard")),
                ("TEST_LEGACY_VAR3", Some("legacy")),
            ],
            || {
                let var = EnvVar::new("TEST_STANDARD_VAR3").with_legacy("TEST_LEGACY_VAR3");
                assert_eq!(var.get(), Some("standard".to_string()));
            },
        );
    }

    #[test]
    fn test_env_var_missing() {
        let _lock = ENV_LOCK.lock().unwrap();
        setup();
        let var = EnvVar::new("NONEXISTENT_VAR").with_legacy("ALSO_NONEXISTENT");
        assert_eq!(var.get(), None);
    }

    #[test]
    fn test_env_var_get_bool() {
        let _lock = ENV_LOCK.lock().unwrap();
        setup();
        temp_env::with_vars(
            [
                ("TEST_BOOL_TRUE", Some("true")),
                ("TEST_BOOL_ONE", Some("1")),
                ("TEST_BOOL_YES", Some("YES")),
                ("TEST_BOOL_FALSE", Some("false")),
            ],
            || {
                assert_eq!(EnvVar::new("TEST_BOOL_TRUE").get_bool(), Some(true));
                assert_eq!(EnvVar::new("TEST_BOOL_ONE").get_bool(), Some(true));
                assert_eq!(EnvVar::new("TEST_BOOL_YES").get_bool(), Some(true));
                assert_eq!(EnvVar::new("TEST_BOOL_FALSE").get_bool(), Some(false));
            },
        );
    }

    #[test]
    fn test_env_var_get_list() {
        let _lock = ENV_LOCK.lock().unwrap();
        setup();
        temp_env::with_var("TEST_LIST", Some("a, b, c"), || {
            let var = EnvVar::new("TEST_LIST");
            assert_eq!(
                var.get_list(),
                Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
            );
        });
    }

    #[test]
    fn test_postgres_env_vars() {
        let _lock = ENV_LOCK.lock().unwrap();
        setup();
        temp_env::with_var("PGHOST", Some("localhost"), || {
            assert_eq!(postgres::host().get(), Some("localhost".to_string()));
        });
    }

    #[test]
    fn test_postgres_legacy_fallback() {
        let _lock = ENV_LOCK.lock().unwrap();
        setup();
        temp_env::with_vars(
            [
                ("PGHOST", None::<&str>),
                ("POSTGRESQL_HOST", Some("legacy-host")),
            ],
            || assert_eq!(postgres::host().get(), Some("legacy-host".to_string())),
        );
    }

    #[test]
    fn test_kafka_env_vars() {
        let _lock = ENV_LOCK.lock().unwrap();
        setup();
        temp_env::with_var("KAFKA_BOOTSTRAP_SERVERS", Some("kafka:9092"), || {
            assert_eq!(
                kafka::bootstrap_servers().get(),
                Some("kafka:9092".to_string())
            );
        });
    }

    #[test]
    fn test_vault_env_vars() {
        let _lock = ENV_LOCK.lock().unwrap();
        setup();
        temp_env::with_var("VAULT_ADDR", Some("https://vault:8200"), || {
            assert_eq!(vault::addr().get(), Some("https://vault:8200".to_string()));
        });
    }

    #[test]
    fn test_vault_openbao_fallback() {
        let _lock = ENV_LOCK.lock().unwrap();
        setup();
        temp_env::with_vars(
            [
                ("VAULT_ADDR", None::<&str>),
                ("OPENBAO_ADDR", Some("https://openbao:8200")),
            ],
            || {
                assert_eq!(
                    vault::addr().get(),
                    Some("https://openbao:8200".to_string())
                );
            },
        );
    }

    #[test]
    fn test_which_name_used() {
        let _lock = ENV_LOCK.lock().unwrap();
        setup();
        temp_env::with_var("TEST_WHICH_LEGACY", Some("value"), || {
            let var = EnvVar::new("TEST_WHICH_STANDARD").with_legacy("TEST_WHICH_LEGACY");
            assert_eq!(var.which_name_used(), Some("TEST_WHICH_LEGACY"));
        });
    }
}
