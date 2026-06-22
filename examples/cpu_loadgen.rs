// Project:   hyperi-rustlib
// File:      examples/cpu_loadgen.rs
// Purpose:   CPU operational-test harness (NOT a product binary)
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! CPU oversubscription operational-test harness.
//!
//! The unit-under-test for the cgroup-confined CPU operational test (see
//! `scripts/operational-cpu-test.sh` and the plan's "Operational testing"
//! section). Like the memory harness it is a *black box*: drive it under a
//! real cgroup `--cpus` cap and observe the KERNEL outcome (CPU-seconds the
//! process actually burned, and cgroup throttling) -- not rustlib internals.
//!
//! The property under test is the adaptive worker pool's **parked-idle**
//! behaviour: when there is no work, workers block on a `Condvar` and consume
//! ~zero CPU. A burst of CPU work is submitted, then the harness goes IDLE for
//! the rest of the window.
//!
//! - `HARNESS_CAP=on`  (default): work runs on the real [`AdaptiveWorkerPool`].
//!   During the idle window its workers PARK, so total CPU burned is roughly
//!   just the burst; under a `--cpus` cap it is NOT throttled while idle.
//! - `HARNESS_CAP=off`: the control -- a naive hand-rolled pool of
//!   over-subscribed OS threads that BUSY-SPIN on an empty queue instead of
//!   parking. During the "idle" window they peg every core they're allowed,
//!   burning the full cgroup CPU quota and getting throttled. If cap=off did
//!   not burn far more CPU than cap=on, the test would be proving nothing.
//!
//! Env knobs (all optional):
//! - `HARNESS_CAP`           on|off                  (default on)
//! - `HARNESS_BURST_TASKS`   CPU tasks in the burst  (default 256)
//! - `HARNESS_TASK_ITERS`    work iterations / task  (default 2_000_000)
//! - `HARNESS_IDLE_MS`       idle window after burst (default 6000)
//! - `HARNESS_SPIN_THREADS`  control spinner count   (default 8x parallelism)
//!
//! Prints `cpu_loadgen done ... cpu_seconds=<N> throttled_usec=<M>` on stdout;
//! the script asserts on those black-box figures.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use hyperi_rustlib::worker::{AdaptiveWorkerPool, WorkerPoolConfig};
use parking_lot::Mutex;

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// A unit of CPU-bound work the optimiser cannot elide. Returns an accumulator
/// that the caller sums and prints so the loop is not dead-code-eliminated.
#[inline(never)]
fn cpu_work(seed: u64, iters: u64) -> u64 {
    let mut acc: u64 = 0x9e37_79b9_7f4a_7c15 ^ seed.wrapping_mul(0xff51_afd7_ed55_8ccd);
    for i in 0..iters {
        // Cheap, data-dependent mix -- no allocations, pure ALU.
        acc = acc
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(i | 1);
        acc ^= acc >> 29;
    }
    acc
}

/// CPU-seconds this process has consumed (utime + stime) from /proc/self/stat.
///
/// USER_HZ is 100 on every supported Linux target; reading it via `sysconf`
/// needs `unsafe`, which this crate forbids, so it is hard-coded with a note.
#[allow(clippy::cast_precision_loss)] // tick counts are tiny; f64 is exact here
fn process_cpu_seconds() -> f64 {
    const USER_HZ: f64 = 100.0;
    let Ok(stat) = std::fs::read_to_string("/proc/self/stat") else {
        return f64::NAN;
    };
    // comm (field 2) is parenthesised and may contain spaces -- parse after
    // the last ')'. Fields after it: state(3) ... utime(14) stime(15).
    let Some(after) = stat.rsplit_once(')').map(|(_, r)| r.trim()) else {
        return f64::NAN;
    };
    let fields: Vec<&str> = after.split_whitespace().collect();
    // 0-based indexes into `fields`: utime = 14-3 = 11, stime = 15-3 = 12.
    let utime: u64 = fields.get(11).and_then(|s| s.parse().ok()).unwrap_or(0);
    let stime: u64 = fields.get(12).and_then(|s| s.parse().ok()).unwrap_or(0);
    (utime + stime) as f64 / USER_HZ
}

