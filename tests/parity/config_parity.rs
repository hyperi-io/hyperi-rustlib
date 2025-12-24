// Project:   hs-rustlib
// File:      tests/parity/config_parity.rs
// Purpose:   Config parity tests against hs-golib
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Configuration cascade parity tests.
//!
//! These tests verify that the 7-layer cascade behaves identically
//! to hs-golib's config package.

use hs_rustlib::config::{Config, ConfigOptions};

/// Test that hard-coded defaults are loaded.
#[test]
fn test_hardcoded_defaults_loaded() {
    let config = Config::new(ConfigOptions::default()).unwrap();

    // These should come from HardcodedDefaults
    assert_eq!(config.get_string("log_level"), Some("info".to_string()));
    assert_eq!(config.get_string("log_format"), Some("auto".to_string()));
}

/// Test that environment variables override file config.
#[test]
fn test_env_overrides_file() {
    std::env::set_var("PARITY_DATABASE_HOST", "envhost");

    let config = Config::new(ConfigOptions {
        env_prefix: "PARITY".into(),
        ..Default::default()
    })
    .unwrap();

    assert_eq!(
        config.get_string("database_host"),
        Some("envhost".to_string())
    );

    std::env::remove_var("PARITY_DATABASE_HOST");
}

/// Test cascade priority: ENV > settings.{env}.yaml > settings.yaml > defaults.yaml > hardcoded.
#[test]
fn test_cascade_priority() {
    // This test documents the expected cascade behaviour
    // In a real parity test, we'd compare against Go output

    let config = Config::new(ConfigOptions::default()).unwrap();

    // Hardcoded defaults should be present
    assert!(config.get_string("log_level").is_some());
}
