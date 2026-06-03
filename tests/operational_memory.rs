// Project:   hyperi-rustlib
// File:      tests/operational_memory.rs
// Purpose:   Black-box, cgroup-confined memory backpressure operational test
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Operational memory test (CRITICAL, see the plan's "Operational testing"
//! section). This is NOT a code-block test -- it drives the real harness
//! ([`examples/mem_loadgen.rs`]) under a genuine cgroup `--memory` limit via
//! docker and asserts the KERNEL outcome with a with/without control:
//!
//! - cap=on  -> survives and backpressures (rejected > 0)
//! - cap=off -> OOM-killed (the control proving the limit is real)
//!
//! `#[ignore]` by default: it needs docker and builds an image (~minutes), so
//! it runs in a dedicated/nightly CI job, not the per-push gate. Run with:
//!
//! ```sh
//! cargo nextest run --features memory --run-ignored all -E 'test(operational)'
//! # or directly:
//! scripts/operational-mem-test.sh 512m 15
//! ```
#![cfg(feature = "memory")]

use std::process::Command;

fn docker_available() -> bool {
    Command::new("docker")
        .arg("version")
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
#[ignore = "operational: needs docker + builds an image; runs in the dedicated CI job"]
fn operational_memory_cap_bounds_under_cgroup_with_control() {
    if !docker_available() {
        eprintln!("SKIP: docker unavailable -- operational memory test not run");
        return;
    }

    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/scripts/operational-mem-test.sh"
    );
    let status = Command::new("bash")
        .arg(script)
        .arg("512m") // cgroup limit
        .arg("15") // cap=on run length (secs)
        .status()
        .expect("failed to spawn operational-mem-test.sh");

    // The script exits 2 to signal its own SKIP (docker vanished mid-run).
    if status.code() == Some(2) {
        eprintln!("SKIP: harness reported docker unavailable");
        return;
    }

    assert!(
        status.success(),
        "operational memory test failed (see script output above): \
         the cap did not bound memory under the cgroup limit, or the cap-off \
         control did not OOM"
    );
}
