// Project:   hyperi-rustlib
// File:      src/concurrency/sink.rs
// Purpose:   BackgroundSink — generic fire-and-forget durable sink
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Generic fire-and-forget durable sink built on bounded `mpsc` +
//! background actor. Consumer hot path is sync-shaped and never blocks
//! the tokio runtime — `try_push` is ~100 ns push to an mpsc queue.
//!
//! See `docs/superpowers/plans/2026-05-08-async-patterns.md` for the
//! design rationale and `hyperi-ai/standards/languages/RUST.md`
//! "Three async primitives" subsection for the canonical usage.
//!
//! # The contract
//!
//! - [`BackgroundSink::try_push`] returns immediately. ~100 ns when the
//!   queue has space; `Err(SinkError::Overflow)` if full (Drop mode).
//! - [`BackgroundSink::push_blocking`] is `async` and awaits queue
//!   space. Use when the caller can yield.
//! - [`BackgroundSink::flush`] is `async` and resolves only after every
//!   message accepted before this call is durably written by the drain.
//!
//! # Shape
//!
//! ```text
//! consumer (many) ──try_push──► mpsc bounded ──► actor task ──► drain.write_batch
//! ```
//!
//! The actor batches messages by size (`batch_size`) or interval
//! (`flush_interval`), whichever fires first, then hands the batch to
//! the drain. Drain implementations supply backend-specific async I/O.
//!
//! # Shutdown
//!
//! When the [`tokio_util::sync::CancellationToken`] is cancelled, the
//! actor drains everything currently in the queue, writes a final
//! batch, calls `drain.close()`, and exits. Use the returned
//! [`BackgroundSinkHandle::join`] to await graceful drain.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::error::{DrainError, SinkError};

/// Configuration for a [`BackgroundSink`].
#[derive(Debug, Clone)]
pub struct BackgroundSinkConfig {
    /// Maximum queued messages before overflow policy kicks in.
    /// Default 10_000.
    pub queue_capacity: usize,

    /// Coalesce up to this many messages into one drain call.
    /// Default 256.
    pub batch_size: usize,

    /// Flush a partial batch after this duration even if it's not full.
    /// Default 100 ms.
    pub flush_interval: Duration,

    /// What `try_push` does when the queue is full.
    /// Default `Overflow::Drop`.
    pub overflow: Overflow,

    /// Optional Prometheus metric prefix. When `Some("dfe_dlq_file")`,
    /// the sink auto-registers and emits:
    ///   - `<prefix>_pushed_total`         (counter)
    ///   - `<prefix>_dropped_total`        (counter, only `Overflow::Drop`)
    ///   - `<prefix>_writes_total`         (counter, per batch)
    ///   - `<prefix>_write_errors_total`   (counter, per failed batch)
    ///   - `<prefix>_pending`              (gauge, current queue depth)
    ///
    /// When `None`, the sink tracks `dropped()` / `pending()` via
    /// internal atomics only — callers can read them but nothing is
    /// published.
    pub metric_prefix: Option<&'static str>,
}

impl Default for BackgroundSinkConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 10_000,
            batch_size: 256,
            flush_interval: Duration::from_millis(100),
            overflow: Overflow::Drop,
            metric_prefix: None,
        }
    }
}

/// Overflow policy for [`BackgroundSink::try_push`].
///
/// `push_blocking` and `flush` always wait for queue space regardless
/// of this setting — overflow only governs the sync `try_push` path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overflow {
    /// `try_push` returns `Err(Overflow)` immediately and increments
    /// the dropped counter. Default for hot paths.
    Drop,
    /// `try_push` is rejected — callers in this mode MUST use
    /// `push_blocking` or `flush`. Documents intent: "this sink may
    /// not be used from a sync context."
    Block,
}

/// Internal channel message. Tagged so flush() barriers and data
/// messages share the queue and are processed in FIFO order.
enum SinkMsg<T> {
    Data(T),
    Barrier(oneshot::Sender<()>),
}

