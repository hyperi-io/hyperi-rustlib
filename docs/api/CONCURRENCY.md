# Concurrency

Three async primitives the rest of the crate is built on:
`BackgroundSink`, `PeriodicWorker`, `ActorHandle`. None of them are
novel -- they're the conventional answers to fire-and-forget durable
writes, timer-driven loops, and command-queue actors. Centralising
them here keeps the implementations consistent across subsystems and
keeps modules from rolling their own variants.

If you're writing a new long-lived module, pick from this menu before
reaching for `tokio::spawn` + a custom channel.

---

## Decision matrix

| Workload shape | Use |
|----------------|-----|
| Stateless function | Just call the function. No primitive needed. |
| Per-request short-lived work | `tokio::spawn` + `JoinSet` |
| Pure CPU batch | `rayon::par_iter` |
| Read-heavy shared state | `Arc<RwLock<T>>` |
| Fire-and-forget durable writes | [`BackgroundSink`](#backgroundsink) |
| Timer-driven loop | [`PeriodicWorker`](#periodicworker) |
| Long-lived state behind a command queue | [`ActorHandle`](#actorhandle) |
| Pipeline stage with backpressure | `tokio::mpsc::channel(N)` |

The three primitives below cover the cases the rest of the rows leave
to "build it yourself". Use them.

---

## `BackgroundSink`

Fire-and-forget durable write. Caller `try_push`es items; an actor task
drains them through a `SinkDrain` impl that writes durably (disk file,
DLQ queue, Kafka topic). Caller returns immediately -- the actor does
the I/O.

Used by [DLQ](../pipeline/DLQ.md) for queued DLQ entries; by any
subsystem that needs durable side-channel writes without blocking the
hot path.

```rust
use hyperi_rustlib::concurrency::{BackgroundSink, BackgroundSinkConfig, SinkDrain, Overflow};

struct NdjsonDrain { path: PathBuf }

impl SinkDrain<Event> for NdjsonDrain {
    async fn drain(&mut self, batch: Vec<Event>) -> Result<(), anyhow::Error> {
        // write batch to disk
    }
}

// spawn returns a cloneable sink plus a single-owner join handle.
let (sink, handle) = BackgroundSink::spawn(
    NdjsonDrain { path: "/var/log/events.ndjson".into() },
    BackgroundSinkConfig {
        queue_capacity: 10_000,
        overflow: Overflow::Drop,
        batch_size: 256,
        flush_interval: Duration::from_millis(500),
        ..Default::default()
    },
    shutdown_token.clone(),
);

// Hot path -- non-blocking:
sink.try_push(event)?;

// Periodic stats:
let dropped = sink.dropped();
let pending = sink.pending();

// On shutdown:
sink.flush().await?;
handle.join().await?;
```

Overflow modes: `Drop` (the default -- `try_push` returns
`Err(Overflow)` immediately and bumps the dropped counter; pick this
for telemetry) and `Block` (`try_push` is rejected unconditionally,
forcing callers to use `push_blocking`/`flush`; pick this when the
sink must never silently drop, e.g. billing).

`push_blocking()` waits for capacity (use sparingly; the point of this
primitive is `try_push` returning immediately). In `Block` mode it is
the only push path -- `try_push` always returns `Err(Overflow)`.

---

## `PeriodicWorker`

Timer-driven loop. Implements a `PeriodicTask` trait; the worker drives
the schedule with `MissedTickBehavior::Delay` (if the task takes longer
than the interval, the next fire is delayed, not piled up).

Used by the [scaling pressure refresher](../pipeline/SCALING.md), by
[secrets rotation polling](SECRETS.md#caching), by any subsystem with a
"check every N seconds" requirement.

```rust
use hyperi_rustlib::concurrency::{PeriodicTask, PeriodicWorker};

struct RefreshScaling;

impl PeriodicTask for RefreshScaling {
    async fn tick(&mut self) -> Result<(), anyhow::Error> {
        ScalingPressure::current().refresh().await;
        Ok(())
    }
}

let worker = PeriodicWorker::spawn(
    RefreshScaling,
    Duration::from_secs(5),
    shutdown_token.clone(),
);

// On shutdown:
worker.join().await?;
```

The shutdown token argument is mandatory -- periodic workers always
take the global cancellation token so they drain cleanly on SIGTERM.

---

## `ActorHandle`

Command-queue actor. Owns mutable state; readers send commands through
an `mpsc` channel; the actor task processes them serially. Use when
state needs serialised access AND lives for the process lifetime.

Used by [BatchEngine](../pipeline/BATCH-ENGINE.md), by [config
hot-reload](../core-pillars/CONFIG.md#hot-reload), by anywhere
"one writer, many readers, long-lived" applies and a lock won't do
(e.g. multi-step command sequences, side-effects between commands).

```rust
use hyperi_rustlib::concurrency::{Actor, ActorConfig, ActorHandle};

enum Cmd {
    AddRule(Rule),
    RemoveRule(RuleId),
    Snapshot(oneshot::Sender<RuleSet>),
}

struct RulesActor { state: RuleSet }

impl Actor for RulesActor {
    type Command = Cmd;
    async fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::AddRule(r) => self.state.insert(r),
            Cmd::RemoveRule(id) => self.state.remove(&id),
            Cmd::Snapshot(reply) => { let _ = reply.send(self.state.clone()); }
        }
    }
}

// spawn returns a cloneable handle plus a single-owner join handle.
let (handle, join) = ActorHandle::<Cmd>::spawn(
    RulesActor { state: RuleSet::new() },
    ActorConfig { queue_capacity: 256, ..Default::default() },
    shutdown_token.clone(),
);

handle.send(Cmd::AddRule(rule)).await?;
let (tx, rx) = oneshot::channel();
handle.send(Cmd::Snapshot(tx)).await?;
let snapshot = rx.await?;

// On shutdown:
join.join().await?;
```

`try_send` for non-blocking pushes; `send` for back-pressure. The
`oneshot` reply pattern is how you do request-response.

---

## API surface

| Item | Purpose |
|------|---------|
| `BackgroundSink::spawn(drain, config, shutdown)` | Spawn the actor task; returns `(BackgroundSink, BackgroundSinkHandle)` |
| `.try_push(msg)` | Hot-path push, returns immediately |
| `.push_blocking(msg)` | Await capacity |
| `.flush()` | Wait for the queue to drain |
| `.dropped() -> u64` | Count of dropped messages (overflow) |
| `.pending() -> usize` | Current queue depth |
| `BackgroundSinkHandle::join()` | Await the actor task exit (single-owner) |
| `PeriodicTask` trait | Implement `tick()` |
| `PeriodicWorker::spawn(task, interval, shutdown)` | Drive the schedule; returns `PeriodicWorker` |
| `PeriodicWorker::join()` | Await worker exit |
| `Actor` trait | Implement `handle(&mut self, Command)` |
| `ActorHandle::spawn(actor, config, shutdown)` | Spawn an actor; returns `(ActorHandle, ActorJoinHandle)` |
| `.send(cmd)` | Await capacity, deliver command |
| `.try_send(cmd)` | Non-blocking variant |
| `ActorJoinHandle::join()` | Await the actor task exit (single-owner) |

---

## Related

- [../pipeline/DLQ.md](../pipeline/DLQ.md) -- `BackgroundSink` consumer
- [../pipeline/SCALING.md](../pipeline/SCALING.md) -- `PeriodicWorker` consumer
- [../pipeline/BATCH-ENGINE.md](../pipeline/BATCH-ENGINE.md) -- `ActorHandle` consumer
- [../core-pillars/SHUTDOWN.md](../core-pillars/SHUTDOWN.md) -- `CancellationToken` discipline
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) -- `concurrency`
- Source: [../../src/concurrency/](../../src/concurrency/)
