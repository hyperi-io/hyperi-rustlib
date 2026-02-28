// Project:   hyperi-rustlib
// File:      src/deployment/mod.rs
// Purpose:   Deployment contract validation for Helm charts and Dockerfiles
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Deployment contract validation for Kubernetes/Helm/Docker.
//!
//! Ensures that deployment artifacts (Helm `values.yaml`, Dockerfile) stay
//! in sync with application defaults. Apps build a [`DeploymentContract`]
//! from their config, then call [`validate_helm_values`] and
//! [`validate_dockerfile`] in their test suite.
//!
//! # Architecture
//!
//! ```text
//! App Config::default()  →  DeploymentContract  →  validate_helm_values("chart/")
//!                                                →  validate_dockerfile("Dockerfile")
//! ```
//!
//! The config cascade (figment) is the SSoT for app defaults. The contract
//! captures the deployment-facing subset, and validation asserts that
//! deployment artifacts match.
//!
//! # Example
//!
//! ```rust,no_run
//! use hyperi_rustlib::deployment::{
//!     DeploymentContract, HealthContract, KedaContract, ContractMismatch,
//! };
//!
//! let contract = DeploymentContract {
//!     app_name: "dfe-loader".into(),
//!     metrics_port: 9090,
//!     health: HealthContract {
//!         liveness_path: "/healthz".into(),
//!         readiness_path: "/readyz".into(),
//!         metrics_path: "/metrics".into(),
//!     },
//!     env_prefix: "DFE_LOADER".into(),
//!     metric_prefix: "loader".into(),
//!     config_mount_path: "/etc/dfe/loader.yaml".into(),
//!     keda: Some(KedaContract::default()),
//! };
//!
//! // In a #[test]:
//! // let mismatches = hyperi_rustlib::deployment::validate_helm_values(&contract, "chart/");
//! // assert!(mismatches.is_empty(), "Contract mismatches: {mismatches:?}");
//! ```

mod contract;
mod error;
mod keda;
mod validate;

pub use contract::{DeploymentContract, HealthContract};
pub use error::{ContractMismatch, DeploymentError};
pub use keda::{KedaConfig, KedaContract};
pub use validate::{validate_dockerfile, validate_helm_values};
