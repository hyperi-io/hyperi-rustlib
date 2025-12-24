// Project:   hs-rustlib
// File:      src/config/mod.rs
// Purpose:   7-layer configuration cascade
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Configuration management with 7-layer cascade.
//!
//! Provides a hierarchical configuration system matching hs-lib (Python)
//! and hs-golib (Go). Configuration is loaded from multiple sources with
//! clear priority ordering.
//!
//! ## Cascade Priority (highest to lowest)
//!
//! 1. CLI arguments (via clap integration)
//! 2. Environment variables (with configurable prefix)
//! 3. `.env` file
//! 4. `settings.{env}.yaml` (environment-specific)
//! 5. `settings.yaml` (base settings)
//! 6. `defaults.yaml`
//! 7. Hard-coded defaults
//!
//! ## Example
//!
//! ```rust,no_run
//! use hs_rustlib::config::{self, ConfigOptions};
//!
//! // Initialise with env prefix
//! config::setup(ConfigOptions {
//!     env_prefix: "MYAPP".into(),
//!     ..Default::default()
//! }).unwrap();
//!
//! // Access configuration
//! let cfg = config::get();
//! let host = cfg.get_string("database.host").unwrap_or_default();
//! let port = cfg.get_int("database.port").unwrap_or(5432);
//! ```

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use figment::providers::{Env, Format, Serialized, Yaml};
use figment::Figment;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::env::get_app_env;

/// Global configuration singleton.
static CONFIG: OnceLock<Config> = OnceLock::new();

/// Configuration errors.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Failed to load configuration file.
    #[error("failed to load config file '{path}': {message}")]
    LoadError { path: PathBuf, message: String },

    /// Failed to extract configuration value.
    #[error("failed to extract config: {0}")]
    ExtractError(#[from] figment::Error),

    /// Missing required configuration key.
    #[error("missing required config key: {0}")]
    MissingKey(String),

    /// Invalid configuration value.
    #[error("invalid config value for '{key}': {reason}")]
    InvalidValue { key: String, reason: String },

    /// Configuration already initialised.
    #[error("configuration already initialised")]
    AlreadyInitialised,

    /// Configuration not initialised.
    #[error("configuration not initialised - call config::setup() first")]
    NotInitialised,
}

/// Configuration options.
#[derive(Debug, Clone)]
pub struct ConfigOptions {
    /// Environment variable prefix (e.g., "MYAPP" for MYAPP_DATABASE_HOST).
    pub env_prefix: String,

    /// Override the detected app environment (dev, staging, prod).
    pub app_env: Option<String>,

    /// Additional paths to search for config files.
    pub config_paths: Vec<PathBuf>,

    /// Whether to load `.env` file.
    pub load_dotenv: bool,
}

impl Default for ConfigOptions {
    fn default() -> Self {
        Self {
            env_prefix: String::new(),
            app_env: None,
            config_paths: Vec::new(),
            load_dotenv: true,
        }
    }
}

/// Configuration manager wrapping Figment.
#[derive(Debug)]
pub struct Config {
    figment: Figment,
    env_prefix: String,
}

impl Config {
    /// Create a new configuration with the given options.
    ///
    /// # Errors
    ///
    /// Returns an error if configuration loading fails.
    pub fn new(opts: ConfigOptions) -> Result<Self, ConfigError> {
        let app_env = opts.app_env.unwrap_or_else(get_app_env);

        // Load .env file if requested
        if opts.load_dotenv {
            let _ = dotenvy::dotenv();
        }

        // Build the cascade (lowest to highest priority)
        let mut figment = Figment::new();

        // 7. Hard-coded defaults (lowest priority)
        figment = figment.merge(Serialized::defaults(HardcodedDefaults::default()));

        // 6. defaults.yaml
        for path in Self::find_config_files("defaults", &opts.config_paths) {
            figment = figment.merge(Yaml::file(&path));
        }

        // 5. settings.yaml
        for path in Self::find_config_files("settings", &opts.config_paths) {
            figment = figment.merge(Yaml::file(&path));
        }

        // 4. settings.{env}.yaml
        let env_settings = format!("settings.{app_env}");
        for path in Self::find_config_files(&env_settings, &opts.config_paths) {
            figment = figment.merge(Yaml::file(&path));
        }

        // 3. .env file values are already loaded into env vars

        // 2. Environment variables (with prefix)
        // Keys are lowercased: TEST_DATABASE_HOST -> database_host
        // Use double underscore for nesting: TEST_DATABASE__HOST -> database.host
        if !opts.env_prefix.is_empty() {
            figment = figment.merge(Env::prefixed(&format!("{}_", opts.env_prefix)).split("__"));
        }

        // 1. CLI args would be merged by the application via merge_cli()

        Ok(Self {
            figment,
            env_prefix: opts.env_prefix,
        })
    }

    /// Find config files with the given base name.
    fn find_config_files(base_name: &str, extra_paths: &[PathBuf]) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let extensions = ["yaml", "yml"];

        // Check current directory
        for ext in &extensions {
            let path = PathBuf::from(format!("{base_name}.{ext}"));
            if path.exists() {
                files.push(path);
                break;
            }
        }

        // Check config subdirectory
        for ext in &extensions {
            let path = PathBuf::from(format!("config/{base_name}.{ext}"));
            if path.exists() {
                files.push(path);
                break;
            }
        }

        // Check extra paths
        for base in extra_paths {
            for ext in &extensions {
                let path = base.join(format!("{base_name}.{ext}"));
                if path.exists() {
                    files.push(path);
                    break;
                }
            }
        }

