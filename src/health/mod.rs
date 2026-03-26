// Project:   hyperi-rustlib
// File:      src/health/mod.rs
// Purpose:   Unified health registry for service health state
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Unified health registry for service readiness and liveness.
//!
//! Provides a global singleton [`HealthRegistry`] that modules register
//! into at construction. The `/readyz` endpoint (or any health check)
//! queries the registry to determine overall service health.
//!
//! # Usage
//!
//! ```rust
//! use hyperi_rustlib::health::{HealthRegistry, HealthStatus};
//!
//! // Register a component health check at construction
//! HealthRegistry::register("kafka_consumer", || HealthStatus::Healthy);
//!
//! // Query overall health
//! assert!(HealthRegistry::is_ready());
//! ```

pub mod registry;

pub use registry::{HealthRegistry, HealthStatus};
