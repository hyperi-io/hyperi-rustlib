// Project:   hyperi-rustlib
// File:      src/cli/mod.rs
// Purpose:   Standard CLI framework for DFE services
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Standard CLI framework for DFE Rust services.
//!
//! Provides the 80% of CLI boilerplate that every DFE service needs:
//! config path, log level/format, metrics address, version, config-check.
//! Apps provide the 20% (config type, service logic) via the [`DfeApp`] trait.
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use clap::Parser;
//! use hyperi_rustlib::cli::{CommonArgs, DfeApp, CliError, StandardCommand, VersionInfo, run_app};
//!
//! #[derive(Parser)]
//! #[command(name = "dfe-loader", version)]
//! struct App {
//!     #[command(flatten)]
//!     common: CommonArgs,
//!
//!     #[command(subcommand)]
//!     command: Option<StandardCommand>,
//! }
//!
//! impl DfeApp for App {
//!     type Config = MyConfig;
//!
//!     fn name(&self) -> &str { "dfe-loader" }
//!     fn env_prefix(&self) -> &str { "DFE_LOADER" }
//!     fn version_info(&self) -> VersionInfo {
//!         VersionInfo::new("dfe-loader", env!("CARGO_PKG_VERSION"))
//!     }
//!     fn common_args(&self) -> &CommonArgs { &self.common }
//!     fn command(&self) -> Option<&StandardCommand> { self.command.as_ref() }
//!     fn load_config(&self, path: Option<&str>) -> Result<MyConfig, CliError> { todo!() }
//!     async fn run_service(&self, config: MyConfig) -> Result<(), CliError> { todo!() }
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     let app = App::parse();
//!     if let Err(e) = run_app(app).await {
//!         eprintln!("fatal: {e}");
//!         std::process::exit(1);
//!     }
//! }
//! ```

mod app;
mod args;
mod commands;
mod error;
pub mod output;
mod runtime;
mod version;

pub use app::{DfeApp, run_app};
pub use args::CommonArgs;
pub use commands::StandardCommand;
pub use error::CliError;
pub use runtime::ServiceRuntime;
pub use version::VersionInfo;

#[cfg(feature = "top")]
pub use commands::TopArgs;
