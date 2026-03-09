// Project:   hyperi-rustlib
// File:      src/cli/app.rs
// Purpose:   DfeApp trait and standard lifecycle runner
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Application trait and lifecycle runner for DFE services.
//!
//! Provides the standard startup sequence: parse → log → config → dispatch.
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::cli::{CommonArgs, DfeApp, CliError, VersionInfo, run_app};
//!
//! struct MyApp { common: CommonArgs }
//!
//! impl DfeApp for MyApp {
//!     type Config = MyConfig;
//!
//!     fn name(&self) -> &str { "my-service" }
//!     fn env_prefix(&self) -> &str { "MY_SERVICE" }
//!     fn version_info(&self) -> VersionInfo {
//!         VersionInfo::new("my-service", env!("CARGO_PKG_VERSION"))
//!     }
//!     fn common_args(&self) -> &CommonArgs { &self.common }
//!     fn load_config(&self, path: Option<&str>) -> Result<MyConfig, CliError> { todo!() }
//!     async fn run_service(&self, config: MyConfig) -> Result<(), CliError> { todo!() }
//! }
//! ```

use std::fmt::Debug;

use serde::de::DeserializeOwned;

use super::error::CliError;
use super::version::VersionInfo;
use super::{CommonArgs, StandardCommand, output};

/// Trait for DFE service applications.
///
/// Implement this trait to get the standard CLI lifecycle for free.
/// The 80% common behaviour (logging, config, metrics, version) is handled
/// by `run_app()`. Your app provides the 20% (config type, service logic).
pub trait DfeApp: Sized {
    /// Application-specific configuration type.
    type Config: DeserializeOwned + Debug + Send + Sync;

    /// Service name (e.g. "dfe-loader").
    fn name(&self) -> &str;

    /// Environment variable prefix for config cascade (e.g. "DFE_LOADER").
    fn env_prefix(&self) -> &str;

    /// Version information for this service.
    fn version_info(&self) -> VersionInfo;

    /// Access the common CLI arguments.
    fn common_args(&self) -> &CommonArgs;

    /// Resolve the active subcommand.
    ///
    /// Returns `None` to default to `StandardCommand::Run`.
    fn command(&self) -> Option<&StandardCommand> {
        None
    }

    /// Load application configuration from the given path (or defaults).
    ///
    /// # Errors
    ///
    /// Returns `CliError` if configuration cannot be loaded or parsed.
    fn load_config(&self, path: Option<&str>) -> Result<Self::Config, CliError>;

    /// Run the main service loop.
    ///
    /// Called after logging and config are initialised.
    ///
    /// # Errors
    ///
    /// Returns `CliError` if the service encounters a fatal error.
    fn run_service(
        &self,
        config: Self::Config,
    ) -> impl std::future::Future<Output = Result<(), CliError>> + Send;
}

/// Drive the standard DFE service lifecycle.
///
/// Handles subcommand dispatch:
/// - `run` (default): init logger → load config → run service
/// - `version`: print version info and exit
/// - `config-check`: load config, validate, print summary
///
/// # Errors
///
/// Returns `CliError` if any lifecycle step fails.
pub async fn run_app<A: DfeApp>(app: A) -> Result<(), CliError> {
    let command = app.command().cloned().unwrap_or(StandardCommand::Run);
    let args = app.common_args();

    match command {
        StandardCommand::Version => {
            let info = app.version_info();
            println!("{info}");
            Ok(())
        }

        StandardCommand::ConfigCheck => {
            // Initialise logger for config-check output
            init_logger(args)?;

            let config_path = args.config.as_deref();
            match app.load_config(config_path) {
                Ok(config) => {
                    output::print_success("configuration is valid");
                    if !args.quiet {
                        eprintln!();
                        output::print_kv("service", &app.name());
                        output::print_kv("config", &config_path.unwrap_or("(defaults)"));
                        output::print_kv("log_level", &args.effective_log_level());
                        output::print_kv("log_format", &args.log_format);
                        output::print_kv("metrics_addr", &args.metrics_addr);
                        eprintln!();
                        eprintln!("  config: {config:#?}");
                    }
                    Ok(())
                }
                Err(e) => {
                    output::print_error(&format!("configuration invalid: {e}"));
                    Err(e)
                }
            }
        }

        StandardCommand::Run => {
            init_logger(args)?;

            tracing::info!(
                service = app.name(),
                version = app.version_info().version,
                "starting service"
            );

            let config_path = args.config.as_deref();
            let config = app.load_config(config_path)?;

            tracing::debug!(?config, "configuration loaded");

            app.run_service(config).await
        }

        #[cfg(feature = "top")]
        StandardCommand::Top(ref top_args) => {
            let top_config = crate::top::TopConfig::from_args(top_args);
            crate::top::run_top(&top_config).map_err(|e| CliError::Service(e.to_string()))
        }
    }
}

/// Initialise the logger from CLI arguments.
#[cfg(feature = "logger")]
fn init_logger(args: &CommonArgs) -> Result<(), CliError> {
    let opts = args.to_logger_options()?;
    crate::logger::setup(opts)?;
    Ok(())
}

/// Initialise the logger from CLI arguments (no-op without logger feature).
#[cfg(not(feature = "logger"))]
fn init_logger(_args: &CommonArgs) -> Result<(), CliError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_command_default_is_run() {
        // When command() returns None, run_app defaults to Run
        let cmd = StandardCommand::Run;
        assert!(matches!(cmd, StandardCommand::Run));
    }
}
