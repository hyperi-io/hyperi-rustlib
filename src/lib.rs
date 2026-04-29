// Project:   hyperi-rustlib
// File:      src/lib.rs
// Purpose:   Main library entry point and public API exports
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # hyperi-rustlib
//!
//! Shared utility library for HyperI Rust applications.
//!
//! Provides configuration management, structured logging, Prometheus metrics,
//! and environment detection — matching the functionality of hyperi-pylib (Python)
//! and hyperi-golib (Go).
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use hyperi_rustlib::{env, config, logger, metrics};
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
//!
//! See `docs/CORE-PILLARS.md` in the repository for the auto-wiring architecture.

#![deny(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(clippy::pedantic)]
#![warn(rustdoc::broken_intra_doc_links)]
#![warn(rustdoc::private_intra_doc_links)]
#![warn(rustdoc::invalid_codeblock_attributes)]
#![warn(rustdoc::bare_urls)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::doc_markdown)] // Allow brand names without backticks
#![allow(clippy::cast_precision_loss)] // Metrics values are fine with f64 precision
#![allow(clippy::missing_panics_doc)] // MVP does not require exhaustive docs
#![allow(clippy::missing_errors_doc)] // MVP does not require exhaustive docs
#![allow(clippy::double_must_use)] // Return types already marked must_use
#![allow(clippy::unused_async)] // Async for future compatibility
#![allow(clippy::redundant_closure_for_method_calls)] // Clearer with explicit closure
#![allow(clippy::result_large_err)] // figment::Error is large by design
#![allow(clippy::needless_pass_by_value)]
// API cleaner with owned values
// Test code allowances - unwrap is acceptable in tests for cleaner assertions
#![cfg_attr(test, allow(clippy::unwrap_used))]
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

// Core modules (always available)
pub mod env;
pub mod kafka_config;
pub mod sensitive;

#[cfg(feature = "runtime")]
#[cfg_attr(docsrs, doc(cfg(feature = "runtime")))]
pub mod runtime;

#[cfg(feature = "shutdown")]
#[cfg_attr(docsrs, doc(cfg(feature = "shutdown")))]
pub mod shutdown;

#[cfg(feature = "health")]
#[cfg_attr(docsrs, doc(cfg(feature = "health")))]
pub mod health;

#[cfg(feature = "config")]
#[cfg_attr(docsrs, doc(cfg(feature = "config")))]
pub mod config;

#[cfg(feature = "logger")]
#[cfg_attr(docsrs, doc(cfg(feature = "logger")))]
pub mod logger;

#[cfg(any(feature = "metrics", feature = "otel-metrics"))]
#[cfg_attr(docsrs, doc(cfg(any(feature = "metrics", feature = "otel-metrics"))))]
pub mod metrics;

#[cfg(feature = "otel-tracing")]
#[cfg_attr(docsrs, doc(cfg(feature = "otel-tracing")))]
pub mod otel_tracing;

#[cfg(feature = "transport")]
#[cfg_attr(docsrs, doc(cfg(feature = "transport")))]
pub mod transport;

#[cfg(feature = "http")]
#[cfg_attr(docsrs, doc(cfg(feature = "http")))]
pub mod http_client;

#[cfg(feature = "http-server")]
#[cfg_attr(docsrs, doc(cfg(feature = "http-server")))]
pub mod http_server;

#[cfg(feature = "database")]
#[cfg_attr(docsrs, doc(cfg(feature = "database")))]
pub mod database;

#[cfg(feature = "cache")]
#[cfg_attr(docsrs, doc(cfg(feature = "cache")))]
pub mod cache;

#[cfg(feature = "spool")]
#[cfg_attr(docsrs, doc(cfg(feature = "spool")))]
pub mod spool;

#[cfg(feature = "tiered-sink")]
#[cfg_attr(docsrs, doc(cfg(feature = "tiered-sink")))]
pub mod tiered_sink;

#[cfg(feature = "secrets")]
#[cfg_attr(docsrs, doc(cfg(feature = "secrets")))]
pub mod secrets;

#[cfg(feature = "directory-config")]
#[cfg_attr(docsrs, doc(cfg(feature = "directory-config")))]
pub mod directory_config;

#[cfg(feature = "memory")]
#[cfg_attr(docsrs, doc(cfg(feature = "memory")))]
pub mod memory;

#[cfg(feature = "scaling")]
#[cfg_attr(docsrs, doc(cfg(feature = "scaling")))]
pub mod scaling;

#[cfg(feature = "worker")]
#[cfg_attr(docsrs, doc(cfg(feature = "worker")))]
pub mod worker;

