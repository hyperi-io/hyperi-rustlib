// Project:   hyperi-rustlib
// File:      src/health/registry.rs
// Purpose:   Global health registry singleton for component health tracking
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Global health registry for unified service health state.
//!
//! Modules register health check callbacks at construction. The registry
//! aggregates component status to determine overall service health.
//!
//! # Design
//!
//! - Global singleton via `OnceLock` (consistent with config registry pattern)
//! - Components register a closure that returns their current [`HealthStatus`]
//! - [`is_healthy`](HealthRegistry::is_healthy) requires ALL components healthy
//! - [`is_ready`](HealthRegistry::is_ready) requires NO components unhealthy
//!   (degraded is acceptable for readiness)
//! - Empty registry is considered healthy (vacuously true)

use std::sync::{Arc, Mutex, OnceLock};

/// Health status of a registered component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    /// Component is fully operational.
    Healthy,
    /// Component is operational but impaired (e.g., circuit half-open,
    /// elevated latency, fallback active).
    Degraded,
    /// Component is not operational. Service should not receive traffic.
    Unhealthy,
}

impl HealthStatus {
    /// String representation for JSON serialisation and endpoint output.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unhealthy => "unhealthy",
        }
    }
}

/// Health check callback — returns current component status.
type HealthCheck = Arc<dyn Fn() -> HealthStatus + Send + Sync>;

/// A registered health check entry.
struct HealthEntry {
    name: String,
    check: HealthCheck,
}

/// Global health registry singleton.
///
/// Modules register health check callbacks at construction. The registry
/// aggregates all component statuses to determine overall service health.
///
/// # Thread Safety
///
/// The registry uses `Mutex<Vec<_>>` for registration (infrequent, at
/// init time) and read access (health checks). For the typical DFE app
/// with 3-8 registered components, lock contention is negligible.
pub struct HealthRegistry {
    components: Mutex<Vec<HealthEntry>>,
}

/// Global singleton instance.
static REGISTRY: OnceLock<HealthRegistry> = OnceLock::new();

impl HealthRegistry {
    /// Create a new empty registry.
    fn new() -> Self {
        Self {
            components: Mutex::new(Vec::new()),
        }
    }

