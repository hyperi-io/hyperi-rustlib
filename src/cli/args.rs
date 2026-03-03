// Project:   hyperi-rustlib
// File:      src/cli/args.rs
// Purpose:   Standard CLI arguments for DFE services
// Language:  Rust
//
// License:   FSL-1.1-ALv2
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
    #[cfg(feature = "config")]
    #[must_use]
    pub fn to_config_options(&self, env_prefix: &str) -> crate::config::ConfigOptions {
        let mut opts = crate::config::ConfigOptions {
            env_prefix: env_prefix.to_string(),
            ..Default::default()
        };
        if let Some(ref path) = self.config {
            opts.config_paths.push(path.into());
        }
        opts
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
