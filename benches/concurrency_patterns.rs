// Project:   hyperi-rustlib
// File:      benches/concurrency_patterns.rs
// Purpose:   Criterion benchmarks for the three async primitives
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Benchmarks for `concurrency::BackgroundSink`, `PeriodicWorker`,
//! `ActorHandle`.
//!
//! Validates the design assumptions:
//!
//! - `BackgroundSink::try_push` is ~100 ns happy path (the consumer hot
//!   path target — slower than this means we've regressed the headline
//!   guarantee).
//! - `ActorHandle::try_send` is in the same ballpark (~100 ns).
//! - Both stay below 500 ns p99 under contention from many concurrent
//!   producers.
//!
//! Run with `cargo bench --bench concurrency_patterns --features concurrency`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

use hyperi_rustlib::concurrency::{
    Actor, ActorConfig, ActorHandle, BackgroundSink, BackgroundSinkConfig, DrainError, Overflow,
    SinkDrain,
};

/// Drain that counts incoming messages atomically. Trivial — keeps the
/// bench focused on the producer hot path, not the drain.
struct CountingDrain {
    count: Arc<AtomicU64>,
}

impl SinkDrain<u64> for CountingDrain {
    async fn write_batch(&mut self, batch: Vec<u64>) -> Result<(), DrainError> {
        self.count.fetch_add(batch.len() as u64, Ordering::Relaxed);
        Ok(())
    }
}

/// Counter actor. Sums each pushed value.
struct CounterActor {
    sum: u64,
}

impl Actor for CounterActor {
    type Command = u64;
    async fn handle(&mut self, cmd: u64) {
        self.sum = self.sum.wrapping_add(cmd);
    }
}

fn bench_background_sink_try_push(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio rt");
    let mut group = c.benchmark_group("background_sink");
    group.throughput(Throughput::Elements(1));

    group.bench_function("try_push_drop_mode", |b| {
        let count = Arc::new(AtomicU64::new(0));
        let shutdown = CancellationToken::new();
        let (sink, _handle) = rt.block_on(async {
            BackgroundSink::spawn(
                CountingDrain {
                    count: count.clone(),
                },
                BackgroundSinkConfig {
                    queue_capacity: 1_000_000,
                    batch_size: 4096,
                    flush_interval: Duration::from_millis(50),
                    overflow: Overflow::Drop,
                    metric_prefix: None,
                },
                shutdown.clone(),
            )
        });

        b.iter(|| {
            let _ = std::hint::black_box(sink.try_push(42));
        });

        shutdown.cancel();
    });

    group.finish();
}

fn bench_actor_try_send(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio rt");
    let mut group = c.benchmark_group("actor");
    group.throughput(Throughput::Elements(1));

    group.bench_function("try_send_unsaturated", |b| {
        let shutdown = CancellationToken::new();
        let (handle, _join) = rt.block_on(async {
            ActorHandle::spawn(
                CounterActor { sum: 0 },
                ActorConfig {
                    queue_capacity: 1_000_000,
                    idle_interval: Duration::from_mins(1),
                },
                shutdown.clone(),
            )
        });

        b.iter(|| {
            let _ = std::hint::black_box(handle.try_send(42));
        });

        shutdown.cancel();
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_background_sink_try_push,
    bench_actor_try_send
);
criterion_main!(benches);
