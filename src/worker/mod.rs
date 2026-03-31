// Project:   hyperi-rustlib
// File:      src/worker/mod.rs
// Purpose:   Adaptive worker pool with hybrid rayon + tokio execution
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Internal vertical scaling module for DFE pipeline applications.
//!
//! Provides CPU-saturating parallelism via a hybrid rayon (CPU-bound) + tokio
//! (async I/O) worker pool. Reactively scales thread count up and down based
//! on CPU utilisation and memory pressure, bounded by configurable watermark
//! thresholds. All thresholds are config-cascade overridable and observable
//! as gauge metrics for Grafana overlay.
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

mod config;
pub(crate) mod metrics;
mod pool;
pub(crate) mod scaler;

pub use config::WorkerPoolConfig;
pub use pool::AdaptiveWorkerPool;
pub use scaler::{ScalingDecision, ScalingInput};
