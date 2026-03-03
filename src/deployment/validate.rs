// Project:   hyperi-rustlib
// File:      src/deployment/validate.rs
// Purpose:   Validate Helm charts and Dockerfiles against deployment contract
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Validate deployment artifacts against the app contract.
//!
//! [`validate_helm_values`] checks `chart/values.yaml` and template files.
//! [`validate_dockerfile`] checks `Dockerfile` for port, healthcheck, and config path.

use std::path::Path;

use super::contract::DeploymentContract;
use super::error::{ContractMismatch, DeploymentError};

/// Validate a Helm chart directory against the deployment contract.
///
/// Checks `values.yaml` for port, prometheus annotations, KEDA thresholds,
/// and the deployment template for health probe paths and env var prefix.
///
/// Returns a list of mismatches (empty = all good).
///
/// # Errors
///
/// Returns `DeploymentError` if chart files cannot be read or parsed.
pub fn validate_helm_values(
    contract: &DeploymentContract,
    chart_dir: impl AsRef<Path>,
) -> Result<Vec<ContractMismatch>, DeploymentError> {
    let chart_dir = chart_dir.as_ref();
    let mut mismatches = Vec::new();

    // Parse values.yaml
    let values_path = chart_dir.join("values.yaml");
    let values = read_yaml(&values_path)?;

    // Parse Chart.yaml
    let chart_yaml_path = chart_dir.join("Chart.yaml");
    let chart_yaml = read_yaml(&chart_yaml_path)?;

    // Chart name
    if let Some(name) = chart_yaml["name"].as_str() {
        if name != contract.app_name {
            mismatches.push(ContractMismatch {
                field: "Chart.yaml name".into(),
                expected: contract.app_name.clone(),
                actual: name.into(),
            });
        }
    }

    // Service port
    if let Some(port) = values["service"]["port"].as_u64() {
        if port != u64::from(contract.metrics_port) {
            mismatches.push(ContractMismatch {
                field: "service.port".into(),
                expected: contract.metrics_port.to_string(),
                actual: port.to_string(),
            });
        }
    }

    // Metrics address
    if let Some(addr) = values["config"]["metrics"]["address"].as_str() {
        let expected_addr = format!("0.0.0.0:{}", contract.metrics_port);
        if addr != expected_addr {
            mismatches.push(ContractMismatch {
                field: "config.metrics.address".into(),
                expected: expected_addr,
                actual: addr.into(),
            });
        }
    }

    // Prometheus annotations
    validate_prometheus_annotations(&values, contract, &mut mismatches);

    // KEDA thresholds
    if let Some(keda) = &contract.keda {
        validate_keda_values(&values, keda, &mut mismatches);
    }

    // Deployment template (health probes, env prefix, config mount)
    let deployment_path = chart_dir.join("templates/deployment.yaml");
    if deployment_path.exists() {
        let template = read_text(&deployment_path)?;
        validate_deployment_template(&template, contract, &mut mismatches);
    }

    Ok(mismatches)
}

/// Validate a Dockerfile against the deployment contract.
///
/// Checks EXPOSE port, HEALTHCHECK path, and config mount path.
///
/// Returns a list of mismatches (empty = all good).
///
/// # Errors
///
/// Returns `DeploymentError` if the Dockerfile cannot be read.
pub fn validate_dockerfile(
    contract: &DeploymentContract,
    dockerfile_path: impl AsRef<Path>,
) -> Result<Vec<ContractMismatch>, DeploymentError> {
    let dockerfile_path = dockerfile_path.as_ref();
    let content = read_text(dockerfile_path)?;
    let mut mismatches = Vec::new();

    // EXPOSE port
    let expected_expose = format!("EXPOSE {}", contract.metrics_port);
    if !content.contains(&expected_expose) {
        mismatches.push(ContractMismatch {
            field: "Dockerfile EXPOSE".into(),
            expected: expected_expose,
            actual: extract_line_containing(&content, "EXPOSE"),
        });
    }

    // HEALTHCHECK path
    if !content.contains(&contract.health.liveness_path) {
        mismatches.push(ContractMismatch {
            field: "Dockerfile HEALTHCHECK path".into(),
            expected: contract.health.liveness_path.clone(),
            actual: extract_line_containing(&content, "HEALTHCHECK"),
        });
    }

    // HEALTHCHECK port
    let port_str = format!("localhost:{}", contract.metrics_port);
    if !content.contains(&port_str) {
        mismatches.push(ContractMismatch {
            field: "Dockerfile HEALTHCHECK port".into(),
            expected: port_str,
            actual: extract_line_containing(&content, "HEALTHCHECK"),
        });
    }

    // Config mount path
    if !content.contains(&contract.config_mount_path) {
        mismatches.push(ContractMismatch {
            field: "Dockerfile config path".into(),
            expected: contract.config_mount_path.clone(),
            actual: extract_line_containing(&content, "CMD"),
        });
    }

    Ok(mismatches)
}

