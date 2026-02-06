// Project:   hs-rustlib
// File:      src/config/mod.rs
// Purpose:   7-layer configuration cascade
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Configuration management with 8-layer cascade.
//!
//! Provides a hierarchical configuration system matching hs-lib (Python)
//! and hs-golib (Go). Configuration is loaded from multiple sources with
//! clear priority ordering.
//!
//! ## Cascade Priority (highest to lowest)
//!
//! 1. CLI arguments (via clap integration)
//! 2. Environment variables (with configurable prefix)
//! 3. `.env` file (loaded via dotenvy)
//! 4. PostgreSQL (optional, via `config-postgres` feature)
//! 5. `settings.{env}.yaml` (environment-specific)
//! 6. `settings.yaml` (base settings)
//! 7. `defaults.yaml`
//! 8. Hard-coded defaults
//!
//! ## How .env Files Work in the Cascade
//!
//! The `.env` file is loaded early in the cascade using `dotenvy::dotenv()`.
//! This populates the process environment, so `.env` values become available
//! via `std::env::var()`. The cascade then reads environment variables at
//! layer 2, which includes both real environment variables AND `.env` values.
//!
//! **Important**: Real environment variables take precedence over `.env` values
//! because `dotenvy` does NOT overwrite existing environment variables.
//!
//! ```text
//! Priority (highest wins):
//! ┌─────────────────────────────────────────────────────────────┐
//! │ 1. CLI arguments (merged via merge_cli())                   │
//! ├─────────────────────────────────────────────────────────────┤
//! │ 2. Environment variables (PREFIX_KEY)                       │
//! │    ↑ Includes .env values (loaded into env by dotenvy)      │
//! │    ↑ Real env vars win over .env (dotenvy doesn't overwrite)│
//! ├─────────────────────────────────────────────────────────────┤
//! │ 3. PostgreSQL config (if config-postgres feature enabled)   │
//! ├─────────────────────────────────────────────────────────────┤
//! │ 4. settings.{env}.yaml (e.g., settings.production.yaml)     │
//! ├─────────────────────────────────────────────────────────────┤
//! │ 5. settings.yaml                                            │
//! ├─────────────────────────────────────────────────────────────┤
//! │ 6. defaults.yaml                                            │
//! ├─────────────────────────────────────────────────────────────┤
//! │ 7. Hard-coded defaults (lowest priority)                    │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Environment Variable Naming
//!
//! Use the `env_compat` module for standardized environment variable names
//! with legacy alias support and deprecation warnings.
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

pub mod env_compat;

#[cfg(feature = "config-postgres")]
pub mod postgres;

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use figment::providers::{Env, Format, Serialized, Yaml};
use figment::Figment;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::env::get_app_env;

#[cfg(feature = "config-postgres")]
use self::postgres::{PostgresConfig, PostgresConfigError, PostgresConfigSource};

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

    /// PostgreSQL config error.
    #[cfg(feature = "config-postgres")]
    #[error("PostgreSQL config error: {0}")]
    Postgres(#[from] PostgresConfigError),
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

    /// Whether to load `.env` files.
    ///
    /// When enabled, loads `.env` files in this order (lowest to highest priority):
    /// 1. `~/.env` (home directory - global defaults)
    /// 2. Project `.env` (current directory - project overrides)
    ///
    /// Later files override earlier ones. Real environment variables always
    /// take precedence over `.env` values.
    pub load_dotenv: bool,

    /// Whether to load home directory `.env` file (`~/.env`).
    ///
    /// Only applies when `load_dotenv` is true.
    /// Default: true
    pub load_home_dotenv: bool,

    /// PostgreSQL config source (optional, requires `config-postgres` feature).
    #[cfg(feature = "config-postgres")]
    pub postgres: Option<PostgresConfigSource>,
}

