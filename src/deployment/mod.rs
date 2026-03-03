// Project:   hyperi-rustlib
// File:      src/deployment/mod.rs
// Purpose:   Deployment contract validation and generation for Helm charts and Dockerfiles
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Deployment contract validation and generation for Kubernetes/Helm/Docker.
//!
//! Apps provide ~20% customisation via [`DeploymentContract`]; this module
//! generates ~80% boilerplate (Dockerfile, Helm chart, Compose fragment) and
//! validates existing artifacts against the contract.
//!
//! # Architecture
//!
//! ```text
//! App Config::default()  →  DeploymentContract  →  generate_chart("chart/")
//!                                                →  generate_dockerfile()
//!                                                →  generate_compose_fragment()
//!                                                →  validate_helm_values("chart/")
//!                                                →  validate_dockerfile("Dockerfile")
//! ```
//!
//! The config cascade (figment) is the SSoT for app defaults. The contract
//! captures the deployment-facing subset. Generation creates artifacts from
//! scratch; validation asserts that existing artifacts match.
//!
//! # Example
//!
//! ```rust,no_run
//! use hyperi_rustlib::deployment::{
//!     DeploymentContract, HealthContract, KedaContract,
//!     generate_dockerfile, generate_chart, generate_compose_fragment,
//! };
//!
//! let contract = DeploymentContract {
//!     app_name: "dfe-loader".into(),
//!     binary_name: "dfe-loader".into(),
//!     description: "High-performance data loader".into(),
//!     metrics_port: 9090,
//!     health: HealthContract::default(),
//!     env_prefix: "DFE_LOADER".into(),
//!     metric_prefix: "loader".into(),
//!     config_mount_path: "/etc/dfe/loader.yaml".into(),
//!     image_registry: "ghcr.io/hyperi-io".into(),
//!     extra_ports: vec![],
//!     entrypoint_args: vec!["--config".into(), "/etc/dfe/loader.yaml".into()],
//!     secrets: vec![],
//!     default_config: None,
//!     depends_on: vec!["kafka".into(), "clickhouse".into()],
//!     keda: Some(KedaContract::default()),
//! };
//!
//! // Generate Dockerfile
//! let dockerfile = generate_dockerfile(&contract);
//!
//! // Generate Helm chart directory
//! // generate_chart(&contract, "chart/").unwrap();
//!
//! // Generate Docker Compose service fragment
//! let compose = generate_compose_fragment(&contract);
//! ```

mod contract;
mod error;
mod generate;
mod keda;
mod validate;

pub use contract::{
    DeploymentContract, HealthContract, PortContract, SecretEnvContract, SecretGroupContract,
};
pub use error::{ContractMismatch, DeploymentError};
pub use generate::{generate_chart, generate_compose_fragment, generate_dockerfile};
pub use keda::{KedaConfig, KedaContract};
pub use validate::{validate_dockerfile, validate_helm_values};