#[cfg(feature = "cli")]
#[cfg_attr(docsrs, doc(cfg(feature = "cli")))]
pub mod cli;

#[cfg(feature = "top")]
#[cfg_attr(docsrs, doc(cfg(feature = "top")))]
pub mod top;

#[cfg(feature = "io")]
#[cfg_attr(docsrs, doc(cfg(feature = "io")))]
pub mod io;

#[cfg(feature = "dlq")]
#[cfg_attr(docsrs, doc(cfg(feature = "dlq")))]
pub mod dlq;

#[cfg(feature = "output-file")]
#[cfg_attr(docsrs, doc(cfg(feature = "output-file")))]
pub mod output;

#[cfg(feature = "expression")]
#[cfg_attr(docsrs, doc(cfg(feature = "expression")))]
pub mod expression;

#[cfg(feature = "deployment")]
#[cfg_attr(docsrs, doc(cfg(feature = "deployment")))]
pub mod deployment;

#[cfg(feature = "version-check")]
#[cfg_attr(docsrs, doc(cfg(feature = "version-check")))]
pub mod version_check;

// Re-export common types at crate root
pub use env::{Environment, RuntimeContext, runtime_context};
pub use kafka_config::{
    DfeSource, KafkaConfigError, KafkaConfigResult, ServiceRole, TOPIC_SUFFIX_LAND,
    TOPIC_SUFFIX_LOAD, config_from_file, config_from_properties_str,
};
pub use sensitive::SensitiveString;

#[cfg(feature = "runtime")]
#[cfg_attr(docsrs, doc(cfg(feature = "runtime")))]
pub use runtime::RuntimePaths;

#[cfg(feature = "health")]
#[cfg_attr(docsrs, doc(cfg(feature = "health")))]
pub use health::{HealthRegistry, HealthStatus};

#[cfg(feature = "config")]
#[cfg_attr(docsrs, doc(cfg(feature = "config")))]
pub use config::{Config, ConfigError, ConfigOptions};

#[cfg(feature = "config")]
#[cfg_attr(docsrs, doc(cfg(feature = "config")))]
pub use config::flat_env::{ApplyFlatEnv, EnvVarDoc, Normalize};

#[cfg(feature = "config-reload")]
#[cfg_attr(docsrs, doc(cfg(feature = "config-reload")))]
pub use config::reloader::{ConfigReloader, ReloaderConfig};

#[cfg(feature = "config-reload")]
#[cfg_attr(docsrs, doc(cfg(feature = "config-reload")))]
pub use config::shared::SharedConfig;

#[cfg(feature = "config-postgres")]
#[cfg_attr(docsrs, doc(cfg(feature = "config-postgres")))]
pub use config::postgres::{
    FallbackMode, PostgresConfig, PostgresConfigError, PostgresConfigSource,
};

#[cfg(feature = "logger")]
#[cfg_attr(docsrs, doc(cfg(feature = "logger")))]
pub use logger::{
    LogFormat, LoggerError, LoggerOptions, SecurityEvent, SecurityOutcome, ThrottleConfig,
};

#[cfg(any(feature = "metrics", feature = "otel-metrics"))]
#[cfg_attr(docsrs, doc(cfg(any(feature = "metrics", feature = "otel-metrics"))))]
pub use metrics::{DfeMetrics, MetricsConfig, MetricsError, MetricsManager};

#[cfg(feature = "otel-metrics")]
#[cfg_attr(docsrs, doc(cfg(feature = "otel-metrics")))]
pub use metrics::{OtelMetricsConfig, OtelProtocol};

#[cfg(feature = "transport")]
#[cfg_attr(docsrs, doc(cfg(feature = "transport")))]
pub use transport::{
    CommitToken, Message, PayloadFormat, SendResult, Transport, TransportConfig, TransportError,
    TransportResult, TransportType,
};

#[cfg(feature = "http-server")]
#[cfg_attr(docsrs, doc(cfg(feature = "http-server")))]
pub use http_server::{HttpServer, HttpServerConfig, HttpServerError};

#[cfg(feature = "spool")]
#[cfg_attr(docsrs, doc(cfg(feature = "spool")))]
pub use spool::{Spool, SpoolConfig, SpoolError};

#[cfg(feature = "tiered-sink")]
#[cfg_attr(docsrs, doc(cfg(feature = "tiered-sink")))]
pub use tiered_sink::{
    CircuitBreaker, CircuitState, CompressionCodec, DrainStrategy, OrderingMode, Sink, SinkError,
    TieredSink, TieredSinkConfig, TieredSinkError,
};

