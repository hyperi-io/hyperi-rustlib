// Project:   hyperi-rustlib
// File:      src/scaling/pressure.rs
// Purpose:   Lock-free scaling pressure calculator
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Lock-free scaling pressure calculator for KEDA autoscaling.
//!
//! Apps register components at construction, then update values atomically
//! from their pipeline. [`ScalingPressure::calculate`] returns 0.0–100.0.
//!
//! ## Gate Logic
//!
//! Two hard gates are evaluated before the weighted composite:
//!
//! 1. **Circuit breaker** — if any circuit is open, returns 0.0
//!    (sink is down, scaling won't help)
//! 2. **Memory pressure** — if `used/limit >= threshold`, returns 100.0
//!    (scale immediately before OOM)
//!
//! If neither gate fires, the weighted composite is calculated.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::config::{ScalingComponent, ScalingPressureConfig};

/// Active gate preventing normal composite calculation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateType {
    /// Circuit breaker open → 0.0 (scaling won't help).
    CircuitBreaker,
    /// Memory pressure high → 100.0 (scale before OOM).
    MemoryPressure,
}

impl std::fmt::Display for GateType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GateType::CircuitBreaker => write!(f, "circuit_breaker"),
            GateType::MemoryPressure => write!(f, "memory_pressure"),
        }
    }
}

/// Per-component diagnostic snapshot.
#[derive(Debug, Clone)]
pub struct ComponentSnapshot {
    /// Component name.
    pub name: String,
    /// Current raw value.
    pub raw_value: f64,
    /// Score contribution (0.0–weight*100.0).
    pub score: f64,
    /// Configured weight.
    pub weight: f64,
    /// Configured saturation point.
    pub saturation: f64,
}

/// Full diagnostic snapshot of scaling pressure state.
#[derive(Debug, Clone)]
pub struct PressureSnapshot {
    /// Calculated scaling pressure (0.0–100.0).
    pub value: f64,
    /// Active gate, if any.
    pub gate_active: Option<GateType>,
    /// Per-component breakdown.
    pub components: Vec<ComponentSnapshot>,
    /// Current memory usage ratio (0.0–1.0).
    pub memory_ratio: f64,
    /// Whether the circuit breaker is signalled open.
    pub circuit_open: bool,
}

/// Internal entry for a registered component.
struct ComponentEntry {
    name: String,
    weight: f64,
    saturation: f64,
    /// Current value stored as f64 bits in AtomicU64.
    value: AtomicU64,
}

/// Lock-free scaling pressure calculator.
///
/// Produces a 0.0–100.0 composite metric for KEDA (or any autoscaler)
/// based on weighted application signals with two hard gates.
///
/// All updates are lock-free (`Relaxed` ordering) — safe to call from
/// any thread without contention.
///
/// # Example
///
/// ```rust
/// use hyperi_rustlib::scaling::{ScalingPressure, ScalingPressureConfig, ScalingComponent};
///
/// let pressure = ScalingPressure::new(
///     ScalingPressureConfig::default(),
///     vec![
///         ScalingComponent::new("kafka_lag", 0.50, 100_000.0),
///         ScalingComponent::new("memory", 0.50, 1.0),
///     ],
/// );
///
/// pressure.set_component("kafka_lag", 50_000.0);
/// pressure.set_memory(400_000_000, 1_000_000_000);
///
/// let value = pressure.calculate();
/// assert!(value > 0.0 && value < 100.0);
/// ```
pub struct ScalingPressure {
    enabled: bool,
    memory_gate_threshold: f64,
    components: Vec<ComponentEntry>,
    circuit_open: AtomicBool,
    memory_used: AtomicU64,
    memory_limit: AtomicU64,
}

impl ScalingPressure {
    /// Create a new scaling pressure calculator.
    ///
    /// Components define the weighted signals. Their order doesn't matter.
    #[must_use]
    pub fn new(config: ScalingPressureConfig, components: Vec<ScalingComponent>) -> Self {
        let entries = components
            .into_iter()
            .map(|c| ComponentEntry {
                name: c.name,
                weight: c.weight,
                saturation: c.saturation,
                value: AtomicU64::new(0_f64.to_bits()),
            })
            .collect();

        Self {
            enabled: config.enabled,
            memory_gate_threshold: config.memory_gate_threshold,
            components: entries,
            circuit_open: AtomicBool::new(false),
            memory_used: AtomicU64::new(0),
            memory_limit: AtomicU64::new(0),
        }
    }

