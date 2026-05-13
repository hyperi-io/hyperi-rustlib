// Project:   hyperi-rustlib
// File:      src/concurrency/periodic.rs
// Purpose:   PeriodicWorker — timer-driven loop with biased shutdown
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Timer-driven background loop, generic over the task type.
//!
//! Use for everything that's "do X every N seconds": config reload
//! polling, scaling pressure recompute, periodic health snapshots,
//! version-check pings to crates.io. Encapsulates the
//! `tokio::time::interval` + `tokio::select! { biased; shutdown | tick }`
//! boilerplate that would otherwise be open-coded per call site.
//!
//! # Shape
//!
//! ```text
//! tokio::time::interval ──tick──► actor task ──tick()──► PeriodicTask impl
//!                                     ▲
//!                                     │ biased select
//!                                     │
//!                              CancellationToken
//! ```
//!
//! Tick errors are logged at WARN and do NOT terminate the worker —
//! the next tick still fires. Use the `shutdown` hook for cleanup.

use std::future::Future;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::{MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::error::TickError;

/// A task that runs on a fixed interval.
///
/// Implementations may hold mutable state — `tick` takes `&mut self`.
/// Replies / outputs propagate through the state itself (`AtomicU64`
/// counters, `ArcSwap<T>` config handles, etc.) — `tick` returns
/// `Result<(), TickError>` purely for error signalling.
pub trait PeriodicTask: Send + 'static {
    /// Called once per interval tick.
    fn tick(&mut self) -> impl Future<Output = Result<(), TickError>> + Send;

    /// Called once after the shutdown token fires, before the worker
    /// task exits. Default: no-op.
    fn shutdown(&mut self) -> impl Future<Output = Result<(), TickError>> + Send {
        std::future::ready(Ok(()))
    }
}

/// Handle for the worker task.
///
/// Dropping the handle does NOT abort the task — the task lives until
/// the `CancellationToken` is fired or the runtime shuts down. Use
/// [`Self::join`] for graceful drain after signalling shutdown.
pub struct PeriodicWorker {
    join: JoinHandle<()>,
}

impl PeriodicWorker {
    /// Spawn a periodic worker. The first `tick()` fires after one
    /// `interval_duration` elapses (NOT at t=0). This avoids the
    /// common bug where every spawned worker bursts a tick immediately
    /// on startup, hammering downstream dependencies during deploy.
    pub fn spawn<T: PeriodicTask>(
        mut task: T,
        interval_duration: Duration,
        shutdown: CancellationToken,
    ) -> Self {
        let join = tokio::spawn(async move {
            let mut tick = interval(interval_duration);
            tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
            // Consume the immediate first tick.
            tick.tick().await;

            loop {
                tokio::select! {
                    biased;
                    () = shutdown.cancelled() => {
                        if let Err(e) = task.shutdown().await {
                            warn!(error = %e, "periodic task shutdown hook failed");
                        }
                        return;
                    }
                    _ = tick.tick() => {
                        if let Err(e) = task.tick().await {
                            warn!(error = %e, "periodic task tick failed");
                        }
                    }
                }
            }
        });
        Self { join }
    }