#[cfg(feature = "secrets")]
#[cfg_attr(docsrs, doc(cfg(feature = "secrets")))]
pub use secrets::{
    CacheConfig, FileProvider, RotationEvent, SecretMetadata, SecretProvider, SecretSource,
    SecretValue, SecretsConfig, SecretsError, SecretsManager, SecretsResult,
};

#[cfg(feature = "secrets-vault")]
#[cfg_attr(docsrs, doc(cfg(feature = "secrets-vault")))]
pub use secrets::{OpenBaoAuth, OpenBaoConfig, OpenBaoProvider};

#[cfg(feature = "secrets-aws")]
#[cfg_attr(docsrs, doc(cfg(feature = "secrets-aws")))]
pub use secrets::{AwsConfig, AwsProvider};

#[cfg(feature = "directory-config")]
#[cfg_attr(docsrs, doc(cfg(feature = "directory-config")))]
pub use directory_config::{
    ChangeEvent, ChangeOperation, DirectoryConfigError, DirectoryConfigResult,
    DirectoryConfigStore, DirectoryConfigStoreConfig, WriteMode, WriteResult,
};

#[cfg(feature = "memory")]
#[cfg_attr(docsrs, doc(cfg(feature = "memory")))]
pub use memory::{MemoryGuard, MemoryGuardConfig, MemoryPressure, detect_memory_limit};

#[cfg(feature = "scaling")]
#[cfg_attr(docsrs, doc(cfg(feature = "scaling")))]
pub use scaling::{
    ComponentSnapshot, GateType, PressureSnapshot, RateWindow, ScalingComponent, ScalingPressure,
    ScalingPressureConfig,
};

#[cfg(feature = "worker")]
#[cfg_attr(docsrs, doc(cfg(feature = "worker")))]
pub use worker::{
    AccumulatorConfig, AccumulatorFull, AdaptiveWorkerPool, BatchAccumulator, BatchDrainer,
    BatchPipeline, BatchProcessor, PipelineStats, PipelineStatsSnapshot, ScalingDecision,
    ScalingInput, WorkerPoolConfig,
};

#[cfg(feature = "cli")]
#[cfg_attr(docsrs, doc(cfg(feature = "cli")))]
pub use cli::{CliError, CommonArgs, DfeApp, ServiceRuntime, StandardCommand, VersionInfo};

#[cfg(feature = "io")]
#[cfg_attr(docsrs, doc(cfg(feature = "io")))]
pub use io::{FileWriterConfig, NdjsonWriter, RotationPeriod};

#[cfg(feature = "dlq")]
#[cfg_attr(docsrs, doc(cfg(feature = "dlq")))]
pub use dlq::{
    Dlq, DlqBackend, DlqConfig, DlqEntry, DlqError, DlqMode, DlqSource, FileDlq, FileDlqConfig,
};

#[cfg(feature = "dlq-kafka")]
#[cfg_attr(docsrs, doc(cfg(feature = "dlq-kafka")))]
pub use dlq::{DlqRouting, KafkaDlq, KafkaDlqConfig};

#[cfg(feature = "dlq-http")]
#[cfg_attr(docsrs, doc(cfg(feature = "dlq-http")))]
pub use dlq::{HttpDlq, HttpDlqConfig};

#[cfg(feature = "dlq-redis")]
#[cfg_attr(docsrs, doc(cfg(feature = "dlq-redis")))]
pub use dlq::{RedisDlq, RedisDlqConfig};

#[cfg(feature = "output-file")]
#[cfg_attr(docsrs, doc(cfg(feature = "output-file")))]
pub use output::{FileOutput, FileOutputConfig, OutputError};

#[cfg(feature = "expression")]
#[cfg_attr(docsrs, doc(cfg(feature = "expression")))]
pub use expression::{
    ALLOWED_FUNCTIONS, DISALLOWED_FUNCTIONS, ExpressionError, ExpressionResult, build_context,
    compile, evaluate, evaluate_condition, validate,
};

#[cfg(feature = "deployment")]
#[cfg_attr(docsrs, doc(cfg(feature = "deployment")))]
pub use deployment::{
    ContractMismatch, DeploymentContract, DeploymentError, HealthContract, KedaConfig, KedaContract,
};

#[cfg(feature = "version-check")]
#[cfg_attr(docsrs, doc(cfg(feature = "version-check")))]
pub use version_check::{VersionCheck, VersionCheckConfig, VersionCheckResponse};

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
