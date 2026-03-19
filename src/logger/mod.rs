// Project:   hyperi-rustlib
// File:      src/logger/mod.rs
// Purpose:   Structured logging with JSON output and sensitive data masking
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Structured logging with JSON output and sensitive data masking.
//!
//! Provides production-ready logging matching hyperi-pylib (Python) and hyperi-golib (Go).
//! Automatically detects terminal vs container environment for format selection.
//!
//! ## Features
//!
//! - RFC 3339 timestamps with timezone
//! - JSON output for containers, coloured text for terminals
//! - Sensitive data masking (passwords, tokens, API keys)
//! - Environment variable overrides (LOG_LEVEL, LOG_FORMAT, NO_COLOR)
//!
//! ## Example
//!
//! ```rust,no_run
//! use hyperi_rustlib::logger;
//!
//! // Initialise with defaults (auto-detects format)
//! logger::setup_default().unwrap();
//!
//! // Use tracing macros
//! tracing::info!(user_id = 123, "User logged in");
//! tracing::error!(error = "connection failed", "Database error");
//! ```

pub mod format;
pub mod helpers;
mod masking;
pub mod security;

use std::io;
use std::sync::OnceLock;

use thiserror::Error;
use tracing::Level;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer as _;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use tracing_throttle::{Policy, TracingRateLimitLayer};

pub use helpers::{log_debounced, log_sampled, log_state_change};
pub use masking::{MaskingLayer, MaskingWriter, default_sensitive_fields, mask_sensitive_string};
pub use security::{SecurityEvent, SecurityOutcome};

/// Global flag to track initialisation.
static LOGGER_INIT: OnceLock<()> = OnceLock::new();

/// Logger errors.
#[derive(Debug, Error)]
pub enum LoggerError {
    /// Logger already initialised.
    #[error("logger already initialised")]
    AlreadyInitialised,

    /// Failed to set global subscriber.
    #[error("failed to set global subscriber: {0}")]
    SetGlobalError(String),

    /// Invalid log level.
    #[error("invalid log level: {0}")]
    InvalidLevel(String),
}

/// Log output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFormat {
    /// JSON output (for containers/log aggregators).
    Json,
    /// Human-readable coloured text.
    Text,
    /// Auto-detect based on environment (JSON in containers, Text on TTY).
    #[default]
    Auto,
}

impl LogFormat {
    /// Resolve Auto to a concrete format.
    #[must_use]
    pub fn resolve(self) -> Self {
        match self {
            Self::Auto => {
                if is_terminal() && !is_no_color() {
                    Self::Text
                } else {
                    Self::Json
                }
            }
            other => other,
        }
    }
}

impl std::str::FromStr for LogFormat {
    type Err = LoggerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "text" | "pretty" | "human" => Ok(Self::Text),
            "auto" => Ok(Self::Auto),
            _ => Err(LoggerError::InvalidLevel(s.to_string())),
        }
    }
}

/// Log throttle configuration.
///
/// Controls global rate limiting via `tracing-throttle`. Disabled by default.
/// When enabled, identical log events are deduplicated using a token bucket policy.
#[derive(Debug, Clone)]
pub struct ThrottleConfig {
    /// Enable log throttling.
    pub enabled: bool,
    /// Token bucket burst capacity (max events before throttling starts).
    pub burst: f64,
    /// Token recovery rate (tokens per second).
    pub rate: f64,
    /// Maximum number of distinct event signatures to track.
    pub max_signatures: usize,
    /// High-cardinality fields to exclude from signature matching.
    /// Events differing only in these fields will be treated as identical.
    pub excluded_fields: Vec<String>,
}

impl Default for ThrottleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            burst: 50.0,
            rate: 1.0,
            max_signatures: 10_000,
            excluded_fields: vec![
                "request_id".to_string(),
                "trace_id".to_string(),
                "span_id".to_string(),
            ],
        }
    }
}