    /// Set a component's current value (lock-free).
    ///
    /// If `name` doesn't match any registered component, this is a no-op.
    pub fn set_component(&self, name: &str, value: f64) {
        for entry in &self.components {
            if entry.name == name {
                entry.value.store(value.to_bits(), Ordering::Relaxed);
                return;
            }
        }
    }

    /// Signal whether the circuit breaker is open.
    ///
    /// When open, `calculate()` returns 0.0 (scaling won't help when the
    /// downstream sink is unavailable).
    pub fn set_circuit_open(&self, open: bool) {
        self.circuit_open.store(open, Ordering::Relaxed);
    }

    /// Update memory usage for the memory gate.
    ///
    /// When `used_bytes / limit_bytes >= memory_gate_threshold`,
    /// `calculate()` returns 100.0 to trigger immediate scale-up.
    pub fn set_memory(&self, used_bytes: u64, limit_bytes: u64) {
        self.memory_used.store(used_bytes, Ordering::Relaxed);
        self.memory_limit.store(limit_bytes, Ordering::Relaxed);
    }

    /// Calculate composite scaling pressure (0.0–100.0).
    ///
    /// Gate logic:
    /// 1. Disabled → 0.0
    /// 2. Circuit breaker open → 0.0
    /// 3. Memory pressure ≥ threshold → 100.0
    /// 4. Otherwise → weighted composite capped at 100.0
    #[must_use]
    pub fn calculate(&self) -> f64 {
        if !self.enabled {
            return 0.0;
        }

        // Gate 1: Circuit breaker open — sink is down, scaling won't help
        if self.circuit_open.load(Ordering::Relaxed) {
            return 0.0;
        }

        // Compute memory ratio
        let memory_used = self.memory_used.load(Ordering::Relaxed) as f64;
        let memory_limit = self.memory_limit.load(Ordering::Relaxed) as f64;
        let memory_ratio = if memory_limit > 0.0 {
            memory_used / memory_limit
        } else {
            0.0
        };

        // Gate 2: High memory pressure — scale immediately before OOM
        if memory_ratio >= self.memory_gate_threshold {
            return 100.0;
        }

        // Weighted composite
        let mut total = 0.0_f64;
        for entry in &self.components {
            let value = f64::from_bits(entry.value.load(Ordering::Relaxed));
            let score = if entry.saturation > 0.0 {
                (value / entry.saturation).min(1.0) * entry.weight * 100.0
            } else {
                0.0
            };
            total += score;
        }

        total.min(100.0)
    }

    /// Diagnostic snapshot with per-component breakdown.
    #[must_use]
    pub fn snapshot(&self) -> PressureSnapshot {
        let circuit_open = self.circuit_open.load(Ordering::Relaxed);

        let memory_used = self.memory_used.load(Ordering::Relaxed) as f64;
        let memory_limit = self.memory_limit.load(Ordering::Relaxed) as f64;
        let memory_ratio = if memory_limit > 0.0 {
            memory_used / memory_limit
        } else {
            0.0
        };

        // Determine active gate
        let gate_active = if !self.enabled {
            None
        } else if circuit_open {
            Some(GateType::CircuitBreaker)
        } else if memory_ratio >= self.memory_gate_threshold {
            Some(GateType::MemoryPressure)
        } else {
            None
        };

        let components: Vec<ComponentSnapshot> = self
            .components
            .iter()
            .map(|entry| {
                let raw_value = f64::from_bits(entry.value.load(Ordering::Relaxed));
                let score = if entry.saturation > 0.0 {
                    (raw_value / entry.saturation).min(1.0) * entry.weight * 100.0
                } else {
                    0.0
                };
                ComponentSnapshot {
                    name: entry.name.clone(),
                    raw_value,
                    score,
                    weight: entry.weight,
                    saturation: entry.saturation,
                }
            })
            .collect();

        PressureSnapshot {
            value: self.calculate(),
            gate_active,
            components,
            memory_ratio,
            circuit_open,
        }
    }

