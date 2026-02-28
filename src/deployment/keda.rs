// Project:   hyperi-rustlib
// File:      src/deployment/keda.rs
// Purpose:   KEDA autoscaling configuration and contract types
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! KEDA autoscaling configuration.
//!
//! [`KedaConfig`] lives in the app's config cascade so thresholds are
//! overridable via env vars (e.g., `DFE_LOADER__KEDA__KAFKA_LAG_THRESHOLD=5000`).
//!
//! [`KedaContract`] is the subset validated against Helm `values.yaml`.

use serde::{Deserialize, Serialize};

/// KEDA autoscaling configuration for the app config cascade.
///
/// Include this in your app's `Config` struct so KEDA thresholds
/// participate in the figment cascade and are env-var overridable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KedaConfig {
    /// Whether KEDA scaling is enabled.
    pub enabled: bool,
    /// Minimum replica count (0 = scale-to-zero).
    pub min_replicas: u32,
    /// Maximum replica count.
    pub max_replicas: u32,
    /// Seconds between KEDA polling the scaler.
    pub polling_interval: u32,
    /// Seconds before scale-down after load drops.
    pub cooldown_period: u32,
    /// Scale when consumer group lag exceeds this per partition.
    pub kafka_lag_threshold: u64,
    /// Wake from zero replicas when lag exceeds this.
    pub activation_lag_threshold: u64,
    /// Enable CPU-based scaling trigger.
    pub cpu_enabled: bool,
    /// CPU utilisation percentage threshold.
    pub cpu_threshold: u32,
}

impl Default for KedaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_replicas: 1,
            max_replicas: 10,
            polling_interval: 15,
            cooldown_period: 300,
            kafka_lag_threshold: 1000,
            activation_lag_threshold: 0,
            cpu_enabled: true,
            cpu_threshold: 80,
        }
    }
}

/// KEDA contract points validated against Helm `values.yaml`.
///
/// Built from [`KedaConfig`] defaults. Use [`KedaContract::from_config`]
/// to convert.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KedaContract {
    pub min_replicas: u32,
    pub max_replicas: u32,
    pub polling_interval: u32,
    pub cooldown_period: u32,
    pub kafka_lag_threshold: u64,
    pub activation_lag_threshold: u64,
    pub cpu_enabled: bool,
    pub cpu_threshold: u32,
}

impl KedaContract {
    /// Build a contract from a [`KedaConfig`].
    #[must_use]
    pub fn from_config(config: &KedaConfig) -> Self {
        Self {
            min_replicas: config.min_replicas,
            max_replicas: config.max_replicas,
            polling_interval: config.polling_interval,
            cooldown_period: config.cooldown_period,
            kafka_lag_threshold: config.kafka_lag_threshold,
            activation_lag_threshold: config.activation_lag_threshold,
            cpu_enabled: config.cpu_enabled,
            cpu_threshold: config.cpu_threshold,
        }
    }
}

impl Default for KedaContract {
    fn default() -> Self {
        Self::from_config(&KedaConfig::default())
    }
}

impl From<&KedaConfig> for KedaContract {
    fn from(config: &KedaConfig) -> Self {
        Self::from_config(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keda_config_defaults() {
        let cfg = KedaConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.min_replicas, 1);
        assert_eq!(cfg.max_replicas, 10);
        assert_eq!(cfg.polling_interval, 15);
        assert_eq!(cfg.cooldown_period, 300);
        assert_eq!(cfg.kafka_lag_threshold, 1000);
        assert_eq!(cfg.activation_lag_threshold, 0);
        assert!(cfg.cpu_enabled);
        assert_eq!(cfg.cpu_threshold, 80);
    }

    #[test]
    fn test_keda_contract_from_config() {
        let cfg = KedaConfig {
            kafka_lag_threshold: 5000,
            cpu_threshold: 90,
            ..Default::default()
        };
        let contract = KedaContract::from_config(&cfg);
        assert_eq!(contract.kafka_lag_threshold, 5000);
        assert_eq!(contract.cpu_threshold, 90);
    }

    #[test]
    fn test_keda_config_serde_roundtrip() {
        let cfg = KedaConfig::default();
        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        let parsed: KedaConfig = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(parsed.kafka_lag_threshold, cfg.kafka_lag_threshold);
    }
}
