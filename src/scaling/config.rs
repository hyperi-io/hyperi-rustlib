// Project:   hyperi-rustlib
// File:      src/scaling/config.rs
// Purpose:   Scaling pressure configuration types
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Configuration for the scaling pressure calculator.
//!
//! [`ScalingPressureConfig`] provides the base gate thresholds shared across
//! all apps. Per-component weights and saturation points are defined in each
//! app's own config struct and passed as [`ScalingComponent`] at construction.

use serde::{Deserialize, Serialize};

/// Base configuration for scaling pressure calculation.
///
/// Lives in the app's config cascade so thresholds are env-var overridable
/// (e.g., `DFE_LOADER__SCALING__MEMORY_GATE_THRESHOLD=0.9`).
///
/// Component weights and saturation points are app-specific — defined in
/// each app's config and passed to [`super::ScalingPressure::new`] via
/// [`ScalingComponent`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScalingPressureConfig {
    /// Enable scaling pressure calculation.
    /// When disabled, `calculate()` always returns 0.0.
    pub enabled: bool,

    /// Memory usage ratio that triggers the memory gate (0.0–1.0).
    ///
    /// When `memory_used / memory_limit >= threshold`, scaling pressure
    /// is forced to 100.0 to trigger immediate scale-up before OOM.
    pub memory_gate_threshold: f64,
}

impl Default for ScalingPressureConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            memory_gate_threshold: 0.8,
        }
    }
}

impl ScalingPressureConfig {
    /// Load from the config cascade under the `scaling` key.
    ///
    /// Falls back to [`ScalingPressureConfig::default()`] if config is not
    /// initialised or the key is absent.
    #[must_use]
    pub fn from_cascade() -> Self {
        #[cfg(feature = "config")]
        {
            if let Some(cfg) = crate::config::try_get()
                && let Ok(scaling) = cfg.unmarshal_key_registered::<Self>("scaling")
            {
                return scaling;
            }
        }
        Self::default()
    }
}

/// Named scaling component with weight and saturation point.
///
/// Apps define their components with service-specific signals:
///
/// ```rust
/// use hyperi_rustlib::scaling::ScalingComponent;
///
/// let components = vec![
///     ScalingComponent::new("kafka_lag", 0.35, 100_000.0),
///     ScalingComponent::new("buffer_depth", 0.25, 10_000.0),
///     ScalingComponent::new("insert_latency", 0.15, 5.0),
///     ScalingComponent::new("memory", 0.15, 1.0),
///     ScalingComponent::new("errors", 0.10, 100.0),
/// ];
/// ```
#[derive(Debug, Clone)]
pub struct ScalingComponent {
    /// Component name (e.g., "kafka_lag", "buffer_depth").
    pub name: String,
    /// Relative weight (0.0–1.0). All weights should sum to ~1.0.
    pub weight: f64,
    /// Value at which this component contributes its full weight.
    /// Score = `(value / saturation).min(1.0) * weight * 100.0`.
    pub saturation: f64,
}

impl ScalingComponent {
    /// Create a new scaling component.
    #[must_use]
    pub fn new(name: impl Into<String>, weight: f64, saturation: f64) -> Self {
        Self {
            name: name.into(),
            weight,
            saturation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults() {
        let config = ScalingPressureConfig::default();
        assert!(config.enabled);
        assert!((config.memory_gate_threshold - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = ScalingPressureConfig {
            enabled: false,
            memory_gate_threshold: 0.9,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ScalingPressureConfig = serde_json::from_str(&json).unwrap();
        assert!(!parsed.enabled);
        assert!((parsed.memory_gate_threshold - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn test_component_new() {
        let c = ScalingComponent::new("kafka_lag", 0.35, 100_000.0);
        assert_eq!(c.name, "kafka_lag");
        assert!((c.weight - 0.35).abs() < f64::EPSILON);
        assert!((c.saturation - 100_000.0).abs() < f64::EPSILON);
    }
}