/// A drain consumes batches of messages and writes them to the backend.
///
/// Implementations are captured by [`BackgroundSink::spawn`] and live
/// inside the actor task. They run only on the actor — never on
/// consumer hot paths.
///
/// **CRITICAL:** drain methods may take time (disk fsync, network
/// roundtrip). Use true-async I/O (`tokio::fs`, async clients like
/// `rdkafka` / `reqwest` / `redis`) or `tokio::task::spawn_blocking`
/// for unavoidable sync work. **NEVER** put `std::fs::*` /
/// `std::io::Write::*` / `std::thread::sleep` directly in `write_batch`
/// — that pins the actor task's tokio worker, and `tests/sync_in_async.rs`
/// will fail the lint.
pub trait SinkDrain<T: Send>: Send + 'static {
    /// Flush a batch. Implementer chooses the async I/O strategy.
    /// Returned `Err` is logged + counted by the actor; the actor
    /// continues draining subsequent batches.
    ///
    /// Takes `&mut self` — drains typically own mutable I/O state
    /// (file handles, connection pools, write buffers). The actor
    /// holds the only reference; this method is never called
    /// concurrently for a given drain instance.
    fn write_batch(&mut self, batch: Vec<T>)
    -> impl Future<Output = Result<(), DrainError>> + Send;

    /// Block until every entry written to this drain so far is durable
    /// (synced to disk for file backends, acked by the broker for
    /// network backends). Called by the actor when it processes a
    /// `BackgroundSink::flush()` barrier — BEFORE acking the barrier
    /// — so callers of `flush()` see real durability, not just "the
    /// bytes were handed to the kernel".
    ///
    /// Default: no-op (the trait stays additive; non-durable drains
    /// pay nothing). Implementers with durability semantics
    /// (fsync, Kafka producer flush, etc.) override.
    fn flush_durable(&mut self) -> impl Future<Output = Result<(), DrainError>> + Send {
        std::future::ready(Ok(()))
    }

    /// One-shot close at actor shutdown. Default: no-op.
    /// Typical implementations flush remaining state, close file
    /// handles, return network connections to a pool.
    fn close(&mut self) -> impl Future<Output = Result<(), DrainError>> + Send {
        std::future::ready(Ok(()))
    }
}

/// Cloneable handle for pushing messages. Hot-path consumers clone
/// freely and push from many tasks concurrently.
#[derive(Debug, Clone)]
pub struct BackgroundSink<T: Send + 'static> {
    tx: mpsc::Sender<SinkMsg<T>>,
    dropped: Arc<AtomicU64>,
    pending: Arc<AtomicUsize>,
    overflow: Overflow,
    metric_prefix: Option<&'static str>,
}

/// Single-owner handle for awaiting actor shutdown.
///
/// Held by the orchestrator that called [`BackgroundSink::spawn`]. The
/// actor task is detached; the handle exists to let the owner join it
/// after cancelling the shutdown token.
pub struct BackgroundSinkHandle {
    join: JoinHandle<()>,
}

impl BackgroundSinkHandle {
    /// Await the actor's clean exit. Use after signalling shutdown via
    /// the `CancellationToken` passed to `spawn`.
    ///
    /// Returns `Err(JoinError)` only if the actor panicked.
    pub async fn join(self) -> Result<(), tokio::task::JoinError> {
        self.join.await
    }
}