// ============================================================================
// Internal helpers
// ============================================================================

fn validate_prometheus_annotations(
    values: &serde_yaml_ng::Value,
    contract: &DeploymentContract,
    mismatches: &mut Vec<ContractMismatch>,
) {
    let annotations = &values["podAnnotations"];

    if let Some(port) = annotations["prometheus.io/port"].as_str() {
        if port != contract.metrics_port.to_string() {
            mismatches.push(ContractMismatch {
                field: "podAnnotations prometheus.io/port".into(),
                expected: contract.metrics_port.to_string(),
                actual: port.into(),
            });
        }
    }

    if let Some(path) = annotations["prometheus.io/path"].as_str() {
        if path != contract.health.metrics_path {
            mismatches.push(ContractMismatch {
                field: "podAnnotations prometheus.io/path".into(),
                expected: contract.health.metrics_path.clone(),
                actual: path.into(),
            });
        }
    }
}

fn validate_keda_values(
    values: &serde_yaml_ng::Value,
    keda: &super::keda::KedaContract,
    mismatches: &mut Vec<ContractMismatch>,
) {
    let chart_keda = &values["keda"];

    check_u64(
        chart_keda,
        "minReplicaCount",
        u64::from(keda.min_replicas),
        "keda.minReplicaCount",
        mismatches,
    );
    check_u64(
        chart_keda,
        "maxReplicaCount",
        u64::from(keda.max_replicas),
        "keda.maxReplicaCount",
        mismatches,
    );
    check_u64(
        chart_keda,
        "pollingInterval",
        u64::from(keda.polling_interval),
        "keda.pollingInterval",
        mismatches,
    );
    check_u64(
        chart_keda,
        "cooldownPeriod",
        u64::from(keda.cooldown_period),
        "keda.cooldownPeriod",
        mismatches,
    );

    // Kafka thresholds (strings in values.yaml)
    let kafka = &chart_keda["kafka"];
    check_str_num(
        kafka,
        "lagThreshold",
        keda.kafka_lag_threshold,
        "keda.kafka.lagThreshold",
        mismatches,
    );
    check_str_num(
        kafka,
        "activationLagThreshold",
        keda.activation_lag_threshold,
        "keda.kafka.activationLagThreshold",
        mismatches,
    );

    // CPU threshold (string in values.yaml)
    let cpu = &chart_keda["cpu"];
    check_str_num(
        cpu,
        "threshold",
        u64::from(keda.cpu_threshold),
        "keda.cpu.threshold",
        mismatches,
    );

    if let Some(enabled) = cpu["enabled"].as_bool() {
        if enabled != keda.cpu_enabled {
            mismatches.push(ContractMismatch {
                field: "keda.cpu.enabled".into(),
                expected: keda.cpu_enabled.to_string(),
                actual: enabled.to_string(),
            });
        }
    }
}