/// cgroup v2 `throttled_usec` -- microseconds the cgroup was CPU-throttled.
/// Best-effort: returns None if cgroup v2 cpu.stat is unavailable.
fn cgroup_throttled_usec() -> Option<u64> {
    let stat = std::fs::read_to_string("/sys/fs/cgroup/cpu.stat").ok()?;
    for line in stat.lines() {
        if let Some(v) = line.strip_prefix("throttled_usec ") {
            return v.trim().parse().ok();
        }
    }
    None
}

fn run_capped(burst_tasks: usize, task_iters: u64, idle: Duration) -> u64 {
    let pool = AdaptiveWorkerPool::new(WorkerPoolConfig::default());
    println!(
        "cpu_loadgen pool target_threads={} max_threads={}",
        pool.target_threads(),
        pool.max_threads(),
    );
    // Burst: run all tasks across the pool, then let it go idle (workers park).
    let items: Vec<u64> = (0..burst_tasks as u64).collect();
    let results = pool.map_owned(items, |i| cpu_work(i, task_iters));
    let checksum: u64 = results.iter().fold(0u64, |a, &r| a ^ r);
    // Idle window -- pool workers are parked on the Condvar, ~0 CPU.
    std::thread::sleep(idle);
    checksum
}

fn run_uncapped(burst_tasks: usize, task_iters: u64, idle: Duration, spin_threads: usize) -> u64 {
    // Naive control: over-subscribed OS threads that BUSY-SPIN on an empty
    // queue instead of parking. `remaining` is the work backlog.
    let remaining = Arc::new(Mutex::new(burst_tasks));
    let stop = Arc::new(AtomicBool::new(false));
    let checksum = Arc::new(Mutex::new(0u64));

    let mut handles = Vec::with_capacity(spin_threads);
    for _ in 0..spin_threads {
        let remaining = Arc::clone(&remaining);
        let stop = Arc::clone(&stop);
        let checksum = Arc::clone(&checksum);
        handles.push(std::thread::spawn(move || {
            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let claimed = {
                    let mut g = remaining.lock();
                    if *g > 0 {
                        *g -= 1;
                        Some(*g as u64)
                    } else {
                        None
                    }
                };
                if let Some(seed) = claimed {
                    let r = cpu_work(seed, task_iters);
                    *checksum.lock() ^= r;
                } else {
                    // The pathology: spin instead of parking. Burns CPU while
                    // there is no work to do -- exactly what the pool avoids.
                    std::hint::spin_loop();
                }
            }
        }));
    }

    // Wait out the same wall window the capped run uses (burst + idle). The
    // spinners keep burning CPU the entire time once the backlog drains.
    std::thread::sleep(idle);
    stop.store(true, Ordering::Relaxed);
    for h in handles {
        let _ = h.join();
    }
    *checksum.lock()
}

fn main() {
    let cap_on = std::env::var("HARNESS_CAP").map_or(true, |v| v != "off");
    let burst_tasks: usize = env_or("HARNESS_BURST_TASKS", 256);
    let task_iters: u64 = env_or("HARNESS_TASK_ITERS", 2_000_000);
    let idle_ms: u64 = env_or("HARNESS_IDLE_MS", 6_000);
    let default_spinners = std::thread::available_parallelism().map_or(32, |n| n.get() * 8);
    let spin_threads: usize = env_or("HARNESS_SPIN_THREADS", default_spinners);
    let idle = Duration::from_millis(idle_ms);

    println!(
        "cpu_loadgen start cap={} burst_tasks={} task_iters={} idle_ms={} spin_threads={}",
        if cap_on { "on" } else { "off" },
        burst_tasks,
        task_iters,
        idle_ms,
        spin_threads,
    );

    let cpu_before = process_cpu_seconds();
    let throttled_before = cgroup_throttled_usec().unwrap_or(0);
    let wall = Instant::now();

    let checksum = if cap_on {
        run_capped(burst_tasks, task_iters, idle)
    } else {
        run_uncapped(burst_tasks, task_iters, idle, spin_threads)
    };

    let cpu_after = process_cpu_seconds();
    let throttled_after = cgroup_throttled_usec().unwrap_or(0);
    let cpu_seconds = cpu_after - cpu_before;
    let throttled_usec = throttled_after.saturating_sub(throttled_before);

    println!(
        "cpu_loadgen done cap={} wall_s={:.2} cpu_seconds={:.2} throttled_usec={} checksum={}",
        if cap_on { "on" } else { "off" },
        wall.elapsed().as_secs_f64(),
        cpu_seconds,
        throttled_usec,
        checksum,
    );
}