    /// Await the worker's clean exit. Use after `shutdown.cancel()`.
    pub async fn join(self) -> Result<(), tokio::task::JoinError> {
        self.join.await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Instant;

    struct CountingTask {
        ticks: Arc<AtomicU32>,
    }

    impl PeriodicTask for CountingTask {
        async fn tick(&mut self) -> Result<(), TickError> {
            self.ticks.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct ShutdownTask {
        ticks: Arc<AtomicU32>,
        shutdown_called: Arc<AtomicU32>,
    }

    impl PeriodicTask for ShutdownTask {
        async fn tick(&mut self) -> Result<(), TickError> {
            self.ticks.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn shutdown(&mut self) -> Result<(), TickError> {
            self.shutdown_called.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct FailingTask {
        ticks: Arc<AtomicU32>,
    }

    impl PeriodicTask for FailingTask {
        async fn tick(&mut self) -> Result<(), TickError> {
            self.ticks.fetch_add(1, Ordering::SeqCst);
            Err(TickError::Generic("simulated".into()))
        }
    }

    #[tokio::test]
    async fn tick_fires_at_interval() {
        let ticks = Arc::new(AtomicU32::new(0));
        let shutdown = CancellationToken::new();
        let _worker = PeriodicWorker::spawn(
            CountingTask {
                ticks: ticks.clone(),
            },
            Duration::from_millis(20),
            shutdown.clone(),
        );
        // Wait ~110ms; expect roughly 5 ticks at 20ms interval.
        tokio::time::sleep(Duration::from_millis(110)).await;
        shutdown.cancel();
        let n = ticks.load(Ordering::SeqCst);
        assert!((4..=7).contains(&n), "got {n} ticks, expected 4-7");
    }

    #[tokio::test]
    async fn first_tick_is_delayed_not_immediate() {
        // Regression test: ensure the worker does NOT fire a tick at
        // t=0 (the common gotcha with tokio::time::interval).
        let ticks = Arc::new(AtomicU32::new(0));
        let shutdown = CancellationToken::new();
        let _worker = PeriodicWorker::spawn(
            CountingTask {
                ticks: ticks.clone(),
            },
            Duration::from_millis(100),
            shutdown.clone(),
        );
        // Check immediately — should be 0 because interval consumed
        // the first tick before the loop started.
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(ticks.load(Ordering::SeqCst), 0);
        shutdown.cancel();
    }

    #[tokio::test]
    async fn shutdown_hook_called_exactly_once() {
        let ticks = Arc::new(AtomicU32::new(0));
        let shutdown_called = Arc::new(AtomicU32::new(0));
        let shutdown = CancellationToken::new();
        let worker = PeriodicWorker::spawn(
            ShutdownTask {
                ticks: ticks.clone(),
                shutdown_called: shutdown_called.clone(),
            },
            Duration::from_mins(1), // very long, no ticks expected
            shutdown.clone(),
        );
        shutdown.cancel();
        worker.join().await.expect("clean exit");
        assert_eq!(shutdown_called.load(Ordering::SeqCst), 1);
        // No tick should have fired in the brief lifetime.
        assert_eq!(ticks.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn failing_tick_does_not_stop_worker() {
        let ticks = Arc::new(AtomicU32::new(0));
        let shutdown = CancellationToken::new();
        let _worker = PeriodicWorker::spawn(
            FailingTask {
                ticks: ticks.clone(),
            },
            Duration::from_millis(15),
            shutdown.clone(),
        );
        // Wait long enough for several failing ticks.
        tokio::time::sleep(Duration::from_millis(80)).await;
        shutdown.cancel();
        let n = ticks.load(Ordering::SeqCst);
        // Worker kept ticking despite errors — proves no panic + no exit.
        assert!(n >= 3, "got {n} ticks, expected >=3 even with errors");
    }

    #[tokio::test]
    async fn biased_select_prioritises_shutdown_over_tick() {
        // If a tick was due simultaneously with shutdown, the biased
        // select! must pick shutdown first. We can't directly observe
        // ordering, but we can verify shutdown completes cleanly even
        // when triggered at the moment a tick would fire.
        let ticks = Arc::new(AtomicU32::new(0));
        let shutdown = CancellationToken::new();
        let worker = PeriodicWorker::spawn(
            CountingTask {
                ticks: ticks.clone(),
            },
            Duration::from_millis(1), // very tight — many tick opportunities
            shutdown.clone(),
        );
        let t0 = Instant::now();
        // Run for a bit, then cancel.
        tokio::time::sleep(Duration::from_millis(20)).await;
        shutdown.cancel();
        worker.join().await.expect("clean exit");
        let elapsed = t0.elapsed();
        // join() must return in much less than 1s — i.e. shutdown
        // wasn't blocked by an in-flight tick.
        assert!(
            elapsed < Duration::from_millis(500),
            "worker took {elapsed:?} to shut down (expected <500ms)",
        );
    }
}
