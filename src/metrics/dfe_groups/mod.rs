// Project:   hyperi-rustlib
// File:      src/metrics/dfe_groups/mod.rs
// Purpose:   DFE-specific metric groups
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Composable DFE metric groups.
//!
//! Opt-in metric structs for DFE pipeline applications. Each group registers
//! standardised metrics using the [`MetricsManager`](super::MetricsManager)
//! namespace prefix (e.g. `dfe_loader_buffer_bytes`).
//!
//! Feature-gated behind `metrics-dfe`. Non-DFE apps are unaffected.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use hyperi_rustlib::metrics::MetricsManager;
//! use hyperi_rustlib::metrics::dfe_groups::*;
//!
//! let mgr = MetricsManager::new("dfe_loader");
//! let app = AppMetrics::new(&mgr, env!("CARGO_PKG_VERSION"), "abc123");
//! let buffer = BufferMetrics::new(&mgr);
//! let consumer = ConsumerMetrics::new(&mgr);
//! let sink = SinkMetrics::new(&mgr);
//! let cb = CircuitBreakerMetrics::new(&mgr);
//! let bp = BackpressureMetrics::new(&mgr);
//!
//! app.record_received(100);
//! buffer.record_flush(0.042, "size");
//! consumer.set_lag("events", 3, 1500);
//! sink.record_duration("clickhouse", 0.015);
//! cb.record_transition("db.events", "open");
//! bp.record_event();
//! ```

mod app;
mod backpressure;
mod buffer;
mod circuit_breaker;
mod consumer;
mod enrichment;
mod schema_cache;
mod sink;

pub use app::AppMetrics;
pub use backpressure::BackpressureMetrics;
pub use buffer::BufferMetrics;
pub use circuit_breaker::CircuitBreakerMetrics;
pub use consumer::ConsumerMetrics;
pub use enrichment::EnrichmentMetrics;
pub use schema_cache::SchemaCacheMetrics;
pub use sink::SinkMetrics;
