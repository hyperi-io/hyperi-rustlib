// Project:   hyperi-rustlib
// File:      src/worker/batch.rs
// Purpose:   BatchProcessor trait and BatchPipeline for parallel-then-sequential processing
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Batch processing framework for DFE pipeline parallelisation.
//!
//! Provides the [`BatchProcessor`] trait for defining parallel-safe message
//! processing, and [`BatchPipeline`] for orchestrating the parallel (rayon) →
//! sequential (state mutation) pipeline.
//!
//! ## The Pattern
//!
//! Every DFE app follows the same structure:
//!
//! 1. **Parallel phase:** Process each message through a pure `&self` function
//!    (parse, route, transform, enrich) — via rayon `process_batch()`
//! 2. **Sequential phase:** Apply results to mutable state (buffer push,
//!    mark_pending, stats update, DLQ routing)
//!
//! The [`BatchProcessor`] trait captures phase 1. Phase 2 is app-specific
//! (each app has different buffers, caches, and sinks).
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::worker::{BatchPipeline, BatchProcessor};
//!
//! struct MyProcessor<'a> { router: &'a Router, ... }
//!
//! impl BatchProcessor for MyProcessor<'_> {
//!     type Input = KafkaMessage;
//!     type Output = ProcessedMessage;
//!     type Error = MyError;
//!
//!     fn process(&self, msg: &KafkaMessage) -> Result<ProcessedMessage, MyError> {
//!         let parsed = sonic_rs::from_slice(&msg.payload)?;
//!         let table = self.router.route(&parsed)?;
//!         Ok(ProcessedMessage { table, data: parsed })
//!     }
//! }
//!
//! // In event loop:
//! let processor = MyProcessor { router: &router, ... };
//! let results = pipeline.process_batch(&processor, &batch);
//! drop(processor); // release immutable borrows
//! // Sequential phase: apply results to mutable state
//! ```

use std::sync::Arc;

use super::pool::AdaptiveWorkerPool;
use super::stats::PipelineStats;

/// Trait for parallel-safe message processing.
///
/// Implement this with a struct that holds only `&` references to immutable
/// dependencies. The `process` method must be pure — no mutable state, no I/O,
/// no `.await`. Safe for rayon `par_iter()`.
///
/// The struct is typically created per-batch in the event loop (borrows released
/// before the sequential phase begins). The borrow checker enforces this.
pub trait BatchProcessor: Sync {
    /// Input message type (e.g. `KafkaMessage`, `HttpRequest`).
    type Input: Sync;

    /// Successful processing result (e.g. `ProcessedMessage`, `CompressedBatch`).
    type Output: Send;

    /// Error type for processing failures.
    type Error: Send;

    /// Process a single input. Must be pure — no mutation, no I/O.
    fn process(&self, input: &Self::Input) -> Result<Self::Output, Self::Error>;
}

/// Orchestrates parallel batch processing via [`AdaptiveWorkerPool`].
///
/// Wraps the worker pool with common DFE pipeline concerns: stats tracking,
/// memory accounting, and metrics emission. Apps provide a [`BatchProcessor`]
/// implementation; the pipeline handles the rest.
pub struct BatchPipeline {
    pool: Arc<AdaptiveWorkerPool>,
    stats: Arc<PipelineStats>,
}

impl BatchPipeline {
    /// Create a new batch pipeline.
    #[must_use]
    pub fn new(pool: Arc<AdaptiveWorkerPool>, stats: Arc<PipelineStats>) -> Self {
        Self { pool, stats }
    }

    /// Process a batch in parallel via rayon.
    ///
    /// Tracks `received` stats automatically. Returns results in input order.
    /// The caller handles the sequential phase (buffer push, DLQ, etc.).
    pub fn process_batch<P: BatchProcessor>(
        &self,
        processor: &P,
        batch: &[P::Input],
    ) -> Vec<Result<P::Output, P::Error>> {
        self.stats.add_received(batch.len() as u64);
        self.pool
            .process_batch(batch, |input| processor.process(input))
    }

    /// Access the underlying worker pool (for `fan_out_async`, scaling, etc.).
    #[must_use]
    pub fn pool(&self) -> &Arc<AdaptiveWorkerPool> {
        &self.pool
    }

    /// Access pipeline stats.
    #[must_use]
    pub fn stats(&self) -> &Arc<PipelineStats> {
        &self.stats
    }
}
