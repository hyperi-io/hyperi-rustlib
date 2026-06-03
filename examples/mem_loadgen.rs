// Project:   hyperi-rustlib
// File:      examples/mem_loadgen.rs
// Purpose:   Memory operational-test harness (NOT a product binary)
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Memory backpressure operational-test harness.
//!
//! This is the unit-under-test for the cgroup-confined memory operational
//! test (see `scripts/operational-mem-test.sh` and the plan's "Operational
//! testing" section). It is deliberately a *black box*: drive it under a real
//! cgroup `--memory` limit and observe the KERNEL outcome (OOM-killed vs
//! survives) and its stdout backpressure counters -- not rustlib's internals.
//!
//! It wires the real [`MemoryGuard`] and runs an OVER-SUBSCRIBED producer:
//! payloads arrive far faster than a deliberately slow sink drains them, so
//! held memory climbs toward the cgroup limit.
//!
//! - `HARNESS_CAP=on`  (default): each payload must pass `guard.try_reserve`;
//!   over-limit arrivals are REJECTED (backpressure) and dropped, so held
//!   memory plateaus below the limit and the process survives.
//! - `HARNESS_CAP=off`: no reservation check -- held memory grows unbounded
//!   and the kernel OOM-kills the process under a cgroup cap. This is the
//!   control: it proves the limit is real and the load genuinely over-subscribes.
//!
//! Env knobs (all optional):
//! - `HARNESS_CAP`            on|off            (default on)
//! - `HARNESS_PAYLOAD_BYTES`  bytes per payload (default 65536)
//! - `HARNESS_RATE_HZ`        accept attempts/s (default 20000)
//! - `HARNESS_HOLD_MS`        how long a payload is held before release (default 3000)
//! - `HARNESS_DURATION_SECS`  run length        (default 20)
//!
//! Exit 0 on clean completion. A non-zero/137 exit (SIGKILL) under a cgroup
//! cap is the OOM signal the operational test asserts on.

use std::time::{Duration, Instant};

use hyperi_rustlib::memory::{MemoryGuard, MemoryGuardConfig};

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let cap_on = std::env::var("HARNESS_CAP").map_or(true, |v| v != "off");
    let payload_bytes: usize = env_or("HARNESS_PAYLOAD_BYTES", 65_536);
    let rate_hz: u64 = env_or("HARNESS_RATE_HZ", 20_000);
    let hold_ms: u64 = env_or("HARNESS_HOLD_MS", 3_000);
    let duration_secs: u64 = env_or("HARNESS_DURATION_SECS", 20);

    let guard = MemoryGuard::new(MemoryGuardConfig::default());
    println!(
        "mem_loadgen start cap={} limit_bytes={} payload={} rate_hz={} hold_ms={} duration_s={}",
        if cap_on { "on" } else { "off" },
        guard.limit_bytes(),
        payload_bytes,
        rate_hz,
        hold_ms,
        duration_secs,
    );

    // Held payloads, each tagged with the instant it was accepted so the slow
    // sink can release them after `hold_ms`.
    let mut held: Vec<(Instant, Vec<u8>)> = Vec::new();
    let mut accepted: u64 = 0;
    let mut rejected: u64 = 0;

    let start = Instant::now();
    let deadline = start + Duration::from_secs(duration_secs);
    // Inter-arrival sleep target; we batch sleeps to keep overhead low.
    #[allow(clippy::cast_precision_loss)] // rate_hz is small; exactness irrelevant
    let per_op = Duration::from_secs_f64(1.0 / rate_hz as f64);
    let mut last_report = start;

    while Instant::now() < deadline {
        // Drain: release payloads older than hold_ms (the slow sink).
        let now = Instant::now();
        let hold_window = Duration::from_millis(hold_ms);
        let mut i = 0;
        while i < held.len() {
            if now.duration_since(held[i].0) >= hold_window {
                let (_, buf) = held.swap_remove(i);
                if cap_on {
                    guard.release(buf.len() as u64);
                }
                // buf dropped here -> memory freed
            } else {
                i += 1;
            }
        }

        // Arrival: allocate + touch a payload so the pages are resident.
        let mut buf = vec![1u8; payload_bytes];
        buf[payload_bytes.saturating_sub(1)] = 1;

        if cap_on {
            if guard.try_reserve(payload_bytes as u64) {
                held.push((now, buf));
                accepted += 1;
            } else {
                // Backpressure: drop the payload, do not hold it.
                rejected += 1;
                drop(buf);
            }
        } else {
            // No cap: always hold -> unbounded growth -> OOM under a cgroup cap.
            held.push((now, buf));
            accepted += 1;
        }

        if now.duration_since(last_report) >= Duration::from_secs(1) {
            let held_bytes: usize = held.iter().map(|(_, b)| b.len()).sum();
            println!(
                "t={}s accepted={} rejected={} held_count={} held_bytes={} tracked_bytes={} under_pressure={}",
                now.duration_since(start).as_secs(),
                accepted,
                rejected,
                held.len(),
                held_bytes,
                guard.current_bytes(),
                guard.under_pressure(),
            );
            last_report = now;
        }

        if per_op > Duration::ZERO {
            std::thread::sleep(per_op);
        }
    }

    let held_bytes: usize = held.iter().map(|(_, b)| b.len()).sum();
    println!(
        "mem_loadgen done accepted={accepted} rejected={rejected} final_held_bytes={held_bytes} tracked_bytes={}",
        guard.current_bytes()
    );
}
