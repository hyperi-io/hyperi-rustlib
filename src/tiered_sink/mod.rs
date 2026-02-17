// Project:   hyperi-rustlib
// File:      src/tiered_sink/mod.rs
// Purpose:   Tiered sink with disk spillover for resilient message delivery
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Tiered sink with automatic disk spillover for resilient message delivery.
//!
//! This module provides a wrapper around any async sink (Kafka, S3, HTTP, etc.)
//! that automatically spills messages to disk when the primary sink is unavailable
//! or backpressuring, then drains them back when the sink recovers.
//!
//! ## Design
//!
//! ```text
//!                     ┌─────────────────────────────────────┐
//!                     │           TieredSink                │
//!                     │                                     │
//!    Message ────────►│  try_send() to primary sink        │
//!                     │         │                           │
//!                     │         ▼                           │
//!                     │    ┌─────────┐                      │
//!                     │    │ Success │──► Done (hot path)   │
//!                     │    └────┬────┘                      │
//!                     │         │ Err(Full/Unavailable)     │
//!                     │         ▼                           │
//!                     │    ┌─────────┐                      │
//!                     │    │  Spool  │──► Disk (cold path)  │
//!                     │    └────┬────┘                      │
//!                     │         │                           │
//!                     │    Background drain task            │
//!                     │    (when primary recovers)          │
//!                     └─────────────────────────────────────┘
//! ```
//!
//! ## Features
//!
//! - **Hot path first**: Always tries primary sink with timeout
//! - **Automatic spillover**: Writes to disk only when primary fails
//! - **Circuit breaker**: Avoids hammering a dead sink
//! - **Background drain**: Recovers spooled messages when sink is healthy
//! - **Configurable ordering**: Interleaved (default) or strict FIFO
//! - **Multiple compression codecs**: LZ4 (default), Snappy, Zstd, None
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::tiered_sink::{TieredSink, TieredSinkConfig, Sink, SinkError};
//!
//! // Implement Sink for your backend
//! struct MyKafkaSink { /* ... */ }
//!
//! #[async_trait::async_trait]
//! impl Sink for MyKafkaSink {
//!     type Error = MyError;
//!
//!     async fn try_send(&self, data: &[u8]) -> Result<(), SinkError<Self::Error>> {
//!         // Send to Kafka...
//!         Ok(())
//!     }
//! }
//!
//! // Wrap with TieredSink
//! let kafka = MyKafkaSink::new();
//! let config = TieredSinkConfig::new("/var/spool/myapp.queue");
//! let tiered = TieredSink::new(kafka, config).await?;
//!
//! // Use tiered - automatically spills to disk if Kafka is down
//! tiered.send(b"my message").await?;
//! ```

mod circuit;
mod codec;
mod config;
mod drainer;
mod error;
mod sink;
mod tiered;

pub use circuit::{CircuitBreaker, CircuitState};
pub use codec::CompressionCodec;
pub use config::{DrainStrategy, OrderingMode, TieredSinkConfig};
pub use error::TieredSinkError;
pub use sink::{Sink, SinkError};
pub use tiered::TieredSink;

/// Result type for tiered sink operations.
pub type Result<T> = std::result::Result<T, TieredSinkError>;
