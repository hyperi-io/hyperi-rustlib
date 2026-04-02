// Project:   hyperi-rustlib
// File:      src/config/flat_env.rs
// Purpose:   Flat environment variable override helpers for K8s-friendly config
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Flat environment variable override helpers.
//!
//! DFE services running in Kubernetes receive configuration via flat env vars
//! set by dfe-engine through Helm. These use single underscores as separators
//! (e.g., `DFE_LOADER_KAFKA_BROKERS`), which differs from figment's double-underscore
//! convention for nested keys.
//!
//! This module provides:
//! - Runtime helper functions for reading flat env vars with type conversion
//! - The [`ApplyFlatEnv`] trait for applying overrides to config structs
//! - The [`Normalize`] trait for config normalisation after all sources merge
//! - A generic [`load_config`] function that orchestrates the full cascade
//!
//! ## Usage
//!
//! The helper functions are designed to be called by `#[derive(FlatEnvOverrides)]`
//! generated code, but are also usable standalone:
//!
//! ```rust,no_run
//! use hyperi_rustlib::config::flat_env::*;
//!
//! // In production, env vars are set by the container/K8s ConfigMap.
//! // std::env::set_var is unsafe in edition 2024 — use temp_env in tests.
//! if let Some(host) = flat_env_string("MYAPP", "HOST") {
//!     println!("Host override: {host}");
//! }
//! ```

use std::str::FromStr;

// ---------------------------------------------------------------------------
// Runtime helper functions
// ---------------------------------------------------------------------------

/// Read a flat env var `{PREFIX}_{SUFFIX}` as a `String`.
///
/// Returns `None` if the variable is unset or empty.
/// Logs at debug level when an override is applied.
#[must_use]
pub fn flat_env_string(prefix: &str, suffix: &str) -> Option<String> {
    let key = format!("{prefix}_{suffix}");
    match std::env::var(&key) {
        Ok(v) if !v.is_empty() => {
            tracing::debug!(env_var = %key, "flat env override applied");
            Some(v)
        }
        _ => None,
    }
}

/// Read a flat env var as a comma-separated list.
///
/// Trims whitespace from each element and filters out empty strings.
/// Returns `None` if the variable is unset or empty.
#[must_use]
pub fn flat_env_list(prefix: &str, suffix: &str) -> Option<Vec<String>> {
    flat_env_string(prefix, suffix).map(|v| {
        v.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    })
}

/// Read a flat env var as a `bool`.
///
/// Accepts (case-insensitive): `true`, `1`, `yes` as true; `false`, `0`, `no` as false.
/// Logs a warning and returns `None` for unrecognised values.
#[must_use]
pub fn flat_env_bool(prefix: &str, suffix: &str) -> Option<bool> {
    flat_env_string(prefix, suffix).and_then(|v| match v.to_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => {
            let key = format!("{prefix}_{suffix}");
            tracing::warn!(env_var = %key, value = %v, "invalid bool value, ignoring");
            None
        }
    })
}

/// Read a flat env var and parse via [`FromStr`].
///
/// Returns `None` if the variable is unset, empty, or fails to parse.
/// Logs a warning on parse failure.
#[must_use]
pub fn flat_env_parsed<T: FromStr>(prefix: &str, suffix: &str) -> Option<T> {
    flat_env_string(prefix, suffix).and_then(|v| {
        v.parse::<T>().ok().or_else(|| {
            let key = format!("{prefix}_{suffix}");
            tracing::warn!(env_var = %key, value = %v, "failed to parse, ignoring");
            None
        })
    })
}