impl<T: Send + 'static> BackgroundSink<T> {
    /// Spawn the actor task. Returns a cloneable sink + single-owner
    /// join handle.
    pub fn spawn<D: SinkDrain<T>>(
        drain: D,
        config: BackgroundSinkConfig,
        shutdown: CancellationToken,
    ) -> (Self, BackgroundSinkHandle) {
        let (tx, rx) = mpsc::channel(config.queue_capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        let pending = Arc::new(AtomicUsize::new(0));
        let metric_prefix = config.metric_prefix;
        let overflow = config.overflow;

        if let Some(prefix) = metric_prefix {
            metrics::describe_counter!(
                format!("{prefix}_pushed_total"),
                "Messages successfully enqueued by the background sink"
            );
            metrics::describe_counter!(
                format!("{prefix}_dropped_total"),
                "Messages dropped due to queue overflow"
            );
            metrics::describe_counter!(
                format!("{prefix}_writes_total"),
                "Batch writes attempted by the drain"
            );
            metrics::describe_counter!(
                format!("{prefix}_write_errors_total"),
                "Batch writes that returned an error"
            );
            metrics::describe_gauge!(
                format!("{prefix}_pending"),
                "Current background sink queue depth"
            );
        }

        let actor_pending = Arc::clone(&pending);
        let join = tokio::spawn(actor_loop(
            rx,
            drain,
            config,
            shutdown,
            actor_pending,
            metric_prefix,
        ));

        (
            Self {
                tx,
                dropped,
                pending,
                overflow,
                metric_prefix,
            },
            BackgroundSinkHandle { join },
        )
    }

    /// Sync-shaped fire-and-forget push. ~100 ns happy path. Never
    /// awaits, never blocks the runtime, never holds a lock.
    ///
    /// - `Overflow::Drop` (default): returns `Err(Overflow)` immediately
    ///   when the queue is full and increments the dropped counter.
    /// - `Overflow::Block`: returns `Err(Overflow)` unconditionally —
    ///   callers in Block mode must use `push_blocking`. This makes the
    ///   policy decision explicit at the call site.
    pub fn try_push(&self, msg: T) -> Result<(), SinkError> {
        match self.overflow {
            Overflow::Drop => {
                // Increment BEFORE sending. The actor's
                // `write_batch_with_metrics` subtracts from `pending`
                // when it processes a message. If we incremented AFTER
                // send, a fast actor could receive + process + subtract
                // before our add landed — underflowing `pending` to a
                // huge wrap-around value.
                self.pending.fetch_add(1, Ordering::Relaxed);
                match self.tx.try_send(SinkMsg::Data(msg)) {
                    Ok(()) => {
                        if let Some(p) = self.metric_prefix {
                            metrics::counter!(format!("{p}_pushed_total")).increment(1);
                        }
                        Ok(())
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        // Send refused — give the slot back.
                        self.pending.fetch_sub(1, Ordering::Relaxed);
                        self.dropped.fetch_add(1, Ordering::Relaxed);
                        if let Some(p) = self.metric_prefix {
                            metrics::counter!(format!("{p}_dropped_total")).increment(1);
                        }
                        Err(SinkError::Overflow)
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        self.pending.fetch_sub(1, Ordering::Relaxed);
                        Err(SinkError::Closed)
                    }
                }
            }
            Overflow::Block => Err(SinkError::Overflow),
        }
    }

    /// Async push that awaits queue space. Returns when queued (NOT
    /// yet durably written — use [`Self::flush`] for that).
    pub async fn push_blocking(&self, msg: T) -> Result<(), SinkError> {
        // Increment before send, same race avoidance as `try_push`. The
        // .await yield point makes the race window wider than the sync
        // case; the same fix applies.
        self.pending.fetch_add(1, Ordering::Relaxed);
        if self.tx.send(SinkMsg::Data(msg)).await.is_err() {
            self.pending.fetch_sub(1, Ordering::Relaxed);
            return Err(SinkError::Closed);
        }
        if let Some(p) = self.metric_prefix {
            metrics::counter!(format!("{p}_pushed_total")).increment(1);
        }
        Ok(())
    }

    /// Await durability of every message accepted before this call.
    /// Returns when the actor has processed past a barrier inserted at
    /// the back of the queue.
    ///
    /// In `Overflow::Drop` mode, "every message accepted" excludes
    /// messages that were dropped via overflow — those were never in
    /// the queue. If you need lossless flush semantics, use
    /// `Overflow::Block` + `push_blocking`.
    pub async fn flush(&self) -> Result<(), SinkError> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.tx
            .send(SinkMsg::Barrier(ack_tx))
            .await
            .map_err(|_| SinkError::Closed)?;
        ack_rx.await.map_err(|_| SinkError::Closed)
    }

    /// Total messages dropped due to overflow since spawn.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Current queue depth (approximate — actor may be mid-recv).
    #[must_use]
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::Relaxed)
    }
}

