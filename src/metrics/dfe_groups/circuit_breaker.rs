// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED

//! Circuit breaker metrics.

use super::super::MetricsManager;

/// Circuit breaker metrics for per-target failure tracking.
///
/// State values: 0=closed (healthy), 1=open (failing), 2=half-open (probing).
#[derive(Clone)]
pub struct CircuitBreakerMetrics {
    namespace: String,
}

impl CircuitBreakerMetrics {
    pub fn new(manager: &MetricsManager) -> Self {
        let ns = manager.namespace();
        let state_key = if ns.is_empty() {
            "circuit_breaker_state".to_string()
        } else {
            format!("{ns}_circuit_breaker_state")
        };
        metrics::describe_gauge!(
            state_key,
            "Circuit breaker state (0=closed, 1=open, 2=half-open)"
        );

        let trans_key = if ns.is_empty() {
            "circuit_breaker_transitions_total".to_string()
        } else {
            format!("{ns}_circuit_breaker_transitions_total")
        };
        metrics::describe_counter!(trans_key, "Circuit breaker state transitions");

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
