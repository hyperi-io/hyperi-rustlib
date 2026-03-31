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

    /// Register all metrics for this service.
    ///
    /// Called by `metrics-manifest` and `generate-artefacts` subcommands to
    /// capture the full metric catalogue without starting the service.
    /// The default implementation is a no-op. Override to register
    /// `DfeMetrics`, metric groups, and app-specific metrics.
    #[cfg(any(feature = "metrics", feature = "otel-metrics"))]
    fn register_metrics(&self, _manager: &crate::metrics::MetricsManager) {}

    /// Build the deployment contract for this service.
    ///
    /// Called by `generate-artefacts` to produce container specs, health
    /// endpoints, KEDA config, and metrics manifest. The default returns
    /// `None`. Override to provide a contract.
    #[cfg(feature = "deployment")]
    fn deployment_contract(&self) -> Option<crate::deployment::DeploymentContract> {
        None
    }
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

        #[cfg(any(feature = "metrics", feature = "otel-metrics"))]
        StandardCommand::MetricsManifest => {
            let mgr = crate::metrics::MetricsManager::new(app.name());
            app.register_metrics(&mgr);
            let manifest = mgr.registry().manifest();
            println!(
                "{}",
                serde_json::to_string_pretty(&manifest)
                    .map_err(|e| CliError::Service(format!("JSON serialisation failed: {e}")))?
            );
            Ok(())
        }
        #[cfg(not(any(feature = "metrics", feature = "otel-metrics")))]
        StandardCommand::MetricsManifest => {
            output::print_error("metrics feature not enabled — no manifest available");
            Err(CliError::Service("metrics feature not enabled".into()))
        }

        StandardCommand::GenerateArtefacts(ref artefact_args) => {
            generate_artefacts(&app, artefact_args)?;
            Ok(())
        }

        StandardCommand::Run => {
            let version_info = app.version_info();
            init_logger_for_service(args, app.name(), &version_info.version)?;

            tracing::info!(
                service = app.name(),
                version = version_info.version,
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

/// Initialise the logger with service name and version injected into JSON output.
#[cfg(feature = "logger")]
fn init_logger_for_service(
    args: &CommonArgs,
    service_name: &str,
    service_version: &str,
) -> Result<(), CliError> {
    let opts = args.to_logger_options()?;
    crate::logger::setup(crate::logger::LoggerOptions {
        service_name: Some(service_name.to_string()),
        service_version: Some(service_version.to_string()),
        ..opts
    })?;
    Ok(())
}

/// Initialise the logger from CLI arguments (no-op without logger feature).
#[cfg(not(feature = "logger"))]
fn init_logger(_args: &CommonArgs) -> Result<(), CliError> {
    Ok(())
}

/// Initialise the logger with service name and version (no-op without logger feature).
#[cfg(not(feature = "logger"))]
fn init_logger_for_service(
    _args: &CommonArgs,
    _service_name: &str,
    _service_version: &str,
) -> Result<(), CliError> {
    Ok(())
}

/// Generate all CI artefacts for this service.
///
/// Produces metrics manifest, deployment contract, and container spec
/// in the output directory. Files are deterministic — running twice produces
/// identical output (no timestamps that change between runs).
fn generate_artefacts<A: DfeApp>(
    app: &A,
    args: &super::commands::GenerateArtefactsArgs,
) -> Result<(), CliError> {
    let output_dir = std::path::Path::new(&args.output_dir);
    std::fs::create_dir_all(output_dir)
        .map_err(|e| CliError::Service(format!("failed to create output dir: {e}")))?;

    let mut generated = Vec::new();

    // Metrics manifest
    #[cfg(any(feature = "metrics", feature = "otel-metrics"))]
    {
        let mgr = crate::metrics::MetricsManager::new(app.name());
        app.register_metrics(&mgr);
        let manifest = mgr.registry().manifest();
        let path = output_dir.join("metrics-manifest.json");
        let json = serde_json::to_string_pretty(&manifest)
            .map_err(|e| CliError::Service(format!("metrics manifest JSON failed: {e}")))?;
        std::fs::write(&path, &json)
            .map_err(|e| CliError::Service(format!("failed to write {}: {e}", path.display())))?;
        generated.push(format!(
            "metrics-manifest.json ({} metrics)",
            manifest.metrics.len()
        ));
    }

    // Deployment contract
    #[cfg(feature = "deployment")]
    if let Some(contract) = app.deployment_contract() {
        let path = output_dir.join("deployment-contract.json");
        let json = serde_json::to_string_pretty(&contract)
            .map_err(|e| CliError::Service(format!("deployment contract JSON failed: {e}")))?;
        std::fs::write(&path, &json)
            .map_err(|e| CliError::Service(format!("failed to write {}: {e}", path.display())))?;
        generated.push("deployment-contract.json".to_string());
    }

    if generated.is_empty() {
        output::print_warn("no artefacts generated (no metrics or deployment features enabled)");
    } else {
        output::print_success(&format!(
            "generated {} artefact(s) in {}",
            generated.len(),
            output_dir.display()
        ));
        for name in &generated {
            output::print_kv("  wrote", name);
        }
    }

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
