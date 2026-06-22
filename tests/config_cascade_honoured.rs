// Project:   hyperi-rustlib
// File:      tests/config_cascade_honoured.rs
// Purpose:   Prove the config cascade is ACTUALLY applied to from_cascade subsystems
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Class-catching test for the 2.8.11 cascade-applies fix.
//!
//! The bug: `run_app` built the `ServiceRuntime` (governor / worker pool /
//! batch engine / scaling) from `*::from_cascade()` WITHOUT ever populating the
//! global `CONFIG` singleton, so every subsystem silently took its hard-coded
//! defaults regardless of the app's config file. This test pins the fix: once
//! the cascade is set up from an explicit config file, the platform sections in
//! that file are HONOURED by every `from_cascade()` reader.
//!
//! The `CONFIG` singleton is a process-global `OnceLock`, so this test owns its
//! own integration binary -- exactly one `config::setup()` per file.
#![cfg(all(
    feature = "config",
    feature = "worker-batch",
    feature = "governor",
    feature = "memory",
    feature = "scaling"
))]

use hyperi_rustlib::config::{self, ConfigOptions};
use hyperi_rustlib::memory::MemoryGuardConfig;
use hyperi_rustlib::worker::BatchProcessingConfig;
use hyperi_rustlib::{ScalingPressureConfig, SelfRegulationConfig, WorkerPoolConfig};

/// Write a temp config file carrying NON-default platform sections, set up the
/// cascade from it, and assert each `from_cascade()` reader reflects the file --
/// NOT its hard-coded default.
#[test]
fn platform_sections_in_config_file_are_honoured() {
    // NON-default values: defaults are min_threads=2, max_chunk_size=10_000,
    // self_regulation.enabled=true, scaling.enabled=true. Every value below is
    // deliberately different so a "still defaulting" regression fails loudly.
    let yaml = "\
worker_pool:
  min_threads: 7
  max_threads: 9
self_regulation:
  enabled: false
batch_processing:
  max_chunk_size: 12345
scaling:
  enabled: false
  memory_gate_threshold: 0.55
memory:
  limit_bytes: 123456789
  pressure_threshold: 0.66
";

    let dir = std::env::temp_dir().join(format!("rustlib-cascade-honoured-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join("config.yaml");
    std::fs::write(&file, yaml).expect("write temp config");

    config::setup(ConfigOptions {
        config_file: Some(file.clone()),
        ..Default::default()
    })
    .expect("config setup");

    // --- worker_pool: min_threads honoured (default is 2) ---
    let wp = WorkerPoolConfig::from_cascade("worker_pool").expect("worker pool config valid");
    assert_eq!(
        wp.min_threads, 7,
        "worker_pool.min_threads must come from the config file, not the default (2)"
    );
    assert_eq!(wp.max_threads, 9, "worker_pool.max_threads from file");

    // --- batch_processing: max_chunk_size honoured (default is 10_000) ---
    let bp =
        BatchProcessingConfig::from_cascade("batch_processing").expect("batch config deserialises");
    assert_eq!(
        bp.max_chunk_size, 12345,
        "batch_processing.max_chunk_size must come from the config file, not the default (10_000)"
    );

    // --- self_regulation: enabled=false honoured (default-ON) ---
    let sr = SelfRegulationConfig::from_cascade();
    assert!(
        !sr.enabled,
        "self_regulation.enabled=false in the file must override the default-ON"
    );

    // --- scaling: enabled=false + threshold honoured (defaults true / 0.8) ---
    let sp = ScalingPressureConfig::from_cascade();
    assert!(
        !sp.enabled,
        "scaling.enabled=false in the file must override the default (true)"
    );
    assert!(
        (sp.memory_gate_threshold - 0.55).abs() < 1e-9,
        "scaling.memory_gate_threshold must come from the file (0.55), not the default (0.8)"
    );

    // --- memory: cascade section honoured by the LIVE guard path (2.8.12) ---
    // Pre-2.8.12 the ServiceRuntime guard used from_env, which reads only the
    // flat {PREFIX}_MEMORY_* env vars and ignored this YAML section entirely.
    // from_cascade_with_env is what runtime.rs now uses; the env prefix below
    // is unset, so it reflects the cascade with no flat-env overlay.
    let mem = MemoryGuardConfig::from_cascade_with_env("RUSTLIB_CASCADE_HONOURED_UNSET");
    assert_eq!(
        mem.limit_bytes, 123_456_789,
        "memory.limit_bytes must come from the config file, not the default (0)"
    );
    assert!(
        (mem.pressure_threshold - 0.66).abs() < 1e-9,
        "memory.pressure_threshold must come from the file (0.66), not the default (0.80)"
    );

    std::fs::remove_dir_all(&dir).ok();
}
