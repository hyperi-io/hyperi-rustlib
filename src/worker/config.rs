// Project:   hyperi-rustlib
// File:      src/worker/config.rs
// Purpose:   Configuration for adaptive worker pool
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use serde::{Deserialize, Serialize};

/// Configuration for the adaptive worker pool.
///
/// All values are overridable via the 8-layer config cascade
/// (CLI > ENV > .env > settings.{env}.yaml > settings.yaml > defaults > rustlib > hard-coded).
///
/// Every field is also emitted as a gauge metric for Grafana overlay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerPoolConfig {
    /// Minimum active worker threads (floor for scaling).
    #[serde(default = "default_min_threads")]
    pub min_threads: usize,

    /// Maximum worker threads. 0 = auto-detect from cgroup / `available_parallelism`.
    #[serde(default)]
    pub max_threads: usize,

    /// CPU utilisation below this threshold triggers thread growth.
    #[serde(default = "default_grow_below")]
    pub grow_below: f64,

    /// CPU utilisation above this threshold triggers gentle thread reduction.
    #[serde(default = "default_shrink_above")]
    pub shrink_above: f64,

    /// CPU utilisation above this threshold triggers aggressive thread reduction.
    #[serde(default = "default_emergency_above")]
    pub emergency_above: f64,

    /// Memory pressure above this threshold hard-caps threads at `min_threads`.
    #[serde(default = "default_memory_pressure_cap")]
    pub memory_pressure_cap: f64,

    /// How often to re-evaluate scaling (seconds).
    #[serde(default = "default_scale_interval_secs")]
    pub scale_interval_secs: u64,

    /// Maximum concurrent async fan-out tasks.
    #[serde(default = "default_async_concurrency")]
    pub async_concurrency: usize,

    /// Seconds the pool must be saturated before reporting unhealthy.
    #[serde(default = "default_health_saturation_timeout_secs")]
    pub health_saturation_timeout_secs: u64,
}

fn default_min_threads() -> usize {
    2
}
fn default_grow_below() -> f64 {
    0.60
}
fn default_shrink_above() -> f64 {
    0.85
}
fn default_emergency_above() -> f64 {
    0.95
}
fn default_memory_pressure_cap() -> f64 {
    0.80
}
fn default_scale_interval_secs() -> u64 {
    5
}
fn default_async_concurrency() -> usize {
    32
}
fn default_health_saturation_timeout_secs() -> u64 {
    30
}

impl Default for WorkerPoolConfig {
    fn default() -> Self {
        Self {
            min_threads: default_min_threads(),
            max_threads: 0,
            grow_below: default_grow_below(),
            shrink_above: default_shrink_above(),
            emergency_above: default_emergency_above(),
            memory_pressure_cap: default_memory_pressure_cap(),
            scale_interval_secs: default_scale_interval_secs(),
            async_concurrency: default_async_concurrency(),
            health_saturation_timeout_secs: default_health_saturation_timeout_secs(),
        }
    }
}

impl WorkerPoolConfig {
    /// Load config from the cascade under the given key (e.g. "worker_pool").
    ///
    /// Falls back to defaults if the config cascade is not initialised or the
    /// key is absent. Validates after loading.
    ///
    /// # Errors
    ///
    /// Returns an error if validation fails (e.g. thresholds out of order).
    pub fn from_cascade(key: &str) -> Result<Self, crate::config::ConfigError> {
        let pool_cfg: Self = match crate::config::try_get() {
            Some(cfg) => cfg.unmarshal_key(key).unwrap_or_default(),
            None => {
                tracing::debug!("Config cascade not initialised, using default WorkerPoolConfig");
                Self::default()
            }
        };
        pool_cfg.validate()?;
        Ok(pool_cfg)
    }

    /// Validate configuration invariants.
    ///
    /// # Errors
    ///
    /// Returns an error if thresholds are out of order or min > max.
    pub fn validate(&self) -> Result<(), crate::config::ConfigError> {
        if self.max_threads != 0 && self.min_threads > self.max_threads {
            return Err(crate::config::ConfigError::InvalidValue {
                key: "worker_pool.min_threads".into(),
                reason: format!(
                    "min_threads ({}) > max_threads ({})",
                    self.min_threads, self.max_threads
                ),
            });
        }
        if self.grow_below >= self.shrink_above {
            return Err(crate::config::ConfigError::InvalidValue {
                key: "worker_pool.grow_below".into(),
                reason: format!(
                    "grow_below ({}) >= shrink_above ({})",
                    self.grow_below, self.shrink_above
                ),
            });
        }
        if self.shrink_above >= self.emergency_above {
            return Err(crate::config::ConfigError::InvalidValue {
                key: "worker_pool.shrink_above".into(),
                reason: format!(
                    "shrink_above ({}) >= emergency_above ({})",
                    self.shrink_above, self.emergency_above
                ),
            });
        }
        Ok(())
    }

    /// Resolve `max_threads` to the effective CPU count.
    ///
    /// - `max_threads = 0` → auto-detect from `available_parallelism` (cgroup-aware)
    /// - `max_threads > 0` → cap at `min(configured, available_parallelism)`
    ///   to avoid creating more threads than physical cores
    pub fn resolve_max_threads(&mut self) {
        let available = std::thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(4);

        if self.max_threads == 0 {
            self.max_threads = available;
        } else {
            self.max_threads = self.max_threads.min(available);
        }
    }
}
