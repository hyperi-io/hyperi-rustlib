// Project:   hyperi-rustlib
// File:      tests/from_cascade_deser_error.rs
// Purpose:   Pin the present-but-malformed-section -> WARN + default behaviour
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Deser-error behaviour test for the 2.8.11 fix (step 5).
//!
//! A `worker_pool` section that is PRESENT but malformed (a field has the wrong
//! type) must fall back to the default config -- it must NOT panic and must NOT
//! error the build. The `unmarshal_key_or_warn` helper emits a `tracing::warn!`
//! in this case (observability), but the data path still gets a usable default.
//!
//! Own integration binary: the config singleton is process-global, so this
//! file owns its `config::setup()`.
#![cfg(all(feature = "config", feature = "worker-pool"))]

use hyperi_rustlib::WorkerPoolConfig;
use hyperi_rustlib::config::{self, ConfigOptions};

#[test]
fn malformed_worker_pool_section_falls_back_to_default() {
    // `min_threads` must be an integer; a string is a type mismatch. The
    // section is PRESENT (so this is the malformed case, not the absent case).
    let yaml = "\
worker_pool:
  min_threads: \"not-a-number\"
  max_threads: 9
";

    let dir = std::env::temp_dir().join(format!("rustlib-deser-err-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let file = dir.join("config.yaml");
    std::fs::write(&file, yaml).expect("write config");

    config::setup(ConfigOptions {
        config_file: Some(file.clone()),
        ..Default::default()
    })
    .expect("config setup");

    // Must not panic, must not error -- returns a VALID default config.
    let wp = WorkerPoolConfig::from_cascade("worker_pool")
        .expect("malformed section must fall back to a valid default, not error");

    // Defaulted, not the file's bogus values.
    assert_eq!(
        wp.min_threads,
        WorkerPoolConfig::default().min_threads,
        "malformed worker_pool must default min_threads, not adopt the bad value"
    );
    assert_eq!(
        wp.max_threads,
        WorkerPoolConfig::default().max_threads,
        "a malformed section defaults wholesale (figment deser is all-or-nothing per key)"
    );

    std::fs::remove_dir_all(&dir).ok();
}
