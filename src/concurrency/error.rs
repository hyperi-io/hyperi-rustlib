// Project:   hyperi-rustlib
// File:      src/concurrency/error.rs
// Purpose:   Error types for the three async concurrency primitives
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Error types for [`crate::concurrency`] primitives.
//!
//! Three concrete error enums, one per primitive:
//! [`SinkError`] for `BackgroundSink`, [`TickError`] for `PeriodicWorker`,
//! [`ActorError`] for `ActorHandle`. Plus [`DrainError`] for `SinkDrain`
//! implementations to report backend failures back to the actor.

use std::error::Error as StdError;

use thiserror::Error;

/// Errors returned by [`super::BackgroundSink`] push / flush operations.
#[derive(Debug, Error)]
pub enum SinkError {
    /// Queue full and `Overflow::Drop` was selected -- message discarded
    /// and the sink's `dropped` counter incremented.
    #[error("background sink queue full (overflow=drop policy)")]
    Overflow,

    /// The background actor has exited (shutdown cancelled or all
    /// senders dropped). No further pushes will succeed.
    #[error("background sink actor has exited")]
    Closed,

    /// A drain implementation reported an error during a batch write.
    /// The actor logs + counts these and continues; this variant is
    /// only surfaced if the caller asks the sink for a propagated error
    /// (rare -- drain failures are usually observed via the
    /// `<prefix>_write_errors_total` metric).
    #[error("drain failure: {0}")]
    Drain(#[from] DrainError),
}

/// Errors returned by [`super::SinkDrain`] implementations.
///
/// Drain failures are logged + counted but do NOT terminate the actor.
/// The actor continues draining subsequent batches. If a drain
/// consistently fails, operators see rising `<prefix>_write_errors_total`
/// and the `<prefix>_pending` gauge climbing toward the queue cap.
#[derive(Debug, Error)]
pub enum DrainError {
    /// Standard library I/O error (file writes, syscalls, etc.).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Backend-specific error wrapped as `Box<dyn Error>`. Backends
    /// (Kafka, Redis, HTTP) convert their native error types into this
    /// via `From` impls in their own modules.
    #[error("backend: {0}")]
    Backend(Box<dyn StdError + Send + Sync>),
}

/// Errors returned by [`super::PeriodicTask`] tick implementations.
///
/// Tick errors are logged at WARN and do NOT terminate the worker --
/// the next tick still fires. Consumers wanting fail-fast must return
/// `Ok(())` from `tick` and surface their failure differently (e.g.
/// via a `failed: AtomicBool` flag the parent process polls).
#[derive(Debug, Error)]
pub enum TickError {
    /// Generic error wrapper. Most tick implementations only have one
    /// failure mode and convert their native error type into this.
    #[error("{0}")]
    Generic(Box<dyn StdError + Send + Sync>),
}

/// Errors returned by [`super::ActorHandle`] send operations.
#[derive(Debug, Error)]
pub enum ActorError {
    /// `try_send` saw a full command queue. Caller chooses whether to
    /// drop, retry, or escalate.
    #[error("actor command queue full")]
    Full,

    /// The actor task has exited. No further commands accepted.
    #[error("actor has exited")]
    Closed,
}
