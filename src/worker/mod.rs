// Project:   hyperi-rustlib
// File:      src/worker/mod.rs
// Purpose:   Adaptive worker pool with hybrid rayon + tokio execution
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Adaptive worker pool and batch processing framework.
//!
//! Two layers:
//!
//! - **Generic:** [`AdaptiveWorkerPool`] provides CPU-saturating parallelism via
//!   rayon (CPU-bound) + tokio (async I/O), with reactive pressure-based scaling.
//!   Useful for any workload — not DFE-specific.
//!
//! - **Opinionated:** [`BatchProcessor`] trait + [`BatchPipeline`] provide a
//!   structured parallel-then-sequential pipeline for DFE apps. Apps implement
//!   `BatchProcessor` for their domain; the pipeline handles stats, scaling,
//!   and batch orchestration. [`PipelineStats`] provides common atomic counters.
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use hyperi_rustlib::worker::{AdaptiveWorkerPool, WorkerPoolConfig};
//!
//! let pool = AdaptiveWorkerPool::from_cascade("worker_pool")?;
//! pool.register_metrics(&metrics_manager);
//! pool.start_scaling_loop(shutdown_token.clone());
//!
//! // CPU-bound parallel transform (rayon)
//! let results = pool.process_batch(&messages, |msg| {
//!     parse_and_transform(msg)
//! });
//!
//! // Async parallel enrichment (tokio)
//! let enriched = pool.fan_out_async(&items, |item| async move {
//!     enrich(item).await
//! }).await;
//! ```

mod accumulator;
mod batch;
mod config;
pub(crate) mod metrics;
pub mod ndjson;
mod pool;
pub(crate) mod scaler;
mod stats;

pub use accumulator::{AccumulatorConfig, AccumulatorFull, BatchAccumulator, BatchDrainer};
pub use batch::{BatchPipeline, BatchProcessor};
pub use config::WorkerPoolConfig;
pub use pool::AdaptiveWorkerPool;
pub use scaler::{ScalingDecision, ScalingInput};
pub use stats::{PipelineStats, PipelineStatsSnapshot};
