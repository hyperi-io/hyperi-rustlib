// Project:   hyperi-rustlib
// File:      src/dlq/backend.rs
// Purpose:   DlqBackend enum -- variant per supported backend
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Static enum dispatch for DLQ backends.
//!
//! Replaces the previous `#[async_trait] trait DlqBackend` + `Box<dyn>`
//! shape. Each backend is a concrete variant; the drain matches over
//! variants. No vtable, no `async_trait` macro, no heap-boxed future.
//!
//! See [`super::orchestrator::Dlq`] for usage.

use super::entry::DlqEntry;
use super::error::DlqError;

/// A DLQ backend. One variant per supported destination.
///
/// Variants are feature-gated:
///
/// - [`Self::File`] -- always available
/// - `Kafka` -- `dlq-kafka` feature
/// - `Http` -- `dlq-http` feature
/// - `Redis` -- `dlq-redis` feature
///
/// Each variant's inner struct lives in its sibling module
/// (`file::FileDlqInner`, `kafka::KafkaDlqInner`, etc.). They are
/// crate-private -- consumers configure DLQ via [`super::DlqConfig`] and
/// drive it via [`super::orchestrator::Dlq`].
#[non_exhaustive]
pub enum DlqBackend {
    /// NDJSON file backend with rotation.
    File(super::file::FileDlqInner),

    /// Kafka backend.
    #[cfg(feature = "dlq-kafka")]
    Kafka(super::kafka::KafkaDlqInner),

    /// HTTP POST backend.
    #[cfg(feature = "dlq-http")]
    Http(super::http::HttpDlqInner),

    /// Redis Streams backend.
    #[cfg(feature = "dlq-redis")]
    Redis(super::redis_dlq::RedisDlqInner),
}

impl std::fmt::Debug for DlqBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DlqBackend::")?;
        f.write_str(self.name())
    }
}

impl DlqBackend {
    /// Write a batch of entries to this backend. Called only by the
    /// orchestrator's drain task -- never from a consumer hot path.
    ///
    /// # Errors
    ///
    /// Backend-specific. The orchestrator decides whether to cascade,
    /// fall back, or fan out based on the configured [`super::DlqMode`].
    pub async fn send_batch(&mut self, batch: &[DlqEntry]) -> Result<(), DlqError> {
        match self {
            Self::File(b) => b.send_batch(batch).await,
            #[cfg(feature = "dlq-kafka")]
            Self::Kafka(b) => b.send_batch(batch).await,
            #[cfg(feature = "dlq-http")]
            Self::Http(b) => b.send_batch(batch).await,
            #[cfg(feature = "dlq-redis")]
            Self::Redis(b) => b.send_batch(batch).await,
        }
    }

    /// Make every entry written so far DURABLE.
    ///
    /// Called from the BackgroundSink barrier handler when a consumer
    /// invokes [`super::orchestrator::Dlq::flush`]. Each backend honours
    /// the strongest durability it can express:
    ///
    /// - **File**: `flush()` on the rotating writer. `file-rotate`
    ///   doesn't expose the inner `File`, so we can't `fsync()` -- this
    ///   only flushes to the kernel page cache, so power loss before
    ///   write-back can still lose data. Limitation, tracked until
    ///   `file-rotate` exposes a sync hook.
    /// - **Kafka**: `producer.flush()` -- blocks until every queued
    ///   message is acked by the broker (per the producer's acks
    ///   config). The real durability semantic.
    /// - **HTTP**: no-op. `send_batch` already awaits the response.
    /// - **Redis**: no-op. `send_batch` already awaits the XADD pipeline.
    ///
    /// # Errors
    ///
    /// Backend-specific. Surfaced to the BackgroundSink barrier
    /// handler which logs and continues.
    pub async fn flush_durable(&mut self) -> Result<(), DlqError> {
        match self {
            Self::File(b) => b.flush_durable().await,
            #[cfg(feature = "dlq-kafka")]
            Self::Kafka(b) => b.flush_durable().await,
            #[cfg(feature = "dlq-http")]
            Self::Http(_) => Ok(()),
            #[cfg(feature = "dlq-redis")]
            Self::Redis(_) => Ok(()),
        }
    }

    /// Backend name for log / metric labels.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::File(_) => "file",
            #[cfg(feature = "dlq-kafka")]
            Self::Kafka(_) => "kafka",
            #[cfg(feature = "dlq-http")]
            Self::Http(_) => "http",
            #[cfg(feature = "dlq-redis")]
            Self::Redis(_) => "redis",
        }
    }
}