    /// Whether the engine is enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scaling::ScalingComponent;

    fn test_components() -> Vec<ScalingComponent> {
        vec![
            ScalingComponent::new("kafka_lag", 0.35, 100_000.0),
            ScalingComponent::new("buffer_depth", 0.25, 10_000.0),
            ScalingComponent::new("insert_latency", 0.15, 5.0),
            ScalingComponent::new("memory", 0.15, 1.0),
            ScalingComponent::new("errors", 0.10, 100.0),
        ]
    }

    fn test_pressure() -> ScalingPressure {
        ScalingPressure::new(ScalingPressureConfig::default(), test_components())
    }

    #[test]
    fn test_zero_load() {
        let p = test_pressure();
        let value = p.calculate();
        assert!(
            value.abs() < f64::EPSILON,
            "Zero load should produce 0.0, got {value}"
        );
    }

    #[test]
    fn test_single_component_at_saturation() {
        let p = test_pressure();
        // kafka_lag at saturation (100,000) → contributes weight * 100 = 35.0
        p.set_component("kafka_lag", 100_000.0);
        let value = p.calculate();
        assert!(
            (value - 35.0).abs() < 0.01,
            "kafka_lag at saturation should contribute 35.0, got {value}"
        );
    }

    #[test]
    fn test_single_component_half_saturation() {
        let p = test_pressure();
        // kafka_lag at 50% saturation → contributes 0.5 * 0.35 * 100 = 17.5
        p.set_component("kafka_lag", 50_000.0);
        let value = p.calculate();
        assert!(
            (value - 17.5).abs() < 0.01,
            "kafka_lag at half saturation should contribute 17.5, got {value}"
        );
    }

    #[test]
    fn test_all_components_saturated() {
        let p = test_pressure();
        p.set_component("kafka_lag", 200_000.0); // Over saturation
        p.set_component("buffer_depth", 20_000.0);
        p.set_component("insert_latency", 10.0);
        p.set_component("memory", 2.0);
        p.set_component("errors", 200.0);
        let value = p.calculate();
        assert!(
            (value - 100.0).abs() < 0.01,
            "All saturated should produce 100.0, got {value}"
        );
    }

    #[test]
    fn test_capped_at_100() {
        let p = test_pressure();
        // Way over saturation for all components
        p.set_component("kafka_lag", 1_000_000.0);
        p.set_component("buffer_depth", 1_000_000.0);
        p.set_component("insert_latency", 1_000.0);
        p.set_component("memory", 100.0);
        p.set_component("errors", 100_000.0);
        let value = p.calculate();
        assert!(
            (value - 100.0).abs() < f64::EPSILON,
            "Should be capped at 100.0, got {value}"
        );
    }

    #[test]
    fn test_circuit_breaker_gate() {
        let p = test_pressure();
        p.set_component("kafka_lag", 100_000.0);
        p.set_circuit_open(true);
        let value = p.calculate();
        assert!(
            value.abs() < f64::EPSILON,
            "Circuit breaker open should produce 0.0, got {value}"
        );
    }

    #[test]
    fn test_memory_gate() {
        let p = test_pressure();
        // 80% memory = exactly at threshold
        p.set_memory(800, 1000);
        let value = p.calculate();
        assert!(
            (value - 100.0).abs() < f64::EPSILON,
            "Memory at threshold should produce 100.0, got {value}"
        );
    }

    #[test]
    fn test_memory_gate_above_threshold() {
        let p = test_pressure();
        p.set_memory(900, 1000);
        let value = p.calculate();
        assert!(
            (value - 100.0).abs() < f64::EPSILON,
            "Memory above threshold should produce 100.0, got {value}"
        );
    }

    #[test]
    fn test_memory_below_threshold_uses_composite() {
        let p = test_pressure();
        // 70% memory — below 0.8 threshold
        p.set_memory(700, 1000);
        p.set_component("kafka_lag", 50_000.0);
        let value = p.calculate();
        // Should be composite, not 100.0
        assert!(
            value > 0.0 && value < 100.0,
            "Memory below threshold should use composite, got {value}"
        );
    }