impl Default for ConfigOptions {
    fn default() -> Self {
        Self {
            env_prefix: String::new(),
            app_env: None,
            config_paths: Vec::new(),
            load_dotenv: true,
            load_home_dotenv: true,
            #[cfg(feature = "config-postgres")]
            postgres: None,
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

        // Load .env files in cascade order (lowest to highest priority)
        // Home directory .env provides global defaults
        // Project .env provides project-specific overrides
        // Real environment variables always win (dotenvy doesn't overwrite)
        if opts.load_dotenv {
            Self::load_dotenv_cascade(opts.load_home_dotenv);
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

    /// Create a new configuration with async loading (for PostgreSQL support).
    ///
    /// This method loads configuration asynchronously, allowing PostgreSQL to be
    /// used as a config source. PostgreSQL sits above file-based config in the
    /// cascade, so database values override file values.
    ///
    /// # Errors
    ///
    /// Returns an error if configuration loading fails.
    #[cfg(feature = "config-postgres")]
    pub async fn new_async(opts: ConfigOptions) -> Result<Self, ConfigError> {
        let app_env = opts.app_env.clone().unwrap_or_else(get_app_env);

        // Load .env files in cascade order (lowest to highest priority)
        if opts.load_dotenv {
            Self::load_dotenv_cascade(opts.load_home_dotenv);
        }

        // Determine PostgreSQL config source
        let pg_source = opts
            .postgres
            .clone()
            .unwrap_or_else(|| PostgresConfigSource::from_env(&opts.env_prefix));

        // Load PostgreSQL config (async)
        let pg_config = PostgresConfig::load(&pg_source).await?;

        // Build the cascade (lowest to highest priority)
        let mut figment = Figment::new();

        // 8. Hard-coded defaults (lowest priority)
        figment = figment.merge(Serialized::defaults(HardcodedDefaults::default()));

        // 7. defaults.yaml
        for path in Self::find_config_files("defaults", &opts.config_paths) {
            figment = figment.merge(Yaml::file(&path));
        }

        // 6. settings.yaml
        for path in Self::find_config_files("settings", &opts.config_paths) {
            figment = figment.merge(Yaml::file(&path));
        }

        // 5. settings.{env}.yaml
        let env_settings = format!("settings.{app_env}");
        for path in Self::find_config_files(&env_settings, &opts.config_paths) {
            figment = figment.merge(Yaml::file(&path));
        }

        // 4. PostgreSQL config (above files, below .env)
        if let Some(ref pg) = pg_config {
            let nested = pg.to_nested();
            // For merge mode, we merge into existing config
            // For replace mode, PostgreSQL config replaces file-based config
            // Since figment merges are additive with later values winning,
            // we just merge here - the position in the cascade determines priority
            figment = figment.merge(Serialized::defaults(nested));
        }

        // 3. .env file values are already loaded into env vars

        // 2. Environment variables (with prefix)
        if !opts.env_prefix.is_empty() {
            figment = figment.merge(Env::prefixed(&format!("{}_", opts.env_prefix)).split("__"));
        }

        // 1. CLI args would be merged by the application via merge_cli()

        Ok(Self {
            figment,
            env_prefix: opts.env_prefix,
        })
    }

    /// Load `.env` files in cascade order.
    ///
    /// Order (lowest to highest priority):
    /// 1. `~/.env` (home directory - global defaults)
    /// 2. Project `.env` (current directory - project overrides)
    ///
    /// Note: `dotenvy` does NOT overwrite existing environment variables,
    /// so later files in the cascade take precedence. We load in reverse
    /// order (project first, then home) so that project values are set first
    /// and home values only fill in missing variables.
    ///
    /// Real environment variables always take precedence over all `.env` values.
    fn load_dotenv_cascade(load_home: bool) {
        use tracing::debug;

        // Load project .env first (these values take precedence)
        // dotenvy::dotenv() looks for .env in current directory
        match dotenvy::dotenv() {
            Ok(path) => {
                debug!(path = %path.display(), "Loaded project .env file");
            }
            Err(dotenvy::Error::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound => {
                // No project .env, that's fine
            }
            Err(e) => {
                debug!(error = %e, "Failed to load project .env file");
            }
        }

        // Load home directory .env (only fills in missing values)
        if load_home {
            if let Some(home) = dirs::home_dir() {
                let home_env = home.join(".env");
                if home_env.exists() {
                    match dotenvy::from_path(&home_env) {
                        Ok(()) => {
                            debug!(path = %home_env.display(), "Loaded home .env file");
                        }
                        Err(e) => {
                            debug!(path = %home_env.display(), error = %e, "Failed to load home .env file");
                        }
                    }
                }
            }
        }
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

/// Initialise the global configuration with async loading (for PostgreSQL support).
///
/// This function loads configuration asynchronously, allowing PostgreSQL to be
/// used as a config source.
///
/// # Errors
///
/// Returns an error if configuration loading fails or if already initialised.
#[cfg(feature = "config-postgres")]
pub async fn setup_async(opts: ConfigOptions) -> Result<(), ConfigError> {
    let config = Config::new_async(opts).await?;
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
        assert!(opts.load_home_dotenv);
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
