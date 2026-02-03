// Project:   hs-rustlib
// File:      tests/parity/config_parity.rs
// Purpose:   Config parity tests against hs-pylib
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Configuration cascade parity tests.
//!
//! These tests verify that the 7-layer cascade behaves identically
//! to hs-pylib's config package.
//!
//! ## Cascade Priority (high to low)
//!
//! 1. CLI args/switches          → --host=X (runtime override)
//! 2. ENV variables              → MYAPP_DATABASE_HOST (deployment)
//! 3. .env file                  → Local secrets (gitignored)
//! 4. settings.{env}.yaml        → Environment-specific (production/staging)
//! 5. settings.yaml              → Project base defaults
//! 6. defaults.yaml              → Safe fallback defaults
//! 7. Hard-coded                 → Last resort in code

use hs_rustlib::config::{Config, ConfigOptions};
use std::fs;
use tempfile::TempDir;

/// Guard to cleanup environment variables on drop.
struct EnvGuard {
    vars: Vec<String>,
}

impl EnvGuard {
    fn new(vars: &[(&str, &str)]) -> Self {
        for (k, v) in vars {
            std::env::set_var(k, v);
        }
        Self {
            vars: vars.iter().map(|(k, _)| k.to_string()).collect(),
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for var in &self.vars {
            std::env::remove_var(var);
        }
    }
}

/// Create a test config directory with the full cascade.
fn setup_config_dir() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().expect("failed to create temp dir");
    let path = dir.path().to_path_buf();

    // Layer 6: defaults.yaml (lowest file priority)
    fs::write(
        path.join("defaults.yaml"),
        r#"
log_level: debug
database:
  host: default-host
  port: 5432
  username: default-user
  timeout: 30s
cache:
  enabled: false
"#,
    )
    .expect("failed to write defaults.yaml");

    // Layer 5: settings.yaml (base settings)
    fs::write(
        path.join("settings.yaml"),
        r#"
app_name: test_app
database:
  host: settings-host
  username: settings-user
feature_flags:
  - flag_a
  - flag_b
"#,
    )
    .expect("failed to write settings.yaml");

    // Layer 4: settings.development.yaml (env-specific)
    fs::write(
        path.join("settings.development.yaml"),
        r#"
debug: true
database:
  host: dev-host
  password: dev-password
cache:
  enabled: true
  ttl: 60
"#,
    )
    .expect("failed to write settings.development.yaml");

    // Layer 4: settings.production.yaml (env-specific)
    fs::write(
        path.join("settings.production.yaml"),
        r#"
debug: false
database:
  host: prod-host
  password: prod-password
cache:
  enabled: true
  ttl: 3600
"#,
    )
    .expect("failed to write settings.production.yaml");

    (dir, path)
}

// ============================================================================
// Layer 7: Hard-coded defaults tests
// ============================================================================

#[test]
fn test_hardcoded_defaults_loaded() {
    // Use an empty temp directory to ensure no config files interfere
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let empty_path = temp_dir.path().to_path_buf();

    // Only search the empty temp directory (no defaults.yaml or settings.yaml present)
    let config = Config::new(ConfigOptions {
        config_paths: vec![empty_path],
        load_dotenv: false,
        load_home_dotenv: false,
        ..Default::default()
    })
    .expect("config should load");

    // These should come from HardcodedDefaults since empty_path has no config files
    assert_eq!(config.get_string("log_level"), Some("info".to_string()));
    assert_eq!(config.get_string("log_format"), Some("auto".to_string()));
}

#[test]
fn test_hardcoded_defaults_are_lowest_priority() {
    let (_dir, path) = setup_config_dir();

    let config = Config::new(ConfigOptions {
        config_paths: vec![path],
        ..Default::default()
    })
    .expect("config should load");

    // defaults.yaml overrides hardcoded log_level
    assert_eq!(config.get_string("log_level"), Some("debug".to_string()));
}

// ============================================================================
// Layer 6: defaults.yaml tests
// ============================================================================

#[test]
fn test_defaults_yaml_loaded() {
    let (_dir, path) = setup_config_dir();

    let config = Config::new(ConfigOptions {
        config_paths: vec![path],
        ..Default::default()
    })
    .expect("config should load");

    // Values from defaults.yaml
    assert_eq!(
        config.get_string("database.host"),
        Some("dev-host".to_string()) // Overridden by settings.development.yaml
    );
    assert_eq!(config.get_int("database.port"), Some(5432)); // Only in defaults.yaml
}

// ============================================================================
// Layer 5: settings.yaml tests
// ============================================================================

