// Project:   hyperi-rustlib
// File:      tests/smoke.rs
// Purpose:   Startup smoke test — catches init panics before production
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Startup smoke test.
//!
//! Highest-value single test: catches init panics, missing defaults,
//! broken dependency wiring. Runs on every push.

#[test]
fn smoke_env_detection() {
    let env = hyperi_rustlib::env::Environment::detect();
    assert!(
        !format!("{env:?}").is_empty(),
        "environment detection must return a valid variant"
    );
}

#[test]
fn smoke_runtime_paths() {
    let paths = hyperi_rustlib::runtime::RuntimePaths::discover();
    assert!(
        !paths.data_dir.as_os_str().is_empty(),
        "runtime data_dir must be non-empty"
    );
}
