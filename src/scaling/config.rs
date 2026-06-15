// Project:   hyperi-rustlib
// File:      src/scaling/config.rs
// Purpose:   Scaling pressure configuration types
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Configuration for the scaling pressure calculator.
//!
//! [`ScalingPressureConfig`] provides the base gate thresholds shared across
//! all apps. Per-component weights and saturation points are defined in each
//! app's own config struct and passed as [`ScalingComponent`] at construction.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Base configuration for scaling pressure calculation.
///
/// Lives in the app's config cascade so thresholds are env-var overridable
/// (e.g., `DFE_LOADER__SCALING__MEMORY_GATE_THRESHOLD=0.9`).
///
/// Component weights and saturation points are app-specific -- defined in
/// each app's config and passed to [`super::ScalingPressure::new`] via
/// [`ScalingComponent`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScalingPressureConfig {
    /// Enable scaling pressure calculation.
    /// When disabled, `calculate()` always returns 0.0.
    pub enabled: bool,

    /// Memory usage ratio that triggers the memory gate (0.0-1.0).
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
    /// Relative weight (0.0-1.0). All weights should sum to ~1.0.
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

/// serde default for `PressureExpr::enabled`.
fn default_true() -> bool {
    true
}

/// Configuration for the horizontal scaling-pressure ENGINE (CEL over local
/// metrics).
///
/// Lives under the `scaling` cascade key alongside [`ScalingPressureConfig`];
/// serde ignores each other's extra fields, so both can read the same section.
/// Precedence for the produced pressure: config `pressures` (here) > app-plumbed
/// default > rustlib's context-aware smart default (when `pressures` is empty).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScalingEngineConfig {
    /// Master switch for the CEL pressure engine.
    pub enabled: bool,
    /// Evaluation period in seconds (periodic, off the data hot-path).
    pub interval_secs: u64,
    /// Tunable targets/constants referenced by expressions as `params.<key>`.
    /// Transport-term defaults are filled by `PressureTargets::from_params`;
    /// `cpu_target` defaults to 0.70 (see [`Self::cpu_target`]).
    pub params: BTreeMap<String, f64>,
    /// Named pressure expressions. EMPTY => rustlib composes the context-aware
    /// smart default from the inbound transport kind.
    pub pressures: Vec<PressureExpr>,
    /// Optional explicit inbound/outbound transport kinds (else the runtime
    /// auto-derives them from the transports it builds).
    pub transport: ScalingTransportConfig,
}

impl Default for ScalingEngineConfig {
    fn default() -> Self {
        let mut params = BTreeMap::new();
        params.insert("cpu_target".to_string(), 0.70);
        Self {
            enabled: true,
            interval_secs: 15,
            params,
            pressures: Vec::new(),
            transport: ScalingTransportConfig::default(),
        }
    }
}

impl ScalingEngineConfig {
    /// Load from the config cascade under the `scaling` key.
    ///
    /// Non-registered read (the section is already registered by
    /// [`ScalingPressureConfig::from_cascade`]); falls back to defaults when
    /// config is absent.
    #[must_use]
    pub fn from_cascade() -> Self {
        #[cfg(feature = "config")]
        {
            if let Some(cfg) = crate::config::try_get()
                && let Ok(engine) = cfg.unmarshal_key::<Self>("scaling")
            {
                return engine;
            }
        }
        Self::default()
    }

    /// CPU utilisation target (0-1), defaulting to 0.70 when unset/invalid.
    #[must_use]
    pub fn cpu_target(&self) -> f64 {
        self.params
            .get("cpu_target")
            .copied()
            .filter(|v| *v > 0.0)
            .unwrap_or(0.70)
    }
}

/// A single named pressure expression -> one `{ns}_scaling_pressure{name=...}`
/// gauge. The autoscaler scales to the MAX across all enabled pressures.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PressureExpr {
    /// Output label (`name=...`) on the emitted gauge; must be unique.
    pub name: String,
    /// CEL expression evaluated over the metric/derived/params context.
    pub expression: String,
    /// Whether this pressure is evaluated/emitted.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// Optional explicit transport kinds for the compound pressure (`kafka`,
/// `redis`, `http`, `grpc`, ...). `None` => auto-derived by the runtime.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ScalingTransportConfig {
    /// Inbound transport kind label, or `None` to auto-derive.
    pub inbound: Option<String>,
    /// Outbound transport kind label, or `None` to auto-derive.
    pub outbound: Option<String>,
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