#[test]
fn test_settings_yaml_overrides_defaults() {
    let (_dir, path) = setup_config_dir();

    let config = Config::new(ConfigOptions {
        config_paths: vec![path],
        ..Default::default()
    })
    .expect("config should load");

    // settings.yaml overrides defaults.yaml
    assert_eq!(
        config.get_string("database.username"),
        Some("settings-user".to_string())
    );

    // app_name only in settings.yaml
    assert_eq!(config.get_string("app_name"), Some("test_app".to_string()));
}

// ============================================================================
// Layer 4: settings.{env}.yaml tests
// ============================================================================

#[test]
fn test_env_specific_settings_development() {
    let (_dir, path) = setup_config_dir();
    let _env = EnvGuard::new(&[("APP_ENV", "development")]);

    let config = Config::new(ConfigOptions {
        config_paths: vec![path],
        app_env: Some("development".to_string()),
        ..Default::default()
    })
    .expect("config should load");

    // settings.development.yaml overrides settings.yaml
    assert_eq!(
        config.get_string("database.host"),
        Some("dev-host".to_string())
    );
    assert_eq!(config.get_bool("debug"), Some(true));
    assert_eq!(config.get_bool("cache.enabled"), Some(true));
    assert_eq!(config.get_int("cache.ttl"), Some(60));
}

#[test]
fn test_env_specific_settings_production() {
    let (_dir, path) = setup_config_dir();

    let config = Config::new(ConfigOptions {
        config_paths: vec![path],
        app_env: Some("production".to_string()),
        ..Default::default()
    })
    .expect("config should load");

    // settings.production.yaml overrides settings.yaml
    assert_eq!(
        config.get_string("database.host"),
        Some("prod-host".to_string())
    );
    assert_eq!(config.get_bool("debug"), Some(false));
    assert_eq!(config.get_int("cache.ttl"), Some(3600));
}

// ============================================================================
// Layer 3: .env file tests
// ============================================================================

#[test]
fn test_dotenv_file_loaded() {
    let (_dir, path) = setup_config_dir();

    // Create .env file in temp directory
    let dotenv_path = path.join(".env");
    fs::write(&dotenv_path, "DOTENV_TEST_VAR=from_dotenv\n").expect("failed to write .env");

    // Change to temp directory so dotenvy finds it
    let original_dir = std::env::current_dir().expect("failed to get cwd");
    std::env::set_current_dir(&path).expect("failed to change dir");

    let config = Config::new(ConfigOptions {
        env_prefix: "DOTENV_TEST".to_string(),
        config_paths: vec![path.clone()],
        load_dotenv: true,
        ..Default::default()
    })
    .expect("config should load");

    // Restore original directory
    std::env::set_current_dir(original_dir).expect("failed to restore dir");

    // .env should have been loaded into environment
    assert_eq!(config.get_string("var"), Some("from_dotenv".to_string()));

    // Cleanup
    std::env::remove_var("DOTENV_TEST_VAR");
}

// ============================================================================
// Layer 2: Environment variables tests
// ============================================================================

#[test]
fn test_env_overrides_file() {
    let (_dir, path) = setup_config_dir();
    let _env = EnvGuard::new(&[("PARITY_DATABASE__HOST", "env-host")]);

    let config = Config::new(ConfigOptions {
        env_prefix: "PARITY".to_string(),
        config_paths: vec![path],
        ..Default::default()
    })
    .expect("config should load");

    // ENV var overrides settings.development.yaml
    assert_eq!(
        config.get_string("database.host"),
        Some("env-host".to_string())
    );
}

#[test]
fn test_env_var_flat_key() {
    let _env = EnvGuard::new(&[("FLAT_LOG_LEVEL", "warn")]);

    let config = Config::new(ConfigOptions {
        env_prefix: "FLAT".to_string(),
        ..Default::default()
    })
    .expect("config should load");

    // Flat env var (no nesting)
    assert_eq!(config.get_string("log_level"), Some("warn".to_string()));
}

#[test]
fn test_env_var_nested_key() {
    let _env = EnvGuard::new(&[("NESTED_DATABASE__HOST", "nested-host")]);

    let config = Config::new(ConfigOptions {
        env_prefix: "NESTED".to_string(),
        ..Default::default()
    })
    .expect("config should load");

    // Nested env var using double underscore
    assert_eq!(
        config.get_string("database.host"),
        Some("nested-host".to_string())
    );
}

#[test]
fn test_env_var_deeply_nested() {
    // Figment's split("__") replaces __ with . for nested keys
    // With prefix "DEEP2_", env var "DEEP2_A__B" becomes "a.b"
    let _env = EnvGuard::new(&[("DEEP2_CACHE__REDIS__ENABLED", "true")]);

    let config = Config::new(ConfigOptions {
        env_prefix: "DEEP2".to_string(),
        ..Default::default()
    })
    .expect("config should load");

    // Three levels: cache.redis.enabled
    assert_eq!(config.get_bool("cache.redis.enabled"), Some(true));
}