/// Logger configuration options.
#[derive(Debug, Clone)]
pub struct LoggerOptions {
    /// Log level (DEBUG, INFO, WARN, ERROR).
    pub level: Level,
    /// Output format.
    pub format: LogFormat,
    /// Include source file and line in output.
    pub add_source: bool,
    /// Enable sensitive data masking.
    pub enable_masking: bool,
    /// Field names to mask.
    pub sensitive_fields: Vec<String>,
    /// Include span events.
    pub span_events: bool,
    /// Log throttle configuration (deduplicate identical events).
    pub throttle: ThrottleConfig,
    /// Service name injected into JSON log output.
    /// Auto-populated by DfeApp. Falls back to SERVICE_NAME env var.
    pub service_name: Option<String>,
    /// Service version injected into JSON log output.
    /// Auto-populated by DfeApp. Falls back to SERVICE_VERSION env var.
    pub service_version: Option<String>,
}

impl Default for LoggerOptions {
    fn default() -> Self {
        Self {
            level: Level::INFO,
            format: LogFormat::Auto,
            add_source: true,
            enable_masking: true,
            sensitive_fields: default_sensitive_fields(),
            span_events: false,
            throttle: ThrottleConfig::default(),
            service_name: None,
            service_version: None,
        }
    }
}

/// Initialise the global logger with custom options.
///
/// # Errors
///
/// Returns an error if the logger is already initialised.
pub fn setup(opts: LoggerOptions) -> Result<(), LoggerError> {
    if LOGGER_INIT.get().is_some() {
        return Err(LoggerError::AlreadyInitialised);
    }

    let format = opts.format.resolve();

    // Build the env filter
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(opts.level.to_string()));

    // RFC 3339 timestamp format
    let timer = UtcTime::rfc_3339();

    let span_events = if opts.span_events {
        FmtSpan::NEW | FmtSpan::CLOSE
    } else {
        FmtSpan::NONE
    };

    // Build sensitive fields set for masking writer
    let sensitive: std::collections::HashSet<String> = if opts.enable_masking {
        opts.sensitive_fields
            .iter()
            .map(|s| s.to_lowercase())
            .collect()
    } else {
        std::collections::HashSet::new()
    };

    // Build optional throttle filter
    let throttle_filter = build_throttle_filter(&opts.throttle);

    match format {
        LogFormat::Json => {
            let writer = masking::make_masking_writer(
                sensitive,
                true,
                opts.service_name.clone(),
                opts.service_version.clone(),
            );
            let layer = tracing_subscriber::fmt::layer()
                .json()
                .with_timer(timer)
                .with_file(opts.add_source)
                .with_line_number(opts.add_source)
                .with_target(true)
                .with_span_events(span_events)
                .with_writer(writer);

            if let Some(throttle) = throttle_filter {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(layer.with_filter(throttle))
                    .try_init()
                    .map_err(|e| LoggerError::SetGlobalError(e.to_string()))?;
            } else {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(layer)
                    .try_init()
                    .map_err(|e| LoggerError::SetGlobalError(e.to_string()))?;
            }
        }
        LogFormat::Text => {
            let writer = masking::make_masking_writer(sensitive, false, None, None);
            let ansi = !is_no_color();
            let formatter = format::ColouredFormatter::new(ansi)
                .with_file(opts.add_source)
                .with_line_number(opts.add_source);
            let layer = tracing_subscriber::fmt::layer()
                .with_ansi(ansi)
                .with_span_events(span_events)
                .event_format(formatter)
                .with_writer(writer);

            if let Some(throttle) = throttle_filter {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(layer.with_filter(throttle))
                    .try_init()
                    .map_err(|e| LoggerError::SetGlobalError(e.to_string()))?;
            } else {
                tracing_subscriber::registry()
                    .with(filter)
                    .with(layer)
                    .try_init()
                    .map_err(|e| LoggerError::SetGlobalError(e.to_string()))?;
            }
        }
        LogFormat::Auto => unreachable!("Auto should be resolved"),
    }

    let _ = LOGGER_INIT.set(());
    Ok(())
}

