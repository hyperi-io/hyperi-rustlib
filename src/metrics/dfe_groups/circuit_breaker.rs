// Project:   hyperi-rustlib
// File:      src/metrics/dfe_groups/circuit_breaker.rs
// Purpose:   DFE circuit breaker metrics group
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Circuit breaker metrics.

use super::super::MetricsManager;
use super::super::manifest::{MetricDescriptor, MetricType};

/// Circuit breaker metrics for per-target failure tracking.
///
/// State values: 0=closed (healthy), 1=open (failing), 2=half-open (probing).
#[derive(Clone)]
pub struct CircuitBreakerMetrics {
    namespace: String,
}

impl CircuitBreakerMetrics {
    #[must_use]
    pub fn new(manager: &MetricsManager) -> Self {
        let ns = manager.namespace();

        // circuit_breaker_state — label-based, register descriptor manually
        let state_key = if ns.is_empty() {
            "circuit_breaker_state".to_string()
        } else {
            format!("{ns}_circuit_breaker_state")
        };
        metrics::describe_gauge!(
            state_key.clone(),
            "Circuit breaker state (0=closed, 1=open, 2=half-open)"
        );
        manager.registry().push(MetricDescriptor {
            name: state_key,
            metric_type: MetricType::Gauge,
            description: "Circuit breaker state (0=closed, 1=open, 2=half-open)".into(),
            unit: String::new(),
            labels: vec!["target".into()],
            group: "circuit_breaker".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });

        // circuit_breaker_transitions_total — label-based
        let trans_key = if ns.is_empty() {
            "circuit_breaker_transitions_total".to_string()
        } else {
            format!("{ns}_circuit_breaker_transitions_total")
        };
        metrics::describe_counter!(trans_key.clone(), "Circuit breaker state transitions");
        manager.registry().push(MetricDescriptor {
            name: trans_key,
            metric_type: MetricType::Counter,
            description: "Circuit breaker state transitions".into(),
            unit: String::new(),
            labels: vec!["target".into(), "to_state".into()],
            group: "circuit_breaker".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });

        Self {
            namespace: ns.to_string(),
        }
    }

    /// Set circuit breaker state for a target.
    #[inline]
    pub fn set_state(&self, target: &str, state: u8) {
        let key = if self.namespace.is_empty() {
            "circuit_breaker_state".to_string()
        } else {
            format!("{}_circuit_breaker_state", self.namespace)
        };
        metrics::gauge!(key, "target" => target.to_string()).set(f64::from(state));
    }

    /// Record a state transition.
    #[inline]
    pub fn record_transition(&self, target: &str, to_state: &str) {
        let key = if self.namespace.is_empty() {
            "circuit_breaker_transitions_total".to_string()
        } else {
            format!("{}_circuit_breaker_transitions_total", self.namespace)
        };
        metrics::counter!(
            key,
            "target" => target.to_string(),
            "to_state" => to_state.to_string()
        )
        .increment(1);
    }
}
