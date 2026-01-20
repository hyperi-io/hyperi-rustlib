// Project:   hs-rustlib
// File:      src/lib.rs
// Purpose:   Main library entry point and public API exports
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! # hs-rustlib
//!
//! Shared utility library for HyperSec Rust applications.
//!
//! Provides configuration management, structured logging, Prometheus metrics,
//! and environment detection - matching the functionality of hs-lib (Python)
//! and hs-golib (Go).
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use hs_rustlib::{env, config, logger, metrics};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Detect runtime environment
//!     let environment = env::Environment::detect();
//!     println!("Running in: {:?}", environment);
//!
//!     // Initialise logger (respects LOG_LEVEL env var)
//!     logger::setup_default()?;
//!
//!     // Load configuration with 7-layer cascade
//!     config::setup(config::ConfigOptions {
//!         env_prefix: "MYAPP".into(),
//!         ..Default::default()
//!     })?;
//!
//!     // Access config
//!     let cfg = config::get();
//!     let db_host = cfg.get_string("database.host").unwrap_or_default();
//!
//!     // Create metrics
//!     let metrics_mgr = metrics::MetricsManager::new("myapp");
//!     let _counter = metrics_mgr.counter("requests_total", "Total requests processed");
//!
//!     tracing::info!(db_host = %db_host, "Application started");
//!     Ok(())
//! }
//! ```

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::doc_markdown)] // Allow brand names without backticks
#![allow(clippy::cast_precision_loss)] // Metrics values are fine with f64 precision
#![allow(clippy::missing_panics_doc)] // MVP does not require exhaustive docs
#![allow(clippy::missing_errors_doc)] // MVP does not require exhaustive docs
#![allow(clippy::double_must_use)] // Return types already marked must_use
#![allow(clippy::unused_async)] // Async for future compatibility
#![allow(clippy::redundant_closure_for_method_calls)] // Clearer with explicit closure
#![allow(clippy::result_large_err)] // figment::Error is large by design
#![allow(clippy::needless_pass_by_value)] // API cleaner with owned values

// Core modules (always available)
pub mod env;

#[cfg(feature = "runtime")]
pub mod runtime;

#[cfg(feature = "config")]
pub mod config;

#[cfg(feature = "logger")]
pub mod logger;

#[cfg(feature = "metrics")]
pub mod metrics;

#[cfg(feature = "transport")]
pub mod transport;

#[cfg(feature = "http-server")]
pub mod http_server;

#[cfg(feature = "spool")]
pub mod spool;

#[cfg(feature = "tiered-sink")]
pub mod tiered_sink;

#[cfg(feature = "license")]
pub mod license;

// Re-export common types at crate root
pub use env::Environment;

#[cfg(feature = "runtime")]
pub use runtime::RuntimePaths;

#[cfg(feature = "config")]
pub use config::{Config, ConfigError, ConfigOptions};

#[cfg(feature = "logger")]
pub use logger::{LogFormat, LoggerError, LoggerOptions};

#[cfg(feature = "metrics")]
pub use metrics::{MetricsConfig, MetricsError, MetricsManager};

#[cfg(feature = "transport")]
pub use transport::{
    CommitToken, Message, PayloadFormat, SendResult, Transport, TransportConfig, TransportError,
    TransportResult, TransportType,
};

#[cfg(feature = "http-server")]
pub use http_server::{HttpServer, HttpServerConfig, HttpServerError};

#[cfg(feature = "spool")]
pub use spool::{Spool, SpoolConfig, SpoolError};

#[cfg(feature = "tiered-sink")]
pub use tiered_sink::{
    CircuitBreaker, CircuitState, CompressionCodec, DrainStrategy, OrderingMode, Sink, SinkError,
    TieredSink, TieredSinkConfig, TieredSinkError,
};

#[cfg(feature = "license")]
pub use license::{License, LicenseError, LicenseOptions, LicenseSettings, LicenseSource};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Initialise all library components with default settings.
///
/// This is a convenience function that:
/// 1. Detects the runtime environment
/// 2. Sets up the logger with auto-detection
/// 3. Loads configuration with the given env prefix
///
/// # Errors
///
/// Returns an error if logger or config initialisation fails.
#[cfg(all(feature = "config", feature = "logger"))]
pub fn init(env_prefix: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    logger::setup_default()?;
    config::setup(config::ConfigOptions {
        env_prefix: env_prefix.to_string(),
        ..Default::default()
    })?;
    Ok(())
}
