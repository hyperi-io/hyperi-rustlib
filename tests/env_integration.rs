// Project:   hyperi-rustlib
// File:      tests/env_integration.rs
// Purpose:   Integration tests for environment variable loading
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

#![allow(unsafe_code)]

//! Integration tests for environment variable loading and .env cascade.
//!
//! These tests verify that:
//! - Standard ENV variable names are correctly loaded
//! - Legacy ENV aliases work with deprecation warnings
//! - The .env file cascade (home + project) works correctly

use std::sync::Mutex;

// Guard to prevent concurrent test interference with environment variables
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Helper to set/unset environment variables for testing.
struct EnvGuard {
    vars: Vec<String>,
}

impl EnvGuard {
    fn new(vars: &[(&str, &str)]) -> Self {
        for (name, value) in vars {
            // SAFETY: single-threaded test setup, ENV_LOCK held by caller
            unsafe { std::env::set_var(name, value) };
        }
        Self {
            vars: vars.iter().map(|(name, _)| name.to_string()).collect(),
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for var in &self.vars {
            // SAFETY: single-threaded test teardown, ENV_LOCK held by caller
            unsafe { std::env::remove_var(var) };
        }
    }
}

// =============================================================================
// Kafka ENV Loading Tests
// =============================================================================

#[cfg(feature = "transport-kafka")]
mod kafka_env {
    use super::*;
    use hyperi_rustlib::transport::kafka::KafkaConfig;

    #[test]
    fn test_kafka_from_env_standard_names() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            ("KAFKA_BOOTSTRAP_SERVERS", "broker1:9092,broker2:9092"),
            ("KAFKA_SASL_USERNAME", "testuser"),
            ("KAFKA_SASL_PASSWORD", "testpass"),
            ("KAFKA_SECURITY_PROTOCOL", "SASL_SSL"),
            ("KAFKA_SASL_MECHANISM", "SCRAM-SHA-512"),
            ("KAFKA_GROUP_ID", "test-group"),
            ("KAFKA_TOPICS", "topic1,topic2"),
        ]);

        let config = KafkaConfig::from_env_standard();

        assert_eq!(config.brokers, vec!["broker1:9092", "broker2:9092"]);
        assert_eq!(config.sasl_username, Some("testuser".to_string()));
        assert_eq!(config.sasl_password, Some("testpass".to_string()));
        assert_eq!(config.security_protocol, "SASL_SSL");
        assert_eq!(config.sasl_mechanism, Some("SCRAM-SHA-512".to_string()));
        assert_eq!(config.group, "test-group");
        assert_eq!(config.topics, vec!["topic1", "topic2"]);
    }

    #[test]
    fn test_kafka_from_env_legacy_brokers() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            ("KAFKA_BROKERS", "legacy-broker:9092"), // Legacy name
        ]);

        let config = KafkaConfig::from_env_standard();

        // Should fall back to legacy KAFKA_BROKERS
        assert_eq!(config.brokers, vec!["legacy-broker:9092"]);
    }

    #[test]
    fn test_kafka_from_env_legacy_sasl_user() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            ("KAFKA_SASL_USER", "legacy-user"), // Legacy name
        ]);

        let config = KafkaConfig::from_env_standard();

        // Should fall back to legacy KAFKA_SASL_USER
        assert_eq!(config.sasl_username, Some("legacy-user".to_string()));
    }

    #[test]
    fn test_kafka_from_env_standard_wins_over_legacy() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            ("KAFKA_BOOTSTRAP_SERVERS", "standard:9092"),
            ("KAFKA_BROKERS", "legacy:9092"), // Should be ignored
        ]);

        let config = KafkaConfig::from_env_standard();

        // Standard name should win
        assert_eq!(config.brokers, vec!["standard:9092"]);
    }

    #[test]
    fn test_kafka_from_env_with_prefix() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            ("MYAPP_BOOTSTRAP_SERVERS", "prefixed:9092"),
            ("MYAPP_GROUP_ID", "prefixed-group"),
        ]);

        let config = KafkaConfig::from_env("MYAPP");

        assert_eq!(config.brokers, vec!["prefixed:9092"]);
        assert_eq!(config.group, "prefixed-group");
    }

    #[test]
    fn test_kafka_from_env_ssl_skip_verify() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[("KAFKA_SSL_SKIP_VERIFY", "true")]);

        let config = KafkaConfig::from_env_standard();

        assert!(config.ssl_skip_verify);
    }

    #[test]
    fn test_kafka_from_env_profile() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[("KAFKA_PROFILE", "devtest")]);

        let config = KafkaConfig::from_env_standard();

        assert_eq!(
            config.profile,
            hyperi_rustlib::transport::kafka::KafkaProfile::DevTest
        );
        // DevTest should auto-enable ssl_skip_verify
        assert!(config.ssl_skip_verify);
    }
}

