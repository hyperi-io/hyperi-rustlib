// Project:   scalo
// File:      tests/rename_aliases.rs
// Purpose:   Verify deprecated brand aliases resolve to the renamed types
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Compile-time + runtime checks that the deprecated brand aliases
//! (`DfeMetrics`, `DfeSource`, `DfeApp`) still name the renamed types, so the
//! DFE consumers compile unchanged through the scalo transition.

#![allow(deprecated)]

/// `DfeSource` aliases `KafkaSource` (kafka_config is not feature-gated).
#[test]
fn dfe_source_alias_is_kafka_source() {
    let via_alias: scalo::DfeSource = scalo::KafkaSource::new("syslog");
    assert_eq!(via_alias, scalo::DfeSource::new("syslog"));
}

/// `DfeMetrics` aliases `ServiceMetrics`. Compiles only if they are the same
/// type (the identity function returns the argument under the new name).
#[cfg(feature = "metrics-dfe")]
#[allow(dead_code)]
fn _dfe_metrics_alias_is_service_metrics(x: scalo::metrics::ServiceMetrics) -> scalo::DfeMetrics {
    x
}

/// `DfeApp` aliases the `ServiceApp` trait. Compiles only if the bound
/// `A: DfeApp` satisfies `A: ServiceApp` (i.e. they are the same trait).
#[cfg(feature = "cli-service")]
#[allow(dead_code)]
fn _dfe_app_alias_is_service_app<A: scalo::DfeApp>() {
    fn requires_service_app<A: scalo::cli::ServiceApp>() {}
    requires_service_app::<A>();
}
