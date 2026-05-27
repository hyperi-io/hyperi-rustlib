// Project:   hyperi-rustlib
// File:      src/concurrency/actor.rs
// Purpose:   ActorHandle -- stateful command-queue actor
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Stateful command-queue actor.
//!
//! Generic helper for the canonical "spawn a task that owns mutable
//! state and processes commands from a channel" pattern documented in
//! `hyperi-ai/standards/languages/RUST.md` "Long-Lived Background
//! Actors". Used when a single task should serialise mutations to
//! shared state -- DLQ orchestrator routing, state machines, etc.
//!
//! # Shape
//!
//! ```text
//! consumer ──send/try_send──► mpsc bounded ──► actor task ──handle()──► Actor (state)
//!                                                  ▲
//!                                                  │ biased select
//!                                                  │
//!                                          CancellationToken + idle ticker
//! ```
//!
//! Replies are conveyed via `oneshot::Sender<Reply>` fields embedded
//! in the `Command` variant -- see the canonical recipe in RUST.md.

use std::future::Future;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;

use super::error::ActorError;

/// Configuration for an [`ActorHandle`].
#[derive(Debug, Clone)]
pub struct ActorConfig {
    /// Maximum queued commands before `try_send` returns Full.
    /// Default 1024.
    pub queue_capacity: usize,

    /// `on_idle` is called every `idle_interval` when no commands are
    /// pending. Set to `Duration::MAX` (or any very large value) to
    /// effectively disable idle ticks. Default 1 minute.
    pub idle_interval: Duration,
}

impl Default for ActorConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 1024,
            idle_interval: Duration::from_mins(1),
        }
    }
}

/// An actor owns mutable state and processes commands sequentially.
///
/// Implementations pick a `Command` type (typically an enum with
/// embedded `oneshot::Sender<Reply>` fields for request-response).
pub trait Actor: Send + 'static {
    /// Command type received from the channel.
    type Command: Send + 'static;

    /// Process one command. Called in receive order.
    fn handle(&mut self, cmd: Self::Command) -> impl Future<Output = ()> + Send;

    /// Called every `idle_interval` when no commands are pending.
    /// Default: no-op. Useful for periodic state maintenance
    /// (cleanup, metrics emit, etc.) without a separate timer task.
    fn on_idle(&mut self) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }

    /// Called once after shutdown is signalled (or all senders
    /// dropped) and the in-flight command finishes.
    fn on_shutdown(&mut self) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }
}

/// Cloneable handle for sending commands.
///
/// Clone freely across tasks -- `mpsc::Sender` clone is cheap.
#[derive(Debug, Clone)]
pub struct ActorHandle<Cmd: Send + 'static> {
    tx: mpsc::Sender<Cmd>,
}

/// Single-owner handle for awaiting actor shutdown.
pub struct ActorJoinHandle {
    join: JoinHandle<()>,
}

impl<Cmd: Send + 'static> ActorHandle<Cmd> {
    /// Spawn the actor task. Returns a cloneable command-sender +
    /// single-owner join handle.
    pub fn spawn<A: Actor<Command = Cmd>>(
        actor: A,
        config: ActorConfig,
        shutdown: CancellationToken,
    ) -> (Self, ActorJoinHandle) {
        let (tx, rx) = mpsc::channel(config.queue_capacity);
        let join = tokio::spawn(actor_loop(actor, rx, config, shutdown));
        (Self { tx }, ActorJoinHandle { join })
    }

    /// Send a command. Awaits queue space if full.
    pub async fn send(&self, cmd: Cmd) -> Result<(), ActorError> {
        self.tx.send(cmd).await.map_err(|_| ActorError::Closed)
    }

    /// Try to send a command. Returns immediately.
    ///
    /// `Err(Full)` if queue is full (caller decides whether to drop,
    /// retry, or escalate). `Err(Closed)` if the actor has exited.
    pub fn try_send(&self, cmd: Cmd) -> Result<(), ActorError> {
        self.tx.try_send(cmd).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => ActorError::Full,
            mpsc::error::TrySendError::Closed(_) => ActorError::Closed,
        })
    }
}

impl ActorJoinHandle {
    /// Await the actor's clean exit.
    pub async fn join(self) -> Result<(), tokio::task::JoinError> {
        self.join.await
    }
}

