// Project:   hs-rustlib
// File:      tests/parity/env_parity.rs
// Purpose:   Environment detection parity tests
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Environment detection parity tests.
//!
//! These tests verify that environment detection behaves identically
//! to hs-golib's env package.

use hs_rustlib::env::{get_app_env, Environment};

/// Test that Environment::detect() returns valid enum.
#[test]
fn test_detect_returns_valid_environment() {
    let env = Environment::detect();
    assert!(matches!(
        env,
        Environment::Kubernetes
            | Environment::Docker
            | Environment::Container
            | Environment::BareMetal
    ));
}

/// Test is_container() matches Go behaviour.
#[test]
fn test_is_container_parity() {
    // K8s, Docker, Container should return true
    assert!(Environment::Kubernetes.is_container());
    assert!(Environment::Docker.is_container());
    assert!(Environment::Container.is_container());

    // BareMetal should return false
    assert!(!Environment::BareMetal.is_container());
}

/// Test get_app_env() priority: APP_ENV > ENVIRONMENT > ENV > "development".
#[test]
fn test_get_app_env_priority() {
    // Clear all
    std::env::remove_var("APP_ENV");
    std::env::remove_var("ENVIRONMENT");
    std::env::remove_var("ENV");

    // Default should be "development"
    assert_eq!(get_app_env(), "development");

    // ENV should override default
    std::env::set_var("ENV", "staging");
    assert_eq!(get_app_env(), "staging");

    // ENVIRONMENT should override ENV
    std::env::set_var("ENVIRONMENT", "production");
    assert_eq!(get_app_env(), "production");

    // APP_ENV should override all
    std::env::set_var("APP_ENV", "testing");
    assert_eq!(get_app_env(), "testing");

    // Cleanup
    std::env::remove_var("APP_ENV");
    std::env::remove_var("ENVIRONMENT");
    std::env::remove_var("ENV");
}

/// Test Display implementation matches Go string output.
#[test]
fn test_environment_display_parity() {
    assert_eq!(Environment::Kubernetes.to_string(), "kubernetes");
    assert_eq!(Environment::Docker.to_string(), "docker");
    assert_eq!(Environment::Container.to_string(), "container");
    assert_eq!(Environment::BareMetal.to_string(), "bare_metal");
}