fn validate_deployment_template(
    template: &str,
    contract: &DeploymentContract,
    mismatches: &mut Vec<ContractMismatch>,
) {
    // Health probe paths
    let liveness_pattern = format!("path: {}", contract.health.liveness_path);
    if !template.contains(&liveness_pattern) {
        mismatches.push(ContractMismatch {
            field: "deployment liveness probe path".into(),
            expected: contract.health.liveness_path.clone(),
            actual: "(not found in template)".into(),
        });
    }

    let readiness_pattern = format!("path: {}", contract.health.readiness_path);
    if !template.contains(&readiness_pattern) {
        mismatches.push(ContractMismatch {
            field: "deployment readiness probe path".into(),
            expected: contract.health.readiness_path.clone(),
            actual: "(not found in template)".into(),
        });
    }

    // Env var prefix (check for __ nesting pattern)
    let env_pattern = format!("{}__", contract.env_prefix);
    if !template.contains(&env_pattern) {
        mismatches.push(ContractMismatch {
            field: "deployment env var prefix".into(),
            expected: env_pattern,
            actual: "(not found in template)".into(),
        });
    }

    // Config mount path
    if !template.contains(&contract.config_mount_path)
        && !template.contains(
            contract
                .config_mount_path
                .rsplit('/')
                .nth(1)
                .unwrap_or("/etc"),
        )
    {
        mismatches.push(ContractMismatch {
            field: "deployment config mount path".into(),
            expected: contract.config_mount_path.clone(),
            actual: "(not found in template)".into(),
        });
    }
}

/// Check a YAML integer field against an expected value.
fn check_u64(
    parent: &serde_yaml_ng::Value,
    key: &str,
    expected: u64,
    label: &str,
    mismatches: &mut Vec<ContractMismatch>,
) {
    if let Some(val) = parent[key].as_u64() {
        if val != expected {
            mismatches.push(ContractMismatch {
                field: label.into(),
                expected: expected.to_string(),
                actual: val.to_string(),
            });
        }
    }
}

/// Check a YAML string field that represents a number.
fn check_str_num(
    parent: &serde_yaml_ng::Value,
    key: &str,
    expected: u64,
    label: &str,
    mismatches: &mut Vec<ContractMismatch>,
) {
    if let Some(val) = parent[key].as_str() {
        if val != expected.to_string() {
            mismatches.push(ContractMismatch {
                field: label.into(),
                expected: expected.to_string(),
                actual: val.into(),
            });
        }
    }
}

fn read_yaml(path: &Path) -> Result<serde_yaml_ng::Value, DeploymentError> {
    if !path.exists() {
        return Err(DeploymentError::NotFound(path.display().to_string()));
    }
    let content = std::fs::read_to_string(path).map_err(|e| DeploymentError::ReadFile {
        path: path.display().to_string(),
        source: e,
    })?;
    serde_yaml_ng::from_str(&content).map_err(|e| DeploymentError::ParseYaml {
        path: path.display().to_string(),
        source: e,
    })
}

fn read_text(path: &Path) -> Result<String, DeploymentError> {
    if !path.exists() {
        return Err(DeploymentError::NotFound(path.display().to_string()));
    }
    std::fs::read_to_string(path).map_err(|e| DeploymentError::ReadFile {
        path: path.display().to_string(),
        source: e,
    })
}

