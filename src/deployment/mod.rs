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
//!     DeploymentContract, HealthContract, ImageProfile, KedaContract, NativeDepsContract,
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
//!     base_image: "ubuntu:24.04".into(),
//!     native_deps: NativeDepsContract::for_rustlib_features(
//!         &["transport-kafka", "spool", "tiered-sink"],
//!         "ubuntu:24.04",
//!     ),
//!     image_profile: ImageProfile::Production,
//!     oci_labels: Default::default(),
//!     schema_version: 1,
//! };
//!
//! // Generate production Dockerfile
//! let dockerfile = generate_dockerfile(&contract);
//!
//! // Generate development Dockerfile (same binary, adds debug tools)
//! let dev_dockerfile = generate_dockerfile(&contract.with_dev_profile());
//!
//! // Generate Helm chart directory
//! // generate_chart(&contract, "chart/").unwrap();
//!
//! // Generate Docker Compose service fragment
//! let compose = generate_compose_fragment(&contract);
//! ```

mod contract;
mod error;
pub mod generate;
mod keda;
mod native_deps;
mod registry;
#[cfg(feature = "deployment-smoke")]
pub mod smoke;
mod validate;

pub use contract::{
    DeploymentContract, HealthContract, ImageProfile, OciLabels, PortContract, SecretEnvContract,
    SecretGroupContract,
};
pub use error::{ContractMismatch, DeploymentError};
pub use generate::{
    ArgocdConfig, generate_argocd_application, generate_chart, generate_compose_fragment,
    generate_container_manifest, generate_dockerfile, generate_runtime_stage,
};
pub use keda::{KedaConfig, KedaContract};
pub use native_deps::{AptRepoContract, NativeDepsContract};
pub use registry::{
    DEFAULT_BASE_IMAGE, DEFAULT_IMAGE_REGISTRY, argocd_repo_url_from_cascade,
    base_image_from_cascade, image_registry_from_cascade,
};
pub use validate::{validate_dockerfile, validate_helm_values};
