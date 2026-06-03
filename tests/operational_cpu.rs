// Project:   hyperi-rustlib
// File:      tests/operational_cpu.rs
// Purpose:   Black-box, cgroup-confined CPU oversubscription operational test
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Operational CPU test (CRITICAL, see the plan's "Operational testing"
//! section). Not a code-block test -- it drives the real harness
//! ([`examples/cpu_loadgen.rs`]) under a genuine cgroup `--cpus` cap via docker
//! and asserts the KERNEL outcome with a with/without control:
//!
//! - cap=on  -> parks idle workers, burns ~only the burst's CPU
//! - cap=off -> busy-spin control burns >= 2x the CPU and is cgroup-throttled
//!
//! `#[ignore]` by default: it needs docker and builds an image (~minutes), so
//! it runs in a dedicated/nightly CI job, not the per-push gate. Run with:
//!
//! ```sh
//! cargo nextest run --features worker --run-ignored all -E 'test(operational_cpu)'
//! # or directly:
//! scripts/operational-cpu-test.sh 0.5 6000
//! ```
#![cfg(feature = "worker")]

use std::process::Command;

fn docker_available() -> bool {
    Command::new("docker")
        .arg("version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
#[ignore = "operational: needs docker + builds an image; runs in the dedicated CI job"]
fn operational_cpu_pool_parks_idle_with_control() {
    if !docker_available() {
        eprintln!("SKIP: docker unavailable -- operational CPU test not run");
        return;
    }

    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/scripts/operational-cpu-test.sh"
    );
    let status = Command::new("bash")
        .arg(script)
        .arg("0.5") // cgroup --cpus cap
        .arg("6000") // idle window (ms)
        .status()
        .expect("failed to spawn operational-cpu-test.sh");

    // The script exits 2 to signal its own SKIP (docker vanished mid-run).
    if status.code() == Some(2) {
        eprintln!("SKIP: harness reported docker unavailable");
        return;
    }

    assert!(
        status.success(),
        "operational CPU test failed (see script output above): the pool did \
         not park idle relative to the busy-spin control, or the control was \
         not cgroup-throttled"
    );
}