    #[test]
    fn test_memory_gate_takes_precedence_over_circuit_breaker() {
        // Memory gate (100.0) fires. Circuit breaker (0.0) is checked first.
        // Circuit breaker is checked before memory — so CB wins.
        let p = test_pressure();
        p.set_memory(900, 1000);
        p.set_circuit_open(true);
        let value = p.calculate();
        assert!(
            value.abs() < f64::EPSILON,
            "Circuit breaker should take precedence, got {value}"
        );
    }

    #[test]
    fn test_disabled() {
        let config = ScalingPressureConfig {
            enabled: false,
            ..Default::default()
        };
        let p = ScalingPressure::new(config, test_components());
        p.set_component("kafka_lag", 100_000.0);
        p.set_memory(900, 1000);
        let value = p.calculate();
        assert!(
            value.abs() < f64::EPSILON,
            "Disabled should produce 0.0, got {value}"
        );
    }

    #[test]
    fn test_unknown_component_is_noop() {
        let p = test_pressure();
        // Should not panic or affect result
        p.set_component("nonexistent", 999.0);
        let value = p.calculate();
        assert!(
            value.abs() < f64::EPSILON,
            "Unknown component should not affect result, got {value}"
        );
    }

    #[test]
    fn test_zero_memory_limit() {
        let p = test_pressure();
        // Zero limit should not trigger memory gate (avoid div by zero)
        p.set_memory(100, 0);
        p.set_component("kafka_lag", 50_000.0);
        let value = p.calculate();
        assert!(
            value > 0.0,
            "Zero memory limit should not trigger gate, got {value}"
        );
    }

    #[test]
    fn test_zero_saturation_component() {
        let p = ScalingPressure::new(
            ScalingPressureConfig::default(),
            vec![ScalingComponent::new("broken", 0.50, 0.0)],
        );
        p.set_component("broken", 100.0);
        let value = p.calculate();
        assert!(
            value.abs() < f64::EPSILON,
            "Zero saturation component should contribute 0.0, got {value}"
        );
    }

    #[test]
    fn test_snapshot() {
        let p = test_pressure();
        p.set_component("kafka_lag", 50_000.0);
        p.set_component("buffer_depth", 5_000.0);
        p.set_memory(500, 1000);

        let snap = p.snapshot();
        assert!(!snap.circuit_open);
        assert!(snap.gate_active.is_none());
        assert!((snap.memory_ratio - 0.5).abs() < f64::EPSILON);
        assert_eq!(snap.components.len(), 5);
        assert!(snap.value > 0.0);

        // Check kafka_lag component
        let lag = snap
            .components
            .iter()
            .find(|c| c.name == "kafka_lag")
            .unwrap();
        assert!((lag.raw_value - 50_000.0).abs() < f64::EPSILON);
        assert!((lag.score - 17.5).abs() < 0.01);
    }

    #[test]
    fn test_snapshot_with_gate() {
        let p = test_pressure();
        p.set_circuit_open(true);

        let snap = p.snapshot();
        assert!(snap.circuit_open);
        assert_eq!(snap.gate_active, Some(GateType::CircuitBreaker));
        assert!(snap.value.abs() < f64::EPSILON);
    }

    #[test]
    fn test_is_enabled() {
        let p = test_pressure();
        assert!(p.is_enabled());

        let disabled = ScalingPressure::new(
            ScalingPressureConfig {
                enabled: false,
                ..Default::default()
            },
            vec![],
        );
        assert!(!disabled.is_enabled());
    }

    #[test]
    fn test_mixed_load() {
        let p = test_pressure();
        // Realistic mixed load scenario
        p.set_component("kafka_lag", 20_000.0); // 20% of saturation
        p.set_component("buffer_depth", 3_000.0); // 30% of saturation
        p.set_component("insert_latency", 1.0); // 20% of saturation
        p.set_component("memory", 0.4); // 40% of saturation
        p.set_component("errors", 5.0); // 5% of saturation

        let value = p.calculate();
        // Expected: 0.2*35 + 0.3*25 + 0.2*15 + 0.4*15 + 0.05*10
        // = 7.0 + 7.5 + 3.0 + 6.0 + 0.5 = 24.0
        assert!(
            (value - 24.0).abs() < 0.01,
            "Mixed load should produce ~24.0, got {value}"
        );
    }
}