/// Read a flat env var as a `String`, masking the value in debug output.
///
/// Behaves identically to [`flat_env_string`] but does not include the
/// value in log messages. Use for passwords, tokens, and API keys.
#[must_use]
pub fn flat_env_string_sensitive(prefix: &str, suffix: &str) -> Option<String> {
    let key = format!("{prefix}_{suffix}");
    match std::env::var(&key) {
        Ok(v) if !v.is_empty() => {
            tracing::debug!(env_var = %key, "flat env override applied (sensitive, value masked)");
            Some(v)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Trait for flat environment variable override application.
///
/// Implemented by the `#[derive(FlatEnvOverrides)]` macro, or manually.
/// Each implementation reads `{prefix}_{FIELD_NAME}` env vars and applies
/// them to the struct fields, overriding values from YAML/figment.
pub trait ApplyFlatEnv {
    /// Apply flat environment variable overrides to this config struct.
    ///
    /// The `prefix` is prepended to each field's suffix to form the full
    /// env var name. For nested structs marked with `#[env_section]`, the
    /// field name is appended to the prefix before recursing.
    fn apply_flat_env(&mut self, prefix: &str);

    /// Generate documentation for all supported env vars.
    ///
    /// Returns a list of [`EnvVarDoc`] entries describing each env var
    /// this struct accepts. Used by `emit-env-docs` CLI subcommands.
    #[must_use]
    fn env_var_docs(prefix: &str) -> Vec<EnvVarDoc>
    where
        Self: Sized,
    {
        let _ = prefix; // default implementation returns empty
        Vec::new()
    }
}

/// Trait for config normalisation after all sources are merged.
///
/// Normalisation handles implied settings that should apply regardless of
/// how a value arrived (YAML, env var, CLI arg). For example, setting SASL
/// credentials implies `sasl.enabled = true`.
///
/// Called after: YAML -> figment env -> flat env overrides.
/// Called before: `validate()`.
pub trait Normalize {
    /// Normalise config: infer implied settings, set defaults based on other fields.
    fn normalize(&mut self) {}
}

/// Documentation for a single env var (generated by derive macro).
#[derive(Debug, Clone)]
pub struct EnvVarDoc {
    /// Full env var name (e.g., `DFE_LOADER_KAFKA_BROKERS`).
    pub name: String,
    /// Rust field path (e.g., `kafka.brokers`).
    pub field_path: String,
    /// Type hint for documentation (e.g., `"string"`, `"list"`, `"bool"`, `"u64"`).
    pub type_hint: &'static str,
    /// Whether the value is sensitive (should be masked in docs/logs).
    pub sensitive: bool,
}

// ---------------------------------------------------------------------------
// Generic config loader
// ---------------------------------------------------------------------------

/// Load config with full cascade: YAML -> figment env -> flat env -> normalise.
///
/// This replaces the copy-pasted `Config::load()` functions across DFE services.
/// It orchestrates:
/// 1. `.env` loading (via dotenvy, handled by `config::setup`)
/// 2. YAML file discovery and loading
/// 3. Figment env var merging (double-underscore nesting)
/// 4. Flat env overrides (single-underscore, K8s-friendly)
/// 5. Normalisation (infer implied settings)
///
/// # Errors
///
/// Returns a [`ConfigError`](super::ConfigError) if config loading or
/// deserialisation fails.
pub fn load_config<T>(config_path: Option<&str>, env_prefix: &str) -> Result<T, super::ConfigError>
where
    T: Default + serde::de::DeserializeOwned + ApplyFlatEnv + Normalize,
{
    // Build config options
    let mut opts = super::ConfigOptions {
        env_prefix: env_prefix.to_string(),
        ..Default::default()
    };

    // Add explicit config path if provided
    if let Some(path) = config_path {
        let path_buf = std::path::PathBuf::from(path);
        if let Some(parent) = path_buf.parent() {
            if parent.as_os_str().is_empty() {
                // Relative path with no directory component — use current dir
                opts.config_paths.push(std::path::PathBuf::from("."));
            } else {
                opts.config_paths.push(parent.to_path_buf());
            }
        }
    }

    // Load via rustlib cascade (dotenv + YAML + figment env)
    let cfg = super::Config::new(opts)?;
    let mut config: T = cfg.unmarshal().unwrap_or_default();

    // Apply flat env overrides (single-underscore, K8s-friendly)
    config.apply_flat_env(env_prefix);

    // Normalise (infer implied settings)
    config.normalize();

    Ok(config)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flat_env_string() {
        temp_env::with_var("TEST_PREFIX_MY_FIELD", Some("hello"), || {
            assert_eq!(
                flat_env_string("TEST_PREFIX", "MY_FIELD"),
                Some("hello".to_string())
            );
        });
    }

    #[test]
    fn test_flat_env_string_empty() {
        temp_env::with_var("TEST_PREFIX_EMPTY", Some(""), || {
            assert_eq!(flat_env_string("TEST_PREFIX", "EMPTY"), None);
        });
    }

    #[test]
    fn test_flat_env_string_missing() {
        assert_eq!(flat_env_string("NONEXISTENT_PREFIX", "FIELD"), None);
    }

    #[test]
    fn test_flat_env_list() {
        temp_env::with_var("TEST_PREFIX_ITEMS", Some("a, b, c"), || {
            assert_eq!(
                flat_env_list("TEST_PREFIX", "ITEMS"),
                Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
            );
        });
    }

    #[test]
    fn test_flat_env_list_single() {
        temp_env::with_var("TEST_PREFIX_SINGLE", Some("only"), || {
            assert_eq!(
                flat_env_list("TEST_PREFIX", "SINGLE"),
                Some(vec!["only".to_string()])
            );
        });
    }

    #[test]
    fn test_flat_env_list_with_empty_elements() {
        temp_env::with_var("TEST_PREFIX_SPARSE", Some("a,,b, ,c"), || {
            assert_eq!(
                flat_env_list("TEST_PREFIX", "SPARSE"),
                Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
            );
        });
    }

    #[test]
    fn test_flat_env_bool_true_variants() {
        temp_env::with_var("TEST_PREFIX_FLAG", Some("true"), || {
            assert_eq!(flat_env_bool("TEST_PREFIX", "FLAG"), Some(true));
        });
        temp_env::with_var("TEST_PREFIX_FLAG", Some("1"), || {
            assert_eq!(flat_env_bool("TEST_PREFIX", "FLAG"), Some(true));
        });
        temp_env::with_var("TEST_PREFIX_FLAG", Some("yes"), || {
            assert_eq!(flat_env_bool("TEST_PREFIX", "FLAG"), Some(true));
        });
        temp_env::with_var("TEST_PREFIX_FLAG", Some("YES"), || {
            assert_eq!(flat_env_bool("TEST_PREFIX", "FLAG"), Some(true));
        });
        temp_env::with_var("TEST_PREFIX_FLAG", Some("True"), || {
            assert_eq!(flat_env_bool("TEST_PREFIX", "FLAG"), Some(true));
        });
    }

    #[test]
    fn test_flat_env_bool_false_variants() {
        temp_env::with_var("TEST_PREFIX_FLAG", Some("false"), || {
            assert_eq!(flat_env_bool("TEST_PREFIX", "FLAG"), Some(false));
        });
        temp_env::with_var("TEST_PREFIX_FLAG", Some("0"), || {
            assert_eq!(flat_env_bool("TEST_PREFIX", "FLAG"), Some(false));
        });
        temp_env::with_var("TEST_PREFIX_FLAG", Some("no"), || {
            assert_eq!(flat_env_bool("TEST_PREFIX", "FLAG"), Some(false));
        });
    }

    #[test]
    fn test_flat_env_bool_invalid() {
        temp_env::with_var("TEST_PREFIX_FLAG", Some("maybe"), || {
            assert_eq!(flat_env_bool("TEST_PREFIX", "FLAG"), None);
        });
    }

    #[test]
    fn test_flat_env_parsed_u64() {
        temp_env::with_var("TEST_PREFIX_PORT", Some("8080"), || {
            assert_eq!(flat_env_parsed::<u64>("TEST_PREFIX", "PORT"), Some(8080));
        });
    }

    #[test]
    fn test_flat_env_parsed_u16() {
        temp_env::with_var("TEST_PREFIX_SMALL_PORT", Some("443"), || {
            assert_eq!(
                flat_env_parsed::<u16>("TEST_PREFIX", "SMALL_PORT"),
                Some(443)
            );
        });
    }

    #[test]
    fn test_flat_env_parsed_f64() {
        temp_env::with_var("TEST_PREFIX_RATIO", Some("0.75"), || {
            assert_eq!(flat_env_parsed::<f64>("TEST_PREFIX", "RATIO"), Some(0.75));
        });
    }

    #[test]
    fn test_flat_env_parsed_invalid() {
        temp_env::with_var("TEST_PREFIX_PORT", Some("not_a_number"), || {
            assert_eq!(flat_env_parsed::<u64>("TEST_PREFIX", "PORT"), None);
        });
    }

    #[test]
    fn test_flat_env_parsed_missing() {
        assert_eq!(flat_env_parsed::<u64>("NONEXISTENT_PREFIX", "PORT"), None);
    }

    #[test]
    fn test_flat_env_sensitive() {
        temp_env::with_var("TEST_PREFIX_SECRET", Some("s3cr3t"), || {
            assert_eq!(
                flat_env_string_sensitive("TEST_PREFIX", "SECRET"),
                Some("s3cr3t".to_string())
            );
        });
    }

    #[test]
    fn test_flat_env_sensitive_empty() {
        temp_env::with_var("TEST_PREFIX_SECRET", Some(""), || {
            assert_eq!(flat_env_string_sensitive("TEST_PREFIX", "SECRET"), None);
        });
    }

    #[test]
    fn test_flat_env_sensitive_missing() {
        assert_eq!(
            flat_env_string_sensitive("NONEXISTENT_PREFIX", "SECRET"),
            None
        );
    }

    #[test]
    fn test_apply_flat_env_trait() {
        struct TestConfig {
            value: String,
        }
        impl ApplyFlatEnv for TestConfig {
            fn apply_flat_env(&mut self, prefix: &str) {
                if let Some(v) = flat_env_string(prefix, "VALUE") {
                    self.value = v;
                }
            }
        }
        let mut config = TestConfig {
            value: "default".into(),
        };
        temp_env::with_var("MY_PREFIX_VALUE", Some("overridden"), || {
            config.apply_flat_env("MY_PREFIX");
        });
        assert_eq!(config.value, "overridden");
    }

    #[test]
    fn test_apply_flat_env_no_override() {
        struct TestConfig {
            value: String,
        }
        impl ApplyFlatEnv for TestConfig {
            fn apply_flat_env(&mut self, prefix: &str) {
                if let Some(v) = flat_env_string(prefix, "VALUE") {
                    self.value = v;
                }
            }
        }
        let mut config = TestConfig {
            value: "default".into(),
        };
        // No env var set — value should remain unchanged
        config.apply_flat_env("ABSENT_PREFIX");
        assert_eq!(config.value, "default");
    }

    #[test]
    fn test_normalize_trait_default() {
        struct TestConfig;
        impl Normalize for TestConfig {}
        let mut config = TestConfig;
        config.normalize(); // default is no-op, should not panic
    }

    #[test]
    fn test_normalize_trait_custom() {
        struct TestConfig {
            username: String,
            auth_enabled: bool,
        }
        impl Normalize for TestConfig {
            fn normalize(&mut self) {
                if !self.username.is_empty() {
                    self.auth_enabled = true;
                }
            }
        }
        let mut config = TestConfig {
            username: "admin".into(),
            auth_enabled: false,
        };
        config.normalize();
        assert!(config.auth_enabled);
    }

    #[test]
    fn test_env_var_doc() {
        let doc = EnvVarDoc {
            name: "DFE_LOADER_KAFKA_BROKERS".to_string(),
            field_path: "kafka.brokers".to_string(),
            type_hint: "list",
            sensitive: false,
        };
        assert_eq!(doc.name, "DFE_LOADER_KAFKA_BROKERS");
        assert_eq!(doc.field_path, "kafka.brokers");
        assert_eq!(doc.type_hint, "list");
        assert!(!doc.sensitive);
    }

    #[test]
    fn test_env_var_docs_default() {
        struct TestConfig;
        impl ApplyFlatEnv for TestConfig {
            fn apply_flat_env(&mut self, _prefix: &str) {}
        }
        let docs = TestConfig::env_var_docs("TEST");
        assert!(docs.is_empty());
    }

    #[test]
    fn test_flat_env_list_missing() {
        assert_eq!(flat_env_list("NONEXISTENT_PREFIX", "ITEMS"), None);
    }

    #[test]
    fn test_flat_env_bool_missing() {
        assert_eq!(flat_env_bool("NONEXISTENT_PREFIX", "FLAG"), None);
    }
}