/// Initialise the global logger with default settings.
///
/// Respects environment variables:
/// - `LOG_LEVEL` or `RUST_LOG`: Log level
/// - `LOG_FORMAT`: Output format (json, text, auto)
/// - `NO_COLOR`: Disable coloured output
/// - `LOG_THROTTLE_ENABLED`: Enable log deduplication (default: false)
/// - `LOG_THROTTLE_BURST`: Token bucket burst capacity (default: 50)
/// - `LOG_THROTTLE_RATE`: Token recovery rate per second (default: 1.0)
///
/// # Errors
///
/// Returns an error if the logger is already initialised.
pub fn setup_default() -> Result<(), LoggerError> {
    let level = std::env::var("LOG_LEVEL")
        .or_else(|_| std::env::var("RUST_LOG"))
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(Level::INFO);

    let format = std::env::var("LOG_FORMAT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(LogFormat::Auto);

    let throttle_enabled = std::env::var("LOG_THROTTLE_ENABLED")
        .ok()
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

    let throttle_burst = std::env::var("LOG_THROTTLE_BURST")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50.0);

    let throttle_rate = std::env::var("LOG_THROTTLE_RATE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);

    let service_name = std::env::var("SERVICE_NAME").ok();
    let service_version = std::env::var("SERVICE_VERSION").ok();

    setup(LoggerOptions {
        level,
        format,
        throttle: ThrottleConfig {
            enabled: throttle_enabled,
            burst: throttle_burst,
            rate: throttle_rate,
            ..Default::default()
        },
        service_name,
        service_version,
        ..Default::default()
    })
}

/// Build an optional throttle filter from configuration.
fn build_throttle_filter(config: &ThrottleConfig) -> Option<TracingRateLimitLayer> {
    if !config.enabled {
        return None;
    }

    let policy = Policy::token_bucket(config.burst, config.rate)
        .unwrap_or_else(|_| Policy::token_bucket(50.0, 1.0).expect("default policy is valid"));

    let mut builder = TracingRateLimitLayer::builder()
        .with_policy(policy)
        .with_max_signatures(config.max_signatures);

    if !config.excluded_fields.is_empty() {
        builder = builder.with_excluded_fields(config.excluded_fields.clone());
    }

    match builder.build() {
        Ok(layer) => Some(layer),
        Err(e) => {
            eprintln!("Failed to build log throttle layer: {e}");
            None
        }
    }
}

/// Check if stderr is a terminal.
fn is_terminal() -> bool {
    use std::io::IsTerminal;
    io::stderr().is_terminal()
}

/// Check if NO_COLOR environment variable is set.
fn is_no_color() -> bool {
    std::env::var("NO_COLOR").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_format_from_str() {
        assert_eq!("json".parse::<LogFormat>().unwrap(), LogFormat::Json);
        assert_eq!("text".parse::<LogFormat>().unwrap(), LogFormat::Text);
        assert_eq!("pretty".parse::<LogFormat>().unwrap(), LogFormat::Text);
        assert_eq!("auto".parse::<LogFormat>().unwrap(), LogFormat::Auto);
    }

    #[test]
    fn test_log_format_resolve() {
        // Json and Text should stay as-is
        assert_eq!(LogFormat::Json.resolve(), LogFormat::Json);
        assert_eq!(LogFormat::Text.resolve(), LogFormat::Text);

        // Auto resolves based on environment
        let resolved = LogFormat::Auto.resolve();
        assert!(matches!(resolved, LogFormat::Json | LogFormat::Text));
    }

    #[test]
    fn test_logger_options_default() {
        let opts = LoggerOptions::default();
        assert_eq!(opts.level, Level::INFO);
        assert_eq!(opts.format, LogFormat::Auto);
        assert!(opts.add_source);
        assert!(opts.enable_masking);
        assert!(!opts.sensitive_fields.is_empty());
    }

    #[test]
    fn test_is_no_color() {
        temp_env::with_var("NO_COLOR", None::<&str>, || assert!(!is_no_color()));
        temp_env::with_var("NO_COLOR", Some("1"), || assert!(is_no_color()));
    }
}