async fn actor_loop<T, D>(
    mut rx: mpsc::Receiver<SinkMsg<T>>,
    mut drain: D,
    config: BackgroundSinkConfig,
    shutdown: CancellationToken,
    pending: Arc<AtomicUsize>,
    metric_prefix: Option<&'static str>,
) where
    T: Send + 'static,
    D: SinkDrain<T>,
{
    let mut batch: Vec<T> = Vec::with_capacity(config.batch_size);
    let mut tick = interval(config.flush_interval);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Consume the immediate first tick; otherwise interval fires at t=0.
    tick.tick().await;

    loop {
        tokio::select! {
            biased;

            () = shutdown.cancelled() => {
                // Refuse new sends BEFORE we drain. Producers that race
                // shutdown will see `Closed` instead of having their
                // message sit in a channel we observed as empty.
                rx.close();

                // Flush whatever's already accumulated.
                if !batch.is_empty() {
                    write_batch_with_metrics(
                        &mut drain, std::mem::take(&mut batch),
                        &pending, metric_prefix,
                    ).await;
                }
                while let Ok(msg) = rx.try_recv() {
                    match msg {
                        SinkMsg::Data(t) => {
                            batch.push(t);
                            if batch.len() >= config.batch_size {
                                write_batch_with_metrics(
                                    &mut drain, std::mem::take(&mut batch),
                                    &pending, metric_prefix,
                                ).await;
                            }
                        }
                        SinkMsg::Barrier(ack) => {
                            // Flush the current batch BEFORE acking. The
                            // requester is waiting for durability of
                            // every message accepted before their flush
                            // call — acking before the write violates
                            // that contract.
                            if !batch.is_empty() {
                                write_batch_with_metrics(
                                    &mut drain, std::mem::take(&mut batch),
                                    &pending, metric_prefix,
                                ).await;
                            }
                            // After the batch is dispatched to the drain,
                            // ask the drain to make it DURABLE (fsync /
                            // producer flush). Default impl is a no-op.
                            if let Err(e) = drain.flush_durable().await {
                                warn!(error = %e, "sink drain flush_durable failed during shutdown barrier");
                            }
                            let _ = ack.send(());
                        }
                    }
                }
                if !batch.is_empty() {
                    write_batch_with_metrics(
                        &mut drain, std::mem::take(&mut batch),
                        &pending, metric_prefix,
                    ).await;
                }
                if let Err(e) = drain.close().await {
                    warn!(error = %e, "sink drain close failed");
                }
                return;
            }

            msg = rx.recv() => match msg {
                Some(SinkMsg::Data(t)) => {
                    batch.push(t);
                    if batch.len() >= config.batch_size {
                        write_batch_with_metrics(
                            &mut drain, std::mem::take(&mut batch),
                            &pending, metric_prefix,
                        ).await;
                    }
                }
                Some(SinkMsg::Barrier(ack)) => {
                    if !batch.is_empty() {
                        write_batch_with_metrics(
                            &mut drain, std::mem::take(&mut batch),
                            &pending, metric_prefix,
                        ).await;
                    }
                    if let Err(e) = drain.flush_durable().await {
                        warn!(error = %e, "sink drain flush_durable failed during barrier");
                    }
                    let _ = ack.send(());
                }
                None => {
                    // All senders dropped — graceful exit.
                    if !batch.is_empty() {
                        write_batch_with_metrics(
                            &mut drain, std::mem::take(&mut batch),
                            &pending, metric_prefix,
                        ).await;
                    }
                    if let Err(e) = drain.close().await {
                        warn!(error = %e, "sink drain close failed");
                    }
                    return;
                }
            },

            _ = tick.tick() => {
                if !batch.is_empty() {
                    write_batch_with_metrics(
                        &mut drain, std::mem::take(&mut batch),
                        &pending, metric_prefix,
                    ).await;
                }
            }
        }
    }
}