async fn actor_loop<A: Actor>(
    mut actor: A,
    mut rx: mpsc::Receiver<A::Command>,
    config: ActorConfig,
    shutdown: CancellationToken,
) {
    let mut idle = interval(config.idle_interval);
    idle.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Consume the immediate first tick.
    idle.tick().await;

    loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => {
                actor.on_shutdown().await;
                return;
            }
            cmd = rx.recv() => if let Some(c) = cmd {
                actor.handle(c).await;
            } else {
                // All senders dropped -- graceful exit.
                actor.on_shutdown().await;
                return;
            },
            _ = idle.tick() => {
                actor.on_idle().await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use tokio::sync::oneshot;

    enum Cmd {
        Increment,
        Read(oneshot::Sender<u32>),
    }

    struct Counter {
        value: u32,
    }

    impl Actor for Counter {
        type Command = Cmd;

        async fn handle(&mut self, cmd: Cmd) {
            match cmd {
                Cmd::Increment => self.value += 1,
                Cmd::Read(reply) => {
                    let _ = reply.send(self.value);
                }
            }
        }
    }

    #[tokio::test]
    async fn actor_handles_commands_in_order() {
        let shutdown = CancellationToken::new();
        let (handle, _join) = ActorHandle::spawn(
            Counter { value: 0 },
            ActorConfig::default(),
            shutdown.clone(),
        );
        for _ in 0..10 {
            handle.send(Cmd::Increment).await.expect("send ok");
        }
        let (tx, rx) = oneshot::channel();
        handle.send(Cmd::Read(tx)).await.expect("send ok");
        assert_eq!(rx.await.expect("reply"), 10);
        shutdown.cancel();
    }

    #[tokio::test]
    async fn try_send_returns_full_when_saturated() {
        struct SlowCounter {
            value: u32,
            release: Arc<tokio::sync::Notify>,
        }
        impl Actor for SlowCounter {
            type Command = u32;
            async fn handle(&mut self, _cmd: u32) {
                self.release.notified().await;
                self.value += 1;
            }
        }
        let release = Arc::new(tokio::sync::Notify::new());
        let shutdown = CancellationToken::new();
        let cfg = ActorConfig {
            queue_capacity: 4,
            idle_interval: Duration::from_mins(1),
        };
        let (handle, _join) = ActorHandle::spawn(
            SlowCounter {
                value: 0,
                release: release.clone(),
            },
            cfg,
            shutdown.clone(),
        );
        // Saturate: actor is blocked on notified(), queue fills to 4,
        // 5th try_send must hit Full.
        let mut full_count = 0;
        for i in 0..20 {
            match handle.try_send(i) {
                Ok(()) => {}
                Err(ActorError::Full) => full_count += 1,
                Err(e) => panic!("unexpected: {e}"),
            }
        }
        assert!(full_count >= 10, "got {full_count} Full errors");
        shutdown.cancel();
        release.notify_waiters();
    }

    #[tokio::test]
    async fn on_shutdown_called_once() {
        struct ShutdownObserver {
            called: Arc<AtomicU32>,
        }
        impl Actor for ShutdownObserver {
            type Command = ();
            async fn handle(&mut self, _cmd: ()) {}
            async fn on_shutdown(&mut self) {
                self.called.fetch_add(1, Ordering::SeqCst);
            }
        }
        let called = Arc::new(AtomicU32::new(0));
        let shutdown = CancellationToken::new();
        let (_handle, join) = ActorHandle::spawn(
            ShutdownObserver {
                called: called.clone(),
            },
            ActorConfig::default(),
            shutdown.clone(),
        );
        shutdown.cancel();
        join.join().await.expect("clean exit");
        assert_eq!(called.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dropping_all_handles_exits_gracefully() {
        struct ShutdownObserver {
            called: Arc<AtomicU32>,
        }
        impl Actor for ShutdownObserver {
            type Command = ();
            async fn handle(&mut self, _cmd: ()) {}
            async fn on_shutdown(&mut self) {
                self.called.fetch_add(1, Ordering::SeqCst);
            }
        }
        let called = Arc::new(AtomicU32::new(0));
        let shutdown = CancellationToken::new();
        let (handle, join) = ActorHandle::spawn(
            ShutdownObserver {
                called: called.clone(),
            },
            ActorConfig::default(),
            shutdown.clone(),
        );
        // Drop the only handle -- actor should see Closed and exit.
        drop(handle);
        join.join().await.expect("clean exit");
        assert_eq!(called.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn idle_tick_fires_when_no_commands() {
        struct IdleCounter {
            ticks: Arc<AtomicU32>,
        }
        impl Actor for IdleCounter {
            type Command = ();
            async fn handle(&mut self, _cmd: ()) {}
            async fn on_idle(&mut self) {
                self.ticks.fetch_add(1, Ordering::SeqCst);
            }
        }
        let ticks = Arc::new(AtomicU32::new(0));
        let shutdown = CancellationToken::new();
        let cfg = ActorConfig {
            queue_capacity: 16,
            idle_interval: Duration::from_millis(20),
        };
        let (_handle, _join) = ActorHandle::spawn(
            IdleCounter {
                ticks: ticks.clone(),
            },
            cfg,
            shutdown.clone(),
        );
        tokio::time::sleep(Duration::from_millis(110)).await;
        shutdown.cancel();
        let n = ticks.load(Ordering::SeqCst);
        assert!((4..=7).contains(&n), "got {n} idle ticks, expected 4-7");
    }
}