// =============================================================================
// Vault/OpenBao ENV Loading Tests
// =============================================================================

#[cfg(feature = "secrets-vault")]
mod vault_env {
    use super::*;
    use hyperi_rustlib::secrets::{OpenBaoAuth, OpenBaoConfig};

    /// All vault/openbao env vars that could interfere with tests.
    /// Must be cleared before each test to prevent leakage from the host.
    const VAULT_ENV_VARS: &[&str] = &[
        "VAULT_ADDR",
        "VAULT_TOKEN",
        "VAULT_SKIP_VERIFY",
        "VAULT_NAMESPACE",
        "VAULT_ROLE_ID",
        "VAULT_SECRET_ID",
        "VAULT_K8S_ROLE",
        "VAULT_K8S_MOUNT",
        "OPENBAO_ADDR",
        "OPENBAO_TOKEN",
        "BAO_ADDR",
        "BAO_TOKEN",
        "OPENBAO_ROOT_TOKEN",
    ];

    /// Clear all vault-related env vars so tests start from a clean slate.
    fn clear_vault_env() {
        for var in VAULT_ENV_VARS {
            // SAFETY: single-threaded test teardown, ENV_LOCK held by caller
            unsafe { std::env::remove_var(var) };
        }
    }

    #[test]
    fn test_vault_from_env_token_auth() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_vault_env();
        let _guard = EnvGuard::new(&[
            ("VAULT_ADDR", "https://vault.example.com:8200"),
            ("VAULT_TOKEN", "s.test-token"),
        ]);

        let config = OpenBaoConfig::from_env().expect("Should load from env");

