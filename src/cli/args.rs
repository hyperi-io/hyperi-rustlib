// Project:   hyperi-rustlib
// File:      src/cli/args.rs
// Purpose:   Standard CLI arguments for DFE services
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Common CLI arguments shared across all DFE services.
//!
//! Use `#[command(flatten)]` to embed these in your application's Clap parser:
//!
//! ```rust,ignore
//! use clap::Parser;
//! use hyperi_rustlib::cli::CommonArgs;
//!
//! #[derive(Parser)]
//! struct App {
//!     #[command(flatten)]
//!     common: CommonArgs,
//! }
//! ```

/// Standard CLI arguments for DFE services.
///
/// Provides the 80% of flags that every service needs:
/// config path, log level/format, metrics address, verbose/quiet modes.
///
/// Embed in your Clap parser with `#[command(flatten)]`.
#[derive(Debug, Clone, clap::Args)]
pub struct CommonArgs {
    /// Path to configuration file.
    #[arg(short = 'c', long = "config")]
    pub config: Option<String>,

    /// Log level (trace, debug, info, warn, error).
    #[arg(
        short = 'l',
        long = "log-level",
        env = "LOG_LEVEL",
        default_value = "info"
    )]
    pub log_level: String,

    /// Log output format (json, text, auto).
    #[arg(long = "log-format", env = "LOG_FORMAT", default_value = "auto")]
    pub log_format: String,

    /// Metrics server bind address.
    #[arg(
        long = "metrics-addr",
        env = "METRICS_ADDR",
        default_value = "0.0.0.0:9090"
    )]
    pub metrics_addr: String,

    /// Enable verbose output (sets log level to debug).
    #[arg(short = 'v', long, conflicts_with = "quiet")]
    pub verbose: bool,

    /// Suppress all output except errors.
    #[arg(short = 'q', long, conflicts_with = "verbose")]
    pub quiet: bool,
}

impl CommonArgs {
    /// Resolve the effective log level, accounting for --verbose and --quiet flags.
    #[must_use]
    pub fn effective_log_level(&self) -> &str {
        if self.verbose {
            "debug"
        } else if self.quiet {
            "error"
        } else {
            &self.log_level
        }
    }

    /// Convert to `LoggerOptions` for use with `logger::setup()`.
    ///
    /// Parses the log level and format strings into their typed equivalents.
    ///
    /// # Errors
    ///
    /// Returns `CliError::InvalidArgument` if the log level or format is invalid.
    #[cfg(feature = "logger")]
    pub fn to_logger_options(&self) -> Result<crate::logger::LoggerOptions, super::CliError> {
        use std::str::FromStr;

        let level: tracing::Level =
            self.effective_log_level()
                .to_uppercase()
                .parse()
                .map_err(|_| {
                    super::CliError::InvalidArgument(format!(
                        "invalid log level: {}",
                        self.effective_log_level()
                    ))
                })?;

        let format = crate::logger::LogFormat::from_str(&self.log_format)
            .map_err(|e| super::CliError::InvalidArgument(format!("invalid log format: {e}")))?;

        Ok(crate::logger::LoggerOptions {
            level,
            format,
            ..Default::default()
        })
    }

    /// Convert to `ConfigOptions` for use with `config::setup()`.
    ///
    /// The `--config <path>` flag names a FILE, so it populates
    /// [`ConfigOptions::config_file`](crate::config::ConfigOptions::config_file)
    /// -- NOT `config_paths`, which is a list of DIRECTORIES to search for the
    /// standard base names. (Before 2.8.11 this wrongly pushed the file path
    /// into `config_paths`, where directory discovery never found it.)
    #[cfg(feature = "config")]
    #[must_use]
    pub fn to_config_options(&self, env_prefix: &str) -> crate::config::ConfigOptions {
        crate::config::ConfigOptions {
            env_prefix: env_prefix.to_string(),
            config_file: self.config.as_deref().map(std::path::PathBuf::from),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_effective_log_level_default() {
        let args = CommonArgs {
            config: None,
            log_level: "info".to_string(),
            log_format: "auto".to_string(),
            metrics_addr: "0.0.0.0:9090".to_string(),
            verbose: false,
            quiet: false,
        };
        assert_eq!(args.effective_log_level(), "info");
    }

    #[test]
    fn test_effective_log_level_verbose() {
        let args = CommonArgs {
            config: None,
            log_level: "info".to_string(),
            log_format: "auto".to_string(),
            metrics_addr: "0.0.0.0:9090".to_string(),
            verbose: true,
            quiet: false,
        };
        assert_eq!(args.effective_log_level(), "debug");
    }

    #[test]
    fn test_effective_log_level_quiet() {
        let args = CommonArgs {
            config: None,
            log_level: "info".to_string(),
            log_format: "auto".to_string(),
            metrics_addr: "0.0.0.0:9090".to_string(),
            verbose: false,
            quiet: true,
        };
        assert_eq!(args.effective_log_level(), "error");
    }

    #[cfg(feature = "config")]
    #[test]
    fn test_to_config_options_sets_config_file_not_paths() {
        let args = CommonArgs {
            config: Some("/etc/svc/config.yaml".to_string()),
            log_level: "info".to_string(),
            log_format: "auto".to_string(),
            metrics_addr: "0.0.0.0:9090".to_string(),
            verbose: false,
            quiet: false,
        };
        let opts = args.to_config_options("MY_SVC");
        assert_eq!(opts.env_prefix, "MY_SVC");
        // The file path lands in config_file, NOT config_paths (the 2.8.11 fix).
        assert_eq!(
            opts.config_file,
            Some(std::path::PathBuf::from("/etc/svc/config.yaml"))
        );
        assert!(opts.config_paths.is_empty());
    }

    #[cfg(feature = "config")]
    #[test]
    fn test_to_config_options_no_config_file_when_absent() {
        let args = CommonArgs {
            config: None,
            log_level: "info".to_string(),
            log_format: "auto".to_string(),
            metrics_addr: "0.0.0.0:9090".to_string(),
            verbose: false,
            quiet: false,
        };
        let opts = args.to_config_options("MY_SVC");
        assert!(opts.config_file.is_none());
        assert!(opts.config_paths.is_empty());
    }

    #[test]
    fn test_effective_log_level_custom() {
        let args = CommonArgs {
            config: None,
            log_level: "warn".to_string(),
            log_format: "auto".to_string(),
            metrics_addr: "0.0.0.0:9090".to_string(),
            verbose: false,
            quiet: false,
        };
        assert_eq!(args.effective_log_level(), "warn");
    }
}
