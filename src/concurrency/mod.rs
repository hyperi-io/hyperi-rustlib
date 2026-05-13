// Project:   hyperi-rustlib
// File:      src/concurrency/mod.rs
// Purpose:   Three generic async primitives for HyperI Rust libraries
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Async concurrency primitives.
//!
//! Three patterns, each generic over the consumer's domain type, each
//! the canonical way to do the corresponding shape of async work in
//! HyperI Rust libraries. See
//! `hyperi-ai/standards/languages/RUST.md` §"Three async primitives
//! every HyperI library uses" for design philosophy and a decision
//! matrix.
//!
//! # Decision matrix
//!
//! | Shape | Use |
//! |---|---|
//! | Consumer pushes; background batches + writes to a backend | [`BackgroundSink`] + [`SinkDrain`] |
//! | Timer-driven loop, no inbound queue | [`PeriodicWorker`] + [`PeriodicTask`] |
//! | Mutable state, command queue, optional oneshot replies | [`ActorHandle`] + [`Actor`] |
//!
//! # The hard rule
//!
//! **An `async fn` in HyperI libraries MUST yield to the runtime.**
//! No hidden synchronous I/O. If you need sync I/O from async:
//!
//! - Use `tokio::fs` / `tokio::io::AsyncWrite` for occasional async I/O.
//! - Use [`BackgroundSink`] for high-throughput durable writes.
//! - Use `tokio::task::spawn_blocking` for one-off unavoidable sync work.
//! - Don't — push the sync work to startup, before the runtime is hot.
//!
//! The grep-based `tests/sync_in_async.rs` lint enforces this
//! mechanically: any `async fn` body containing `std::fs::*`,
//! `std::io::Write::write_*`, `std::thread::sleep`, `reqwest::blocking::*`,
//! or `parking_lot::*::lock()` held across `.await` fails CI.
//!
//! # Considered alternatives
//!
//! - `tokio-actors` (v0.6) — covers `ActorHandle` + `PeriodicWorker`
//!   cleanly but doesn't provide batched-drain-with-flush-barrier,
//!   which is the load-bearing case for [`BackgroundSink`]. Wrapping
//!   it would equal the size of this module's impl. Rejected.
//! - `tokio-prometheus-metered-channel` (v0.2) — bounded channel with
//!   Prometheus metrics, no actor / batch / flush. Too narrow.
//! - `xtra`, `actix`, `bastion` — heavy actor frameworks. Don't fit
//!   the sink shape. Rejected.
//! - `tracing-appender::non_blocking` — used elsewhere for the logger
//!   subscriber; orthogonal to this module's tokio-runtime concerns.
//!
//! Rationale per [Alice Ryhl's "Actors with Tokio"](https://ryhl.io/blog/actors-with-tokio/)
//! and the [2026-05-08 audit](https://github.com/hyperi-io/hyperi-rustlib).

mod actor;
mod error;
mod periodic;
mod sink;

pub use actor::{Actor, ActorConfig, ActorHandle, ActorJoinHandle};
pub use error::{ActorError, DrainError, SinkError, TickError};
pub use periodic::{PeriodicTask, PeriodicWorker};
pub use sink::{BackgroundSink, BackgroundSinkConfig, BackgroundSinkHandle, Overflow, SinkDrain};
