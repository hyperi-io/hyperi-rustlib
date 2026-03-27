// Project:   hyperi-rustlib
// File:      src/dlq/mod.rs
// Purpose:   Unified dead letter queue with pluggable backends
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Unified dead letter queue (DLQ) with pluggable backends.
//!
//! Provides a shared DLQ abstraction for all DFE services. Failed messages
//! are routed to one or more backends (file, Kafka, or custom) using
//! configurable cascade or fan-out modes.
//!
//! ## Backends
//!
//! - **File** (`FileDlq`): NDJSON files with automatic rotation and cleanup.
//!   Always available, no external dependencies.
//! - **Kafka** (`KafkaDlq`): Routes to Kafka topics with per-table or common
//!   routing. Requires the `dlq-kafka` feature.
//! - **Custom**: Implement [`DlqBackend`] and register via [`Dlq::add_backend`].
//!
//! ## Modes
//!
//! - **Cascade** (default): Try Kafka first, fall back to file on failure.
//! - **Fan-out**: Write to all backends for belt-and-suspenders.
//! - **FileOnly**: File backend only (no Kafka dependency).
//! - **KafkaOnly**: Kafka backend only.
//!
//! ## Example
//!
//! ```rust,no_run
//! use hyperi_rustlib::dlq::{Dlq, DlqConfig, DlqEntry, DlqSource};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // File-only DLQ (no Kafka needed)
//! let config = DlqConfig::default();
//! let dlq = Dlq::file_only(&config, "my-service")?;
//!
//! // Route a failed message
//! let entry = DlqEntry::new("my-service", "parse_error", b"bad data".to_vec())
//!     .with_destination("acme.auth")
//!     .with_source(DlqSource::kafka("events", 1, 42));
//!
//! dlq.send(entry).await?;
//! # Ok(())
//! # }
//! ```

mod backend;
mod config;
mod entry;
mod error;
mod file;
mod orchestrator;

#[cfg(feature = "dlq-kafka")]
mod kafka;

#[cfg(feature = "dlq-http")]
mod http;

#[cfg(feature = "dlq-redis")]
mod redis_dlq;

// Core types (always available with `dlq` feature)
pub use backend::DlqBackend;
pub use config::{DlqConfig, DlqMode, FileDlqConfig, RotationPeriod};
pub use entry::{DlqEntry, DlqSource};
pub use error::DlqError;
pub use file::FileDlq;
pub use orchestrator::Dlq;

// Kafka types (only with `dlq-kafka` feature)
#[cfg(feature = "dlq-kafka")]
pub use config::{DlqRouting, KafkaDlqConfig};
#[cfg(feature = "dlq-kafka")]
pub use kafka::KafkaDlq;

// HTTP types (only with `dlq-http` feature)
#[cfg(feature = "dlq-http")]
pub use http::{HttpDlq, HttpDlqConfig};

// Redis types (only with `dlq-redis` feature)
#[cfg(feature = "dlq-redis")]
pub use redis_dlq::{RedisDlq, RedisDlqConfig};

/// Result type for DLQ operations.
pub type Result<T> = std::result::Result<T, DlqError>;