        assert_eq!(config.address, "https://vault.example.com:8200");
        assert!(matches!(config.auth, OpenBaoAuth::Token { token } if token == "s.test-token"));
    }

    #[test]
    fn test_vault_from_env_approle_auth() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_vault_env();
        let _guard = EnvGuard::new(&[
            ("VAULT_ADDR", "https://vault.example.com:8200"),
            ("VAULT_ROLE_ID", "role-123"),
            ("VAULT_SECRET_ID", "secret-456"),
        ]);

        let config = OpenBaoConfig::from_env().expect("Should load from env");

        assert!(matches!(
            config.auth,
            OpenBaoAuth::AppRole {
                role_id,
                secret_id,
                ..
            } if role_id == "role-123" && secret_id == "secret-456"
        ));
    }

    #[test]
    fn test_vault_from_env_k8s_auth() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_vault_env();
        let _guard = EnvGuard::new(&[
            ("VAULT_ADDR", "https://vault.example.com:8200"),
            ("VAULT_K8S_ROLE", "my-k8s-role"),
        ]);

        let config = OpenBaoConfig::from_env().expect("Should load from env");

        assert!(matches!(
            config.auth,
            OpenBaoAuth::Kubernetes { role, .. } if role == "my-k8s-role"
        ));
    }

    #[test]
    fn test_vault_from_env_openbao_fallback() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_vault_env();
        let _guard = EnvGuard::new(&[
            ("OPENBAO_ADDR", "https://openbao:8200"), // Legacy name
            ("OPENBAO_TOKEN", "s.openbao-token"),     // Legacy name
        ]);

        let config = OpenBaoConfig::from_env().expect("Should load from env");

        assert_eq!(config.address, "https://openbao:8200");
        assert!(matches!(config.auth, OpenBaoAuth::Token { token } if token == "s.openbao-token"));
    }

    #[test]
    fn test_vault_from_env_vault_wins_over_openbao() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_vault_env();
        let _guard = EnvGuard::new(&[
            ("VAULT_ADDR", "https://vault-wins:8200"),
            ("OPENBAO_ADDR", "https://openbao-loses:8200"),
            ("VAULT_TOKEN", "vault-token"),
            ("OPENBAO_TOKEN", "openbao-token"),
        ]);

        let config = OpenBaoConfig::from_env().expect("Should load from env");

        // VAULT_* should win
        assert_eq!(config.address, "https://vault-wins:8200");
        assert!(matches!(config.auth, OpenBaoAuth::Token { token } if token == "vault-token"));
    }

    #[test]
    fn test_vault_from_env_skip_verify() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_vault_env();
        let _guard = EnvGuard::new(&[
            ("VAULT_ADDR", "https://vault:8200"),
            ("VAULT_TOKEN", "test"),
            ("VAULT_SKIP_VERIFY", "true"),
        ]);

        let config = OpenBaoConfig::from_env().expect("Should load from env");

        assert!(config.skip_verify);
    }

    #[test]
    fn test_vault_from_env_namespace() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_vault_env();
        let _guard = EnvGuard::new(&[
            ("VAULT_ADDR", "https://vault:8200"),
            ("VAULT_TOKEN", "test"),
            ("VAULT_NAMESPACE", "hypersec"),
        ]);

        let config = OpenBaoConfig::from_env().expect("Should load from env");

        assert_eq!(config.namespace, Some("hypersec".to_string()));
    }

    #[test]
    fn test_vault_from_env_missing_addr() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_vault_env();
        let _guard = EnvGuard::new(&[("VAULT_TOKEN", "test")]); // No VAULT_ADDR

        let config = OpenBaoConfig::from_env();

        assert!(config.is_none());
    }

    #[test]
    fn test_vault_from_env_missing_auth() {
        let _lock = ENV_LOCK.lock().unwrap();
        clear_vault_env();
        let _guard = EnvGuard::new(&[("VAULT_ADDR", "https://vault:8200")]); // No auth

        let config = OpenBaoConfig::from_env();

        assert!(config.is_none());
    }
}

// =============================================================================
// AWS ENV Loading Tests
// =============================================================================

#[cfg(feature = "secrets-aws")]
mod aws_env {
    use super::*;
    use hyperi_rustlib::secrets::AwsConfig;

    #[test]
    fn test_aws_from_env_region() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[("AWS_DEFAULT_REGION", "eu-west-1")]);

        let config = AwsConfig::from_env();

        assert_eq!(config.region, "eu-west-1");
    }

    #[test]
    fn test_aws_from_env_legacy_region() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[("AWS_REGION", "ap-southeast-2")]); // Legacy

        let config = AwsConfig::from_env();

        assert_eq!(config.region, "ap-southeast-2");
    }

    #[test]
    fn test_aws_from_env_endpoint() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[("AWS_ENDPOINT_URL", "http://localhost:4566")]);

        let config = AwsConfig::from_env();

        assert_eq!(
            config.endpoint_url,
            Some("http://localhost:4566".to_string())
        );
    }

    #[test]
    fn test_aws_from_env_default_region() {
        let _lock = ENV_LOCK.lock().unwrap();
        // Clear any region vars so we get the hard-coded default
        let saved_default = std::env::var("AWS_DEFAULT_REGION").ok();
        let saved_legacy = std::env::var("AWS_REGION").ok();
        // SAFETY: ENV_LOCK held, single-threaded test
        unsafe {
            std::env::remove_var("AWS_DEFAULT_REGION");
            std::env::remove_var("AWS_REGION");
        }

        let config = AwsConfig::from_env();

        assert_eq!(config.region, "us-east-1");

        // Restore
        // SAFETY: ENV_LOCK held, single-threaded test
        unsafe {
            if let Some(v) = saved_default {
                std::env::set_var("AWS_DEFAULT_REGION", v);
            }
            if let Some(v) = saved_legacy {
                std::env::set_var("AWS_REGION", v);
            }
        }
    }
}