// ============================================================================
// Layer 1: CLI args tests
// ============================================================================

#[test]
fn test_cli_args_highest_priority() {
    let (_dir, path) = setup_config_dir();
    let _env = EnvGuard::new(&[("CLI_DATABASE__HOST", "env-host")]);

    // Create config with file and env
    let config = Config::new(ConfigOptions {
        env_prefix: "CLI".to_string(),
        config_paths: vec![path],
        ..Default::default()
    })
    .expect("config should load");

    // Simulate CLI args by merging
    #[derive(serde::Serialize)]
    struct CliArgs {
        database: Database,
    }
    #[derive(serde::Serialize)]
    struct Database {
        host: String,
    }

    let cli = CliArgs {
        database: Database {
            host: "cli-host".to_string(),
        },
    };

    let config = config.merge_cli(cli);

    // CLI should override everything
    assert_eq!(
        config.get_string("database.host"),
        Some("cli-host".to_string())
    );
}

// ============================================================================
// Full cascade priority tests
// ============================================================================

#[test]
fn test_full_cascade_priority() {
    let (_dir, path) = setup_config_dir();
    let _env = EnvGuard::new(&[("CASCADE_DATABASE__HOST", "env-host")]);

    // Start with all layers
    let config = Config::new(ConfigOptions {
        env_prefix: "CASCADE".to_string(),
        config_paths: vec![path],
        app_env: Some("development".to_string()),
        ..Default::default()
    })
    .expect("config should load");

    // ENV overrides file configs
    assert_eq!(
        config.get_string("database.host"),
        Some("env-host".to_string())
    );

    // settings.development.yaml values not overridden by ENV
    assert_eq!(config.get_bool("debug"), Some(true));

    // settings.yaml values
    assert_eq!(config.get_string("app_name"), Some("test_app".to_string()));

    // defaults.yaml values not overridden
    assert_eq!(config.get_int("database.port"), Some(5432));

    // hardcoded defaults
    assert_eq!(config.get_string("log_format"), Some("auto".to_string()));
}

// ============================================================================
// Type coercion tests (parity with Python/Go)
// ============================================================================

#[test]
fn test_get_bool_from_string() {
    let _env = EnvGuard::new(&[("BOOL_ENABLED", "true")]);

    let config = Config::new(ConfigOptions {
        env_prefix: "BOOL".to_string(),
        ..Default::default()
    })
    .expect("config should load");

    assert_eq!(config.get_bool("enabled"), Some(true));
}

#[test]
fn test_get_int_from_string() {
    let _env = EnvGuard::new(&[("INT_PORT", "8080")]);

    let config = Config::new(ConfigOptions {
        env_prefix: "INT".to_string(),
        ..Default::default()
    })
    .expect("config should load");

    assert_eq!(config.get_int("port"), Some(8080));
}

#[test]
fn test_duration_parsing() {
    let (_dir, path) = setup_config_dir();

    let config = Config::new(ConfigOptions {
        config_paths: vec![path],
        ..Default::default()
    })
    .expect("config should load");

    // defaults.yaml has timeout: 30s
    let duration = config.get_duration("database.timeout");
    assert_eq!(duration, Some(std::time::Duration::from_secs(30)));
}

// ============================================================================
// Missing/non-existent key tests
// ============================================================================

#[test]
fn test_missing_key_returns_none() {
    let config = Config::new(ConfigOptions::default()).expect("config should load");

    assert_eq!(config.get_string("nonexistent.key"), None);
    assert_eq!(config.get_int("nonexistent.key"), None);
    assert_eq!(config.get_bool("nonexistent.key"), None);
}

#[test]
fn test_contains_key() {
    let config = Config::new(ConfigOptions::default()).expect("config should load");

    // Hardcoded defaults exist
    assert!(config.contains("log_level"));
    assert!(config.contains("log_format"));

    // Non-existent key
    assert!(!config.contains("nonexistent.key"));
}

// ============================================================================
// Unmarshal tests
// ============================================================================

#[test]
fn test_unmarshal_struct() {
    let (_dir, path) = setup_config_dir();

    let config = Config::new(ConfigOptions {
        config_paths: vec![path],
        ..Default::default()
    })
    .expect("config should load");

    #[derive(Debug, serde::Deserialize, PartialEq)]
    struct Database {
        host: String,
        port: i64,
        username: String,
    }

    let db: Database = config.unmarshal_key("database").expect("should unmarshal");
    assert_eq!(db.host, "dev-host"); // From settings.development.yaml
    assert_eq!(db.port, 5432); // From defaults.yaml
    assert_eq!(db.username, "settings-user"); // From settings.yaml
}
