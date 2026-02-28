// Project:   hyperi-rustlib
// File:      src/deployment/contract.rs
// Purpose:   Deployment contract types
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Deployment contract types.

use serde::{Deserialize, Serialize};

use super::keda::KedaContract;

/// Deployment-facing contract points derived from the app config cascade.
///
/// Apps build this from their `Config::default()`. Validation functions
/// compare Helm charts and Dockerfiles against these values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentContract {
    /// Application name (e.g., "dfe-loader") — matched against Chart.yaml `name`.
    pub app_name: String,

    /// Metrics/health listen port (e.g., 9090).
    pub metrics_port: u16,

    /// Health probe endpoint paths.
    pub health: HealthContract,

    /// Environment variable prefix (e.g., "DFE_LOADER").
    /// Used with `__` nesting for figment config cascade.
    pub env_prefix: String,

    /// Prometheus metric namespace/prefix (e.g., "loader").
    pub metric_prefix: String,

    /// Config file mount path (e.g., "/etc/dfe/loader.yaml").
    pub config_mount_path: String,

    /// KEDA autoscaling contract (None if KEDA not used).
    pub keda: Option<KedaContract>,
}

/// Health probe endpoint paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthContract {
    /// Liveness probe path (e.g., "/healthz").
    pub liveness_path: String,

    /// Readiness probe path (e.g., "/readyz").
    pub readiness_path: String,

    /// Prometheus metrics path (e.g., "/metrics").
    pub metrics_path: String,
}

impl DeploymentContract {
    /// Serialise the contract to JSON for `--emit-contract` CLI support.
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Serialise the contract to YAML.
    #[must_use]
    pub fn to_yaml(&self) -> String {
        serde_yaml_ng::to_string(self).unwrap_or_default()
    }
}

impl Default for HealthContract {
    fn default() -> Self {
        Self {
            liveness_path: "/healthz".to_string(),
            readiness_path: "/readyz".to_string(),
            metrics_path: "/metrics".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_contract_defaults() {
        let h = HealthContract::default();
        assert_eq!(h.liveness_path, "/healthz");
        assert_eq!(h.readiness_path, "/readyz");
        assert_eq!(h.metrics_path, "/metrics");
    }

    #[test]
    fn test_contract_to_json() {
        let contract = DeploymentContract {
            app_name: "test-app".into(),
            metrics_port: 9090,
            health: HealthContract::default(),
            env_prefix: "TEST_APP".into(),
            metric_prefix: "test".into(),
            config_mount_path: "/etc/test/config.yaml".into(),
            keda: None,
        };
        let json = contract.to_json();
        assert!(json.contains("test-app"));
        assert!(json.contains("9090"));
    }

    #[test]
    fn test_contract_roundtrip_json() {
        let contract = DeploymentContract {
            app_name: "roundtrip".into(),
            metrics_port: 8080,
            health: HealthContract::default(),
            env_prefix: "RT".into(),
            metric_prefix: "rt".into(),
            config_mount_path: "/config.yaml".into(),
            keda: None,
        };
        let json = contract.to_json();
        let parsed: DeploymentContract = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.app_name, "roundtrip");
        assert_eq!(parsed.metrics_port, 8080);
    }
}