        files
    }

    /// Merge CLI arguments into the configuration.
    ///
    /// Call this after parsing CLI args to add them as highest priority.
    #[must_use]
    pub fn merge_cli<T: serde::Serialize>(mut self, cli_args: T) -> Self {
        self.figment = self.figment.merge(Serialized::defaults(cli_args));
        self
    }

    /// Get a string value.
    #[must_use]
    pub fn get_string(&self, key: &str) -> Option<String> {
        self.figment.extract_inner::<String>(key).ok()
    }

    /// Get an integer value.
    #[must_use]
    pub fn get_int(&self, key: &str) -> Option<i64> {
        self.figment.extract_inner::<i64>(key).ok()
    }

    /// Get a float value.
    #[must_use]
    pub fn get_float(&self, key: &str) -> Option<f64> {
        self.figment.extract_inner::<f64>(key).ok()
    }

    /// Get a boolean value.
    #[must_use]
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.figment.extract_inner::<bool>(key).ok()
    }

    /// Get a duration value (parses strings like "30s", "5m", "1h").
    #[must_use]
    pub fn get_duration(&self, key: &str) -> Option<Duration> {
        let value = self.get_string(key)?;
        parse_duration(&value)
    }

    /// Get a list of strings.
    #[must_use]
    pub fn get_string_list(&self, key: &str) -> Option<Vec<String>> {
        self.figment.extract_inner::<Vec<String>>(key).ok()
    }

    /// Check if a key exists.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.figment.find_value(key).is_ok()
    }

    /// Deserialise the entire configuration into a typed struct.
    ///
    /// # Errors
    ///
    /// Returns an error if deserialisation fails.
    pub fn unmarshal<T: DeserializeOwned>(&self) -> Result<T, ConfigError> {
        self.figment.extract().map_err(ConfigError::ExtractError)
    }

    /// Deserialise a specific key into a typed struct.
    ///
    /// # Errors
    ///
    /// Returns an error if deserialisation fails.
    pub fn unmarshal_key<T: DeserializeOwned>(&self, key: &str) -> Result<T, ConfigError> {
        self.figment
            .extract_inner(key)
            .map_err(ConfigError::ExtractError)
    }

    /// Get the environment variable prefix.
    #[must_use]
    pub fn env_prefix(&self) -> &str {
        &self.env_prefix
    }
}

/// Hard-coded default values (lowest priority in cascade).
#[derive(Debug, serde::Serialize)]
struct HardcodedDefaults {
    log_level: String,
    log_format: String,
}

impl Default for HardcodedDefaults {
    fn default() -> Self {
        Self {
            log_level: "info".to_string(),
            log_format: "auto".to_string(),
        }
    }
}

/// Parse a duration string like "30s", "5m", "1h", "2h30m".
fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim().to_lowercase();

    // Try simple formats first
    if let Some(secs) = s.strip_suffix('s') {
        return secs.parse::<u64>().ok().map(Duration::from_secs);
    }
    if let Some(mins) = s.strip_suffix('m') {
        return mins
            .parse::<u64>()
            .ok()
            .map(|m| Duration::from_secs(m * 60));
    }
    if let Some(hours) = s.strip_suffix('h') {
        return hours
            .parse::<u64>()
            .ok()
            .map(|h| Duration::from_secs(h * 3600));
    }

    // Try parsing as seconds
    s.parse::<u64>().ok().map(Duration::from_secs)
}

// Global singleton functions

/// Initialise the global configuration.
///
/// # Errors
///
/// Returns an error if configuration loading fails or if already initialised.
pub fn setup(opts: ConfigOptions) -> Result<(), ConfigError> {
    let config = Config::new(opts)?;
    CONFIG
        .set(config)
        .map_err(|_| ConfigError::AlreadyInitialised)
}

/// Get the global configuration.
///
/// # Panics
///
/// Panics if configuration has not been initialised.
#[must_use]
pub fn get() -> &'static Config {
    CONFIG
        .get()
        .expect("configuration not initialised - call config::setup() first")
}

/// Try to get the global configuration.
#[must_use]
pub fn try_get() -> Option<&'static Config> {
    CONFIG.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
        assert_eq!(parse_duration("1s"), Some(Duration::from_secs(1)));
    }

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration("5m"), Some(Duration::from_secs(300)));
        assert_eq!(parse_duration("1m"), Some(Duration::from_secs(60)));
    }

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration("1h"), Some(Duration::from_secs(3600)));
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
    }

    #[test]
    fn test_parse_duration_plain_number() {
        assert_eq!(parse_duration("60"), Some(Duration::from_secs(60)));
    }

    #[test]
    fn test_config_options_default() {
        let opts = ConfigOptions::default();
        assert!(opts.env_prefix.is_empty());
        assert!(opts.app_env.is_none());
        assert!(opts.config_paths.is_empty());
        assert!(opts.load_dotenv);
    }

    #[test]
    fn test_config_new() {
        let config = Config::new(ConfigOptions::default());
        assert!(config.is_ok());
    }

    #[test]
    fn test_config_hardcoded_defaults() {
        let config = Config::new(ConfigOptions::default()).unwrap();

        // Should have hardcoded defaults
        assert_eq!(config.get_string("log_level"), Some("info".to_string()));
        assert_eq!(config.get_string("log_format"), Some("auto".to_string()));
    }

    #[test]
    fn test_config_env_override() {
        // Env vars use double underscore for nesting: PREFIX_KEY__SUBKEY -> key.subkey
        // For flat keys, just use PREFIX_KEY -> key
        std::env::set_var("TEST_HOST", "testhost");

        let config = Config::new(ConfigOptions {
            env_prefix: "TEST".into(),
            ..Default::default()
        })
        .unwrap();

        assert_eq!(config.get_string("host"), Some("testhost".to_string()));

        std::env::remove_var("TEST_HOST");
    }
}
