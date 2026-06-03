// Project:   hyperi-rustlib
// File:      src/deployment/generate/manifest.rs
// Purpose:   Container manifest (CI-consumable JSON) generation
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

use crate::deployment::contract::{DeploymentContract, ImageProfile};

// ============================================================================
// Container Manifest (CI-consumable JSON)
// ============================================================================

/// Generate a container manifest JSON for CI consumption.
///
/// This is the minimal subset of the deployment contract that CI needs to
/// build the container image. No secrets, no K8s-specific config.
///
/// # Errors
///
/// Returns an error string if JSON serialisation fails.
pub fn generate_container_manifest(contract: &DeploymentContract) -> Result<String, String> {
    let binary = contract.binary();

    let apt_repos: Vec<serde_json::Value> = contract
        .native_deps
        .apt_repos
        .iter()
        .map(|r| {
            serde_json::json!({
                "key_url": r.key_url,
                "keyring": r.keyring,
                "url": r.url,
                "codename": r.codename,
                "packages": r.packages,
            })
        })
        .collect();

    let mut expose_ports: Vec<u16> = vec![contract.metrics_port];
    expose_ports.extend(contract.extra_ports.iter().map(|p| p.port));

    let profile_str = match contract.image_profile {
        ImageProfile::Production => "production",
        ImageProfile::Development => "development",
    };

    let title = if contract.oci_labels.title.is_empty() {
        &contract.app_name
    } else {
        &contract.oci_labels.title
    };

    let manifest = serde_json::json!({
        "schema_version": "1",
        "app_name": contract.app_name,
        "binary_name": binary,
        "base_image": contract.base_image,
        "image_registry": contract.image_registry,
        "image_profile": profile_str,
        "runtime_packages": {
            "apt_repos": apt_repos,
            "apt_packages": contract.native_deps.apt_packages,
        },
        "expose_ports": expose_ports,
        "healthcheck": {
            "path": contract.health.liveness_path,
            "port": contract.metrics_port,
            "interval": "30s",
            "timeout": "3s",
            "start_period": "5s",
            "retries": 3,
        },
        "entrypoint": [binary],
        "cmd": contract.entrypoint_args,
        "user": "appuser",
        "uid": 1000,
        "labels": {
            "io.hyperi.profile": profile_str,
            "io.hyperi.app": contract.app_name,
            "io.hyperi.metrics_port": contract.metrics_port.to_string(),
            "org.opencontainers.image.title": title,
            "org.opencontainers.image.description": contract.oci_labels.description,
            "org.opencontainers.image.vendor": contract.oci_labels.vendor,
            "org.opencontainers.image.licenses": contract.oci_labels.licenses,
        },
    });

    serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("container manifest JSON failed: {e}"))
}
