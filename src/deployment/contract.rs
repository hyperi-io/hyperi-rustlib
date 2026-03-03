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
/// compare Helm charts and Dockerfiles against these values. Generation
/// functions create deployment artifacts (Dockerfile, Helm chart, Compose
/// fragment) from scratch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentContract {
    /// Application name (e.g., "dfe-loader") — matched against Chart.yaml `name`.
    pub app_name: String,

    /// Binary name (e.g., "dfe-loader"). Defaults to app_name if empty.
    #[serde(default)]
    pub binary_name: String,

    /// One-line description for Chart.yaml.
    #[serde(default)]
    pub description: String,

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

    /// Container registry base (e.g., "ghcr.io/hyperi-io").
    #[serde(default = "default_image_registry")]
    pub image_registry: String,

    /// Additional ports beyond metrics (e.g., HTTP data port for receiver).
    #[serde(default)]
    pub extra_ports: Vec<PortContract>,

    /// Default ENTRYPOINT args (e.g., `["--config", "/etc/dfe/loader.yaml"]`).
    #[serde(default)]
    pub entrypoint_args: Vec<String>,

    /// Secret groups injected from K8s Secrets.
    #[serde(default)]
    pub secrets: Vec<SecretGroupContract>,

    /// App-specific config YAML for values.yaml (serialised as serde_json::Value).
    #[serde(default)]
    pub default_config: Option<serde_json::Value>,

    /// Docker Compose service dependencies (e.g., `["kafka", "clickhouse"]`).
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// KEDA autoscaling contract (None if KEDA not used).
    pub keda: Option<KedaContract>,

    /// Base container image for the runtime stage.
    // I don't like doing it this way but it's the best compromise option
    #[serde(default = "default_base_image")]
    pub base_image: String,
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

/// Additional container port beyond the metrics port.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortContract {
    /// Port name (e.g., "http").
    pub name: String,
    /// Port number (e.g., 8080).
    pub port: u16,
    /// Protocol (default: "TCP").
    #[serde(default = "default_protocol")]
    pub protocol: String,
}

/// A group of secrets from the same K8s Secret (e.g., "kafka", "clickhouse").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretGroupContract {
    /// Group name (e.g., "kafka", "clickhouse").
    /// Used in values.yaml section name and helper template names.
    pub group_name: String,

    /// Environment variables injected from this secret group.
    pub env_vars: Vec<SecretEnvContract>,
}

/// A single environment variable sourced from a K8s Secret.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretEnvContract {
    /// Full env var name (e.g., "DFE_LOADER__KAFKA__PASSWORD").
    pub env_var: String,

    /// Key name in values.yaml secretKeys and default values
    /// (e.g., "password", "username").
    pub key_name: String,

    /// Default K8s secret key name (e.g., "kafka-password").
    pub secret_key: String,
}

fn default_base_image() -> String {
    "ubuntu:24.04".to_string()
}

fn default_image_registry() -> String {
    "ghcr.io/hyperi-io".to_string()
}

fn default_protocol() -> String {
    "TCP".to_string()
}

impl DeploymentContract {
    /// Get the effective binary name (falls back to app_name).
    #[must_use]
    pub fn binary(&self) -> &str {
        if self.binary_name.is_empty() {
            &self.app_name
        } else {
            &self.binary_name
        }
    }

    /// Get the config file name from the mount path (e.g., "loader.yaml").
    #[must_use]
    pub fn config_filename(&self) -> &str {
        self.config_mount_path
            .rsplit('/')
            .next()
            .unwrap_or("config.yaml")
    }

    /// Get the config mount directory (e.g., "/etc/dfe").
    #[must_use]
    pub fn config_dir(&self) -> &str {
        self.config_mount_path
            .rsplit_once('/')
            .map_or("/etc", |(dir, _)| dir)
    }

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
            binary_name: String::new(),
            description: String::new(),
            image_registry: default_image_registry(),
            extra_ports: vec![],
            entrypoint_args: vec![],
            secrets: vec![],
            default_config: None,
            depends_on: vec![],
            base_image: "ubuntu:24.04".into(),
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
            binary_name: String::new(),
            description: String::new(),
            image_registry: default_image_registry(),
            extra_ports: vec![],
            entrypoint_args: vec![],
            secrets: vec![],
            default_config: None,
            depends_on: vec![],
            base_image: "ubuntu:24.04".into(),
        };
        let json = contract.to_json();
        let parsed: DeploymentContract = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.app_name, "roundtrip");
        assert_eq!(parsed.metrics_port, 8080);
    }

    #[test]
    fn test_binary_name_fallback() {
        let contract = DeploymentContract {
            app_name: "my-app".into(),
            binary_name: String::new(),
            metrics_port: 9090,
            health: HealthContract::default(),
            env_prefix: "MY_APP".into(),
            metric_prefix: "app".into(),
            config_mount_path: "/etc/app/config.yaml".into(),
            keda: None,
            description: String::new(),
            image_registry: default_image_registry(),
            extra_ports: vec![],
            entrypoint_args: vec![],
            secrets: vec![],
            default_config: None,
            depends_on: vec![],
            base_image: "ubuntu:24.04".into(),
        };
        assert_eq!(contract.binary(), "my-app");
    }

    #[test]
    fn test_config_filename() {
        let contract = DeploymentContract {
            app_name: "test".into(),
            config_mount_path: "/etc/dfe/loader.yaml".into(),
            metrics_port: 9090,
            health: HealthContract::default(),
            env_prefix: "T".into(),
            metric_prefix: "t".into(),
            keda: None,
            binary_name: String::new(),
            description: String::new(),
            image_registry: default_image_registry(),
            extra_ports: vec![],
            entrypoint_args: vec![],
            secrets: vec![],
            default_config: None,
            depends_on: vec![],
            base_image: "ubuntu:24.04".into(),
        };
        assert_eq!(contract.config_filename(), "loader.yaml");
        assert_eq!(contract.config_dir(), "/etc/dfe");
    }
}