// =============================================================================
// env_compat Module Tests
// =============================================================================

#[cfg(feature = "config")]
mod env_compat_tests {
    use super::*;
    use hyperi_rustlib::config::env_compat::{self, EnvVar};

    #[test]
    fn test_postgres_standard_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            ("PGHOST", "pg-standard"),
            ("PGPORT", "5432"),
            ("PGUSER", "postgres"),
            ("PGDATABASE", "mydb"),
        ]);

        assert_eq!(
            env_compat::postgres::host().get(),
            Some("pg-standard".to_string())
        );
        assert_eq!(env_compat::postgres::port().get(), Some("5432".to_string()));
        assert_eq!(
            env_compat::postgres::user().get(),
            Some("postgres".to_string())
        );
        assert_eq!(
            env_compat::postgres::database().get(),
            Some("mydb".to_string())
        );
    }

    #[test]
    fn test_postgres_legacy_fallback() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            ("POSTGRESQL_HOST", "pg-legacy"),
            ("POSTGRESQL_PORT", "5433"),
        ]);

        assert_eq!(
            env_compat::postgres::host().get(),
            Some("pg-legacy".to_string())
        );
        assert_eq!(env_compat::postgres::port().get(), Some("5433".to_string()));
    }

    #[test]
    fn test_clickhouse_env() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[
            ("CLICKHOUSE_HOST", "clickhouse.local"),
            ("CLICKHOUSE_DATABASE", "events"),
        ]);

        assert_eq!(
            env_compat::clickhouse::host().get(),
            Some("clickhouse.local".to_string())
        );
        assert_eq!(
            env_compat::clickhouse::database().get(),
            Some("events".to_string())
        );
    }

    #[test]
    fn test_env_var_get_bool_variants() {
        let _lock = ENV_LOCK.lock().unwrap();

        // SAFETY: ENV_LOCK held, single-threaded test
        unsafe {
            // Test "true"
            std::env::set_var("TEST_BOOL_1", "true");
            assert_eq!(EnvVar::new("TEST_BOOL_1").get_bool(), Some(true));

            // Test "1"
            std::env::set_var("TEST_BOOL_2", "1");
            assert_eq!(EnvVar::new("TEST_BOOL_2").get_bool(), Some(true));

            // Test "yes"
            std::env::set_var("TEST_BOOL_3", "YES");
            assert_eq!(EnvVar::new("TEST_BOOL_3").get_bool(), Some(true));

            // Test "on"
            std::env::set_var("TEST_BOOL_4", "on");
            assert_eq!(EnvVar::new("TEST_BOOL_4").get_bool(), Some(true));

            // Test "false"
            std::env::set_var("TEST_BOOL_5", "false");
            assert_eq!(EnvVar::new("TEST_BOOL_5").get_bool(), Some(false));

            // Cleanup
            for i in 1..=5 {
                std::env::remove_var(format!("TEST_BOOL_{i}"));
            }
        }
    }

    #[test]
    fn test_env_var_get_list() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::new(&[("TEST_LIST", "a, b, c, d")]);

        let result = EnvVar::new("TEST_LIST").get_list();
        assert_eq!(
            result,
            Some(vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string()
            ])
        );
    }

    #[test]
    fn test_env_var_which_name_used() {
        let _lock = ENV_LOCK.lock().unwrap();

        // SAFETY: ENV_LOCK held, single-threaded test
        unsafe {
            // Set only legacy
            std::env::set_var("LEGACY_VAR_TEST", "value");
            let var = EnvVar::new("STANDARD_VAR_TEST").with_legacy("LEGACY_VAR_TEST");
            assert_eq!(var.which_name_used(), Some("LEGACY_VAR_TEST"));
            std::env::remove_var("LEGACY_VAR_TEST");

            // Set standard
            std::env::set_var("STANDARD_VAR_TEST", "value");
            let var = EnvVar::new("STANDARD_VAR_TEST").with_legacy("LEGACY_VAR_TEST");
            assert_eq!(var.which_name_used(), Some("STANDARD_VAR_TEST"));
            std::env::remove_var("STANDARD_VAR_TEST");
        }
    }
}