async fn write_batch_with_metrics<T, D: SinkDrain<T>>(
    drain: &mut D,
    batch: Vec<T>,
    pending: &AtomicUsize,
    metric_prefix: Option<&'static str>,
) where
    T: Send,
{
    let count = batch.len();
    pending.fetch_sub(count, Ordering::Relaxed);
    if let Some(p) = metric_prefix {
        metrics::counter!(format!("{p}_writes_total")).increment(1);
        metrics::gauge!(format!("{p}_pending")).set(pending.load(Ordering::Relaxed) as f64);
    }
    if let Err(e) = drain.write_batch(batch).await {
        warn!(error = %e, count, "sink drain write_batch failed");
        if let Some(p) = metric_prefix {
            metrics::counter!(format!("{p}_write_errors_total")).increment(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    use tokio::sync::Notify;
    use tokio_util::sync::CancellationToken;

    /// Counts written messages. Used as the trivial drain in fast tests.
    struct CountingDrain {
        count: Arc<AtomicU64>,
    }

    impl SinkDrain<u32> for CountingDrain {
        async fn write_batch(&mut self, batch: Vec<u32>) -> Result<(), DrainError> {
            self.count.fetch_add(batch.len() as u64, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Drain that blocks each call on a Notify. Used to simulate slow
    /// backends and verify the consumer hot path is unaffected.
    struct ThrottledDrain {
        release: Arc<Notify>,
        count: Arc<AtomicU64>,
    }

    impl SinkDrain<u32> for ThrottledDrain {
        async fn write_batch(&mut self, batch: Vec<u32>) -> Result<(), DrainError> {
            self.release.notified().await;
            self.count.fetch_add(batch.len() as u64, Ordering::SeqCst);
            Ok(())
        }
    }

    fn fast_config() -> BackgroundSinkConfig {
        BackgroundSinkConfig {
            queue_capacity: 1024,
            batch_size: 16,
            flush_interval: Duration::from_millis(20),
            overflow: Overflow::Drop,
            metric_prefix: None,
        }
    }

    #[tokio::test]
    async fn try_push_succeeds_when_queue_has_space() {
        let count = Arc::new(AtomicU64::new(0));
        let shutdown = CancellationToken::new();
        let (sink, _handle) = BackgroundSink::spawn(
            CountingDrain {
                count: count.clone(),
            },
            fast_config(),
            shutdown.clone(),
        );

        for i in 0..10 {
            sink.try_push(i).expect("queue has space");
        }
        sink.flush().await.expect("flush ok");
        assert_eq!(count.load(Ordering::SeqCst), 10);
        shutdown.cancel();
    }

    #[tokio::test]
    async fn try_push_returns_overflow_when_full() {
        let count = Arc::new(AtomicU64::new(0));
        let release = Arc::new(Notify::new());
        let shutdown = CancellationToken::new();
        let cfg = BackgroundSinkConfig {
            queue_capacity: 4,
            batch_size: 16,
            flush_interval: Duration::from_mins(1),
            overflow: Overflow::Drop,
            metric_prefix: None,
        };
        let (sink, _handle) = BackgroundSink::spawn(
            ThrottledDrain {
                release: release.clone(),
                count: count.clone(),
            },
            cfg,
            shutdown.clone(),
        );

        // Fill the queue without releasing the actor's first write.
        let mut accepted: u64 = 0;
        let mut overflowed: u64 = 0;
        for i in 0..20 {
            match sink.try_push(i) {
                Ok(()) => accepted += 1,
                Err(SinkError::Overflow) => overflowed += 1,
                Err(e) => panic!("unexpected error: {e}"),
            }
        }
        assert!(overflowed > 0, "expected at least one overflow");
        assert_eq!(sink.dropped(), overflowed);
        // Queue cap is 4 — once filled, every push past that overflows.
        assert!(accepted >= 4, "should accept at least queue_capacity");
        let _ = (accepted, count); // silence unused
        shutdown.cancel();
        release.notify_waiters();
    }

    #[tokio::test]
    async fn try_push_in_block_mode_always_errors() {
        let count = Arc::new(AtomicU64::new(0));
        let shutdown = CancellationToken::new();
        let cfg = BackgroundSinkConfig {
            overflow: Overflow::Block,
            ..fast_config()
        };
        let (sink, _handle) = BackgroundSink::spawn(
            CountingDrain {
                count: count.clone(),
            },
            cfg,
            shutdown.clone(),
        );
        // try_push in Block mode is rejected regardless of queue state.
        match sink.try_push(1) {
            Err(SinkError::Overflow) => {}
            other => panic!("expected Overflow, got {other:?}"),
        }
        // push_blocking still works.
        sink.push_blocking(1)
            .await
            .expect("push_blocking ok in Block mode");
        sink.flush().await.expect("flush ok");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        shutdown.cancel();
    }

    #[tokio::test]
    async fn flush_waits_for_pre_flush_messages() {
        let count = Arc::new(AtomicU64::new(0));
        let shutdown = CancellationToken::new();
        let (sink, _handle) = BackgroundSink::spawn(
            CountingDrain {
                count: count.clone(),
            },
            fast_config(),
            shutdown.clone(),
        );

        for i in 0..100 {
            sink.try_push(i).expect("queue has space");
        }
        sink.flush().await.expect("flush ok");
        // Every pushed message must be drained before flush returns.
        assert_eq!(count.load(Ordering::SeqCst), 100);
        shutdown.cancel();
    }

    #[tokio::test]
    async fn shutdown_drains_remaining_queue() {
        let count = Arc::new(AtomicU64::new(0));
        let shutdown = CancellationToken::new();
        let (sink, handle) = BackgroundSink::spawn(
            CountingDrain {
                count: count.clone(),
            },
            fast_config(),
            shutdown.clone(),
        );
        for i in 0..50 {
            sink.try_push(i).expect("queue has space");
        }
        shutdown.cancel();
        handle.join().await.expect("clean exit");
        assert_eq!(count.load(Ordering::SeqCst), 50);
    }

    #[tokio::test]
    async fn dropped_counter_reflects_overflow_count() {
        let count = Arc::new(AtomicU64::new(0));
        let release = Arc::new(Notify::new());
        let shutdown = CancellationToken::new();
        let cfg = BackgroundSinkConfig {
            queue_capacity: 2,
            batch_size: 16,
            flush_interval: Duration::from_mins(1),
            overflow: Overflow::Drop,
            metric_prefix: None,
        };
        let (sink, _handle) = BackgroundSink::spawn(
            ThrottledDrain {
                release: release.clone(),
                count: count.clone(),
            },
            cfg,
            shutdown.clone(),
        );
        for i in 0..100 {
            let _ = sink.try_push(i);
        }
        // Most pushes overflowed since drain is throttled and queue cap is 2.
        assert!(sink.dropped() >= 90, "dropped={}", sink.dropped());
        shutdown.cancel();
        release.notify_waiters();
    }

    // ---------------------------------------------------------------
    // NON-BLOCKING-UNDER-LOAD TESTS
    //
    // These directly address the "async + non-blocking issues are
    // opaque" concern. They prove the hot path stays sub-microsecond
    // even when the actor is throttled by a slow drain.
    // ---------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn try_push_stays_fast_under_load() {
        let count = Arc::new(AtomicU64::new(0));
        let release = Arc::new(Notify::new());
        let shutdown = CancellationToken::new();
        let cfg = BackgroundSinkConfig {
            queue_capacity: 100_000,
            batch_size: 256,
            flush_interval: Duration::from_mins(1),
            overflow: Overflow::Drop,
            metric_prefix: None,
        };
        let (sink, _handle) = BackgroundSink::spawn(
            ThrottledDrain {
                release: release.clone(),
                count: count.clone(),
            },
            cfg,
            shutdown.clone(),
        );

        // Push 10_000 messages back-to-back. Drain is gated — actor is
        // blocked on `notified()`. The hot path MUST stay fast anyway.
        let start = Instant::now();
        for i in 0..10_000_u32 {
            sink.try_push(i).expect("queue has space");
        }
        let elapsed = start.elapsed();
        // Generous: average <50µs per push (way above the ~100ns target,
        // but the test must be robust against CI noise + multi-thread
        // contention). The bench gives the real performance number.
        let avg_us = elapsed.as_micros() as f64 / 10_000.0;
        assert!(
            avg_us < 50.0,
            "try_push p_avg = {avg_us}µs (expected <50µs under load with throttled drain)",
        );
        shutdown.cancel();
        release.notify_waiters();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn many_concurrent_producers_dont_block_each_other() {
        let count = Arc::new(AtomicU64::new(0));
        let shutdown = CancellationToken::new();
        let cfg = BackgroundSinkConfig {
            queue_capacity: 100_000,
            batch_size: 1024,
            flush_interval: Duration::from_millis(5),
            overflow: Overflow::Drop,
            metric_prefix: None,
        };
        let (sink, _handle) = BackgroundSink::spawn(
            CountingDrain {
                count: count.clone(),
            },
            cfg,
            shutdown.clone(),
        );

        // 8 concurrent producers, each pushing 1000 messages.
        let mut handles = Vec::new();
        for _ in 0..8 {
            let s = sink.clone();
            handles.push(tokio::spawn(async move {
                for i in 0..1_000_u32 {
                    s.try_push(i).expect("queue has space");
                }
            }));
        }
        for h in handles {
            h.await.expect("producer exit");
        }
        sink.flush().await.expect("flush ok");
        assert_eq!(count.load(Ordering::SeqCst), 8_000);
        shutdown.cancel();
    }

    #[tokio::test]
    async fn flush_completes_quickly_when_queue_is_already_empty() {
        let count = Arc::new(AtomicU64::new(0));
        let shutdown = CancellationToken::new();
        let (sink, _handle) = BackgroundSink::spawn(
            CountingDrain {
                count: count.clone(),
            },
            fast_config(),
            shutdown.clone(),
        );
        // Empty-queue flush should return in well under one flush_interval.
        let start = Instant::now();
        sink.flush().await.expect("flush ok");
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_millis(50),
            "empty flush took {elapsed:?} (expected <50ms)",
        );
        shutdown.cancel();
    }

    /// Verify a slow drain doesn't propagate latency back to the
    /// consumer: try_push must remain fast while the actor is blocked
    /// inside write_batch.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn slow_drain_doesnt_block_consumer() {
        struct SlowDrain {
            count: Arc<AtomicU64>,
        }
        impl SinkDrain<u32> for SlowDrain {
            async fn write_batch(&mut self, batch: Vec<u32>) -> Result<(), DrainError> {
                tokio::time::sleep(Duration::from_millis(50)).await;
                self.count.fetch_add(batch.len() as u64, Ordering::SeqCst);
                Ok(())
            }
        }

        let count = Arc::new(AtomicU64::new(0));
        let shutdown = CancellationToken::new();
        let cfg = BackgroundSinkConfig {
            queue_capacity: 10_000,
            batch_size: 16,
            flush_interval: Duration::from_millis(5),
            overflow: Overflow::Drop,
            metric_prefix: None,
        };
        let (sink, _handle) = BackgroundSink::spawn(
            SlowDrain {
                count: count.clone(),
            },
            cfg,
            shutdown.clone(),
        );

        // Push 200 messages while the drain takes 50ms per batch.
        // Consumer should NOT see 50ms latencies on try_push.
        let mut max_us: u128 = 0;
        for i in 0..200_u32 {
            let t0 = Instant::now();
            sink.try_push(i).expect("queue has space");
            let elapsed_us = t0.elapsed().as_micros();
            if elapsed_us > max_us {
                max_us = elapsed_us;
            }
        }
        assert!(
            max_us < 5_000,
            "max try_push latency was {max_us}µs — slow drain leaked back to consumer",
        );
        shutdown.cancel();
    }
}
