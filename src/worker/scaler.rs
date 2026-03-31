// Project:   hyperi-rustlib
// File:      src/worker/scaler.rs
// Purpose:   Scaling controller loop, watermark algorithm, CPU sampling
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use std::sync::Arc;
use std::time::Duration;

use sysinfo::System;
use tokio_util::sync::CancellationToken;

use super::pool::AdaptiveWorkerPool;

/// Result of the watermark scaling algorithm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalingDecision {
    /// Target thread count after applying watermark bands.
    pub target: usize,
    /// Direction of change: "up", "down", "emergency_down", "memory_cap", or "steady".
    pub direction: &'static str,
}

impl ScalingDecision {
    /// Evaluate the watermark scaling algorithm.
    ///
    /// Given current CPU utilisation, memory pressure, and thresholds, returns
    /// the target thread count and the direction of the scaling change.
    #[must_use]
    pub fn evaluate(
        cpu_util: f64,
        memory_pressure: f64,
        current: usize,
        min_threads: usize,
        max_threads: usize,
        grow_below: f64,
        shrink_above: f64,
        emergency_above: f64,
        memory_pressure_cap: f64,
    ) -> Self {
        // Memory pressure overrides everything — prevent OOM
        if memory_pressure > memory_pressure_cap {
            return Self {
                target: min_threads,
                direction: "memory_cap",
            };
        }

        let (raw_target, direction) = if cpu_util < grow_below {
            (current.saturating_add(2), "up")
        } else if cpu_util <= shrink_above {
            (current, "steady")
        } else if cpu_util <= emergency_above {
            (current.saturating_sub(1), "down")
        } else {
            (current.saturating_sub(2), "emergency_down")
        };

        // Clamp to [min, max]
        let target = raw_target.clamp(min_threads, max_threads);

        Self { target, direction }
    }
}

/// Background scaling controller.
///
/// Samples CPU and memory every `scale_interval_secs`, applies the watermark
/// algorithm, and adjusts the semaphore permits on the worker pool.
pub(crate) struct ScalingController {
    pool: Arc<AdaptiveWorkerPool>,
    system: System,
}

impl ScalingController {
    pub fn new(pool: Arc<AdaptiveWorkerPool>) -> Self {
        Self {
            pool,
            system: System::new(),
        }
    }

    /// Run the scaling loop until cancelled.
    pub async fn run(mut self, cancel: CancellationToken) {
        let interval_secs = {
            let cfg = self.pool.config.read();
            cfg.scale_interval_secs
        };

        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));

        loop {
            tokio::select! {
                () = cancel.cancelled() => {
                    tracing::info!("Worker pool scaling controller shutting down");
                    break;
                }
                _ = interval.tick() => {
                    self.tick();
                }
            }
        }
    }

    fn tick(&mut self) {
        // Sample CPU
        self.system.refresh_cpu_all();
        let cpu_util = f64::from(self.system.global_cpu_usage()) / 100.0;

        // Sample memory — dual source: sysinfo process RSS + MemoryGuard if attached
        self.system.refresh_memory();
        let sysinfo_mem_pressure = if self.system.total_memory() > 0 {
            self.system.used_memory() as f64 / self.system.total_memory() as f64
        } else {
            0.0
        };

        #[cfg(feature = "memory")]
        let memory_guard_pressure = self
            .pool
            .memory_guard
            .lock()
            .as_ref()
            .map(|g| g.pressure_ratio())
            .unwrap_or(0.0);
        #[cfg(not(feature = "memory"))]
        let memory_guard_pressure = 0.0;

        let effective_memory_pressure = sysinfo_mem_pressure.max(memory_guard_pressure);

        // Read config (may have been hot-reloaded via Arc<RwLock<>>)
        let cfg = self.pool.config.read().clone();

        let current_permits = self.pool.active_threads();

        let decision = ScalingDecision::evaluate(
            cpu_util,
            effective_memory_pressure,
            current_permits,
            cfg.min_threads,
            cfg.max_threads,
            cfg.grow_below,
            cfg.shrink_above,
            cfg.emergency_above,
            cfg.memory_pressure_cap,
        );

        if decision.direction != "steady" {
            tracing::info!(
                cpu = format!("{cpu_util:.2}"),
                mem = format!("{effective_memory_pressure:.2}"),
                current = current_permits,
                target = decision.target,
                direction = decision.direction,
                "Worker pool scaling"
            );
            metrics::counter!("worker_pool_scale_events_total", "direction" => decision.direction)
                .increment(1);
        } else {
            tracing::debug!(
                cpu = format!("{cpu_util:.2}"),
                current = current_permits,
                "Worker pool steady"
            );
        }

        // Adjust semaphore permits
        self.pool.semaphore.set_permits(decision.target);

        // Emit operational metrics
        metrics::gauge!("worker_pool_active_threads").set(decision.target as f64);
        metrics::gauge!("worker_pool_target_threads").set(decision.target as f64);
        metrics::gauge!("worker_pool_cpu_utilisation").set(cpu_util);
        metrics::gauge!("worker_pool_memory_utilisation").set(effective_memory_pressure);
        metrics::gauge!("worker_pool_saturation")
            .set(decision.target as f64 / cfg.max_threads.max(1) as f64);

        // Feed back into ScalingPressure if attached
        #[cfg(feature = "scaling")]
        if let Some(sp) = self.pool.scaling_pressure.lock().as_ref() {
            let saturation = decision.target as f64 / cfg.max_threads.max(1) as f64;
            sp.set_component("worker_pool_saturation", saturation);
        }
    }
}