fn extract_line_containing(content: &str, keyword: &str) -> String {
    content
        .lines()
        .find(|line| line.contains(keyword))
        .unwrap_or("(not found)")
        .trim()
        .to_string()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment::keda::KedaContract;

    fn test_contract() -> DeploymentContract {
        DeploymentContract {
            app_name: "test-app".into(),
            binary_name: "test-app".into(),
            description: "Test application".into(),
            metrics_port: 9090,
            health: super::super::HealthContract::default(),
            env_prefix: "TEST_APP".into(),
            metric_prefix: "test".into(),
            config_mount_path: "/etc/test/config.yaml".into(),
            image_registry: "ghcr.io/hyperi-io".into(),
            extra_ports: vec![],
            entrypoint_args: vec!["--config".into(), "/etc/test/config.yaml".into()],
            secrets: vec![],
            default_config: None,
            depends_on: vec![],
            keda: Some(KedaContract::default()),
            base_image: "ubuntu:24.04".into(),
        }
    }

    #[test]
    fn test_validate_helm_not_found() {
        let contract = test_contract();
        let result = validate_helm_values(&contract, "/nonexistent/chart");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_dockerfile_not_found() {
        let contract = test_contract();
        let result = validate_dockerfile(&contract, "/nonexistent/Dockerfile");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_dockerfile_with_tempfile() {
        let dir = tempfile::tempdir().unwrap();
        let dockerfile = dir.path().join("Dockerfile");
        std::fs::write(
            &dockerfile,
            "FROM ubuntu:24.04\n\
             EXPOSE 9090\n\
             HEALTHCHECK CMD curl -sf http://localhost:9090/healthz\n\
             CMD [\"--config\", \"/etc/test/config.yaml\"]\n",
        )
        .unwrap();

        let contract = test_contract();
        let mismatches = validate_dockerfile(&contract, &dockerfile).unwrap();
        assert!(
            mismatches.is_empty(),
            "Unexpected mismatches: {mismatches:?}"
        );
    }

    #[test]
    fn test_validate_dockerfile_wrong_port() {
        let dir = tempfile::tempdir().unwrap();
        let dockerfile = dir.path().join("Dockerfile");
        std::fs::write(
            &dockerfile,
            "FROM ubuntu:24.04\n\
             EXPOSE 8080\n\
             HEALTHCHECK CMD curl -sf http://localhost:8080/healthz\n\
             CMD [\"--config\", \"/etc/test/config.yaml\"]\n",
        )
        .unwrap();

        let contract = test_contract();
        let mismatches = validate_dockerfile(&contract, &dockerfile).unwrap();
        assert!(!mismatches.is_empty());
        assert!(mismatches.iter().any(|m| m.field.contains("EXPOSE")));
    }

    #[test]
    fn test_validate_helm_with_tempdir() {
        let dir = tempfile::tempdir().unwrap();
        let chart_dir = dir.path();

        // Chart.yaml
        std::fs::write(
            chart_dir.join("Chart.yaml"),
            "apiVersion: v2\nname: test-app\nversion: 0.1.0\n",
        )
        .unwrap();

        // values.yaml
        std::fs::write(
            chart_dir.join("values.yaml"),
            "service:\n  port: 9090\n\
             config:\n  metrics:\n    address: \"0.0.0.0:9090\"\n\
             podAnnotations:\n  prometheus.io/port: \"9090\"\n  prometheus.io/path: \"/metrics\"\n\
             keda:\n  minReplicaCount: 1\n  maxReplicaCount: 10\n  pollingInterval: 15\n  cooldownPeriod: 300\n\
               kafka:\n    lagThreshold: \"1000\"\n    activationLagThreshold: \"0\"\n\
               cpu:\n    enabled: true\n    threshold: \"80\"\n",
        )
        .unwrap();

        // templates/deployment.yaml
        std::fs::create_dir_all(chart_dir.join("templates")).unwrap();
        std::fs::write(
            chart_dir.join("templates/deployment.yaml"),
            "path: /healthz\npath: /readyz\n\
             TEST_APP__KAFKA__PASSWORD\n\
             /etc/test/config.yaml\n",
        )
        .unwrap();

        let contract = test_contract();
        let mismatches = validate_helm_values(&contract, chart_dir).unwrap();
        assert!(
            mismatches.is_empty(),
            "Unexpected mismatches: {mismatches:?}"
        );
    }

    #[test]
    fn test_validate_helm_wrong_port() {
        let dir = tempfile::tempdir().unwrap();
        let chart_dir = dir.path();

        std::fs::write(
            chart_dir.join("Chart.yaml"),
            "apiVersion: v2\nname: test-app\nversion: 0.1.0\n",
        )
        .unwrap();
        std::fs::write(chart_dir.join("values.yaml"), "service:\n  port: 8080\n").unwrap();

        let contract = test_contract();
        let mismatches = validate_helm_values(&contract, chart_dir).unwrap();
        assert!(mismatches.iter().any(|m| m.field == "service.port"));
    }

    #[test]
    fn test_contract_mismatch_display() {
        let m = ContractMismatch {
            field: "service.port".into(),
            expected: "9090".into(),
            actual: "8080".into(),
        };
        assert_eq!(m.to_string(), "service.port: expected '9090', got '8080'");
    }
}