    /// Get or initialise the global registry.
    fn global() -> &'static Self {
        REGISTRY.get_or_init(Self::new)
    }

    /// Register a health check callback.
    ///
    /// Called by modules at construction time. The callback is invoked
    /// each time health is queried, so it should be cheap (e.g., read
    /// an `AtomicBool` or check a cached state).
    ///
    /// # Duplicate Names
    ///
    /// Multiple components may register with the same name. Each
    /// registration is independent — the registry does not deduplicate.
    pub fn register(
        name: impl Into<String>,
        check: impl Fn() -> HealthStatus + Send + Sync + 'static,
    ) {
        let registry = Self::global();
        if let Ok(mut components) = registry.components.lock() {
            components.push(HealthEntry {
                name: name.into(),
                check: Arc::new(check),
            });
        }
    }

    /// Check if ALL components are healthy.
    ///
    /// Returns `true` if the registry is empty (vacuously true) or
    /// every registered component reports [`HealthStatus::Healthy`].
    #[must_use]
    pub fn is_healthy() -> bool {
        let registry = Self::global();
        let Ok(components) = registry.components.lock() else {
            return false;
        };
        components
            .iter()
            .all(|c| (c.check)() == HealthStatus::Healthy)
    }

    /// Check if the service is ready to receive traffic.
    ///
    /// Ready means no component is [`HealthStatus::Unhealthy`]. Degraded
    /// components are acceptable — the service can still serve requests,
    /// just with reduced capability.
    ///
    /// Returns `true` if the registry is empty (vacuously true).
    #[must_use]
    pub fn is_ready() -> bool {
        let registry = Self::global();
        let Ok(components) = registry.components.lock() else {
            return false;
        };
        components
            .iter()
            .all(|c| (c.check)() != HealthStatus::Unhealthy)
    }

    /// Get per-component health status.
    ///
    /// Returns a snapshot of all registered components and their current
    /// status. Useful for detailed health endpoints.
    #[must_use]
    pub fn components() -> Vec<(String, HealthStatus)> {
        let registry = Self::global();
        let Ok(components) = registry.components.lock() else {
            return Vec::new();
        };
        components
            .iter()
            .map(|c| (c.name.clone(), (c.check)()))
            .collect()
    }

    /// Get a JSON representation of the health state.
    ///
    /// Suitable for a `/health/detailed` endpoint response.
    #[cfg(feature = "serde_json")]
    #[must_use]
    pub fn to_json() -> serde_json::Value {
        let components = Self::components();
        let overall = if Self::is_healthy() {
            "healthy"
        } else if Self::is_ready() {
            "degraded"
        } else {
            "unhealthy"
        };

        serde_json::json!({
            "status": overall,
            "components": components.iter().map(|(name, status)| {
                serde_json::json!({
                    "name": name,
                    "status": status.as_str(),
                })
            }).collect::<Vec<_>>()
        })
    }

    /// Clear all registered components (for testing only).
    #[cfg(test)]
    pub(crate) fn reset() {
        let registry = Self::global();
        if let Ok(mut components) = registry.components.lock() {
            components.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU8, Ordering};

    use super::*;

    /// Tests share global statics — serialise them.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    macro_rules! serial_test {
        () => {
            let _guard = TEST_LOCK.lock().unwrap();
            HealthRegistry::reset();
        };
    }

    #[test]
    fn empty_registry_is_healthy() {
        serial_test!();

        assert!(HealthRegistry::is_healthy());
        assert!(HealthRegistry::is_ready());
        assert!(HealthRegistry::components().is_empty());
    }

    #[test]
    fn register_and_check_healthy() {
        serial_test!();

        HealthRegistry::register("transport", || HealthStatus::Healthy);
        HealthRegistry::register("database", || HealthStatus::Healthy);

        assert!(HealthRegistry::is_healthy());
        assert!(HealthRegistry::is_ready());

        let components = HealthRegistry::components();
        assert_eq!(components.len(), 2);
        assert_eq!(components[0].0, "transport");
        assert_eq!(components[0].1, HealthStatus::Healthy);
        assert_eq!(components[1].0, "database");
        assert_eq!(components[1].1, HealthStatus::Healthy);
    }

    #[test]
    fn unhealthy_component_fails_check() {
        serial_test!();

        HealthRegistry::register("transport", || HealthStatus::Healthy);
        HealthRegistry::register("database", || HealthStatus::Unhealthy);

        assert!(!HealthRegistry::is_healthy());
        assert!(!HealthRegistry::is_ready());
    }

    #[test]
    fn degraded_is_ready_but_not_healthy() {
        serial_test!();

        HealthRegistry::register("transport", || HealthStatus::Healthy);
        HealthRegistry::register("circuit_breaker", || HealthStatus::Degraded);

        assert!(!HealthRegistry::is_healthy());
        assert!(HealthRegistry::is_ready());
    }

    #[test]
    fn dynamic_health_check_reflects_state_changes() {
        serial_test!();

        // Simulate a component whose health changes at runtime
        let state = Arc::new(AtomicU8::new(0)); // 0=healthy, 1=degraded, 2=unhealthy
        let state_clone = state.clone();

        HealthRegistry::register("dynamic", move || {
            match state_clone.load(Ordering::Relaxed) {
                0 => HealthStatus::Healthy,
                1 => HealthStatus::Degraded,
                _ => HealthStatus::Unhealthy,
            }
        });

        // Initially healthy
        assert!(HealthRegistry::is_healthy());
        assert!(HealthRegistry::is_ready());

        // Transition to degraded
        state.store(1, Ordering::Relaxed);
        assert!(!HealthRegistry::is_healthy());
        assert!(HealthRegistry::is_ready());

        // Transition to unhealthy
        state.store(2, Ordering::Relaxed);
        assert!(!HealthRegistry::is_healthy());
        assert!(!HealthRegistry::is_ready());

        // Recovery back to healthy
        state.store(0, Ordering::Relaxed);
        assert!(HealthRegistry::is_healthy());
        assert!(HealthRegistry::is_ready());
    }

    #[test]
    fn health_status_as_str() {
        assert_eq!(HealthStatus::Healthy.as_str(), "healthy");
        assert_eq!(HealthStatus::Degraded.as_str(), "degraded");
        assert_eq!(HealthStatus::Unhealthy.as_str(), "unhealthy");
    }

    #[test]
    #[cfg(feature = "serde_json")]
    fn to_json_includes_all_components() {
        serial_test!();

        HealthRegistry::register("kafka", || HealthStatus::Healthy);
        HealthRegistry::register("clickhouse", || HealthStatus::Degraded);

        let json = HealthRegistry::to_json();

        assert_eq!(json["status"], "degraded");

        let components = json["components"].as_array().unwrap();
        assert_eq!(components.len(), 2);

        assert_eq!(components[0]["name"], "kafka");
        assert_eq!(components[0]["status"], "healthy");

        assert_eq!(components[1]["name"], "clickhouse");
        assert_eq!(components[1]["status"], "degraded");
    }

    #[test]
    #[cfg(feature = "serde_json")]
    fn to_json_empty_registry() {
        serial_test!();

        let json = HealthRegistry::to_json();
        assert_eq!(json["status"], "healthy");
        assert!(json["components"].as_array().unwrap().is_empty());
    }

    #[test]
    #[cfg(feature = "serde_json")]
    fn to_json_unhealthy_status() {
        serial_test!();

        HealthRegistry::register("broken", || HealthStatus::Unhealthy);

        let json = HealthRegistry::to_json();
        assert_eq!(json["status"], "unhealthy");
    }
}
