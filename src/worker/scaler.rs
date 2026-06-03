// Project:   hyperi-rustlib
// File:      src/worker/scaler.rs
// Purpose:   Scaling controller loop, watermark algorithm, CPU sampling
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # Threading model and the CPU vs memory asymmetry
//!
//! **Foundational assumption: Tokio is HyperI's async + multithreading
//! substrate.** We do not build our own runtime or our own cgroup-aware
//! thread-count plumbing -- we lean on the existing wheels:
//!
//! - **Sizing** of both the Tokio runtime (`worker_threads`) and the rayon
//!   [`AdaptiveWorkerPool`] ceiling derives from
//!   [`std::thread::available_parallelism`], which on Linux is **cgroup-aware**
//!   (`cpu.max` / `cpuset`, Rust 1.74+). So both pools auto-size to the
//!   container's CPU budget at startup with no bespoke detection.
//! - **CPU throttling** is the kernel CFS scheduler's job (`cpu.max`). We do
//!   not, and need not, replicate it.
//!
//! ## Why there is no cgroup-CPU backpressure signal (but there IS for memory)
//!
//! CPU and memory are not symmetric, so they are not plumbed symmetrically:
//!
//! | Resource | Over-use outcome | Self-correcting? | Dynamic backpressure |
//! |----------|------------------|------------------|----------------------|
//! | Memory   | OOM-kill         | No -- fatal      | Yes -- the `MemoryGuard` + cgroup-first pressure signal |
//! | CPU      | CFS throttle     | Yes -- graceful  | No -- the kernel already handles it |
//!
//! Exceeding the CPU quota is graceful and automatic (the scheduler throttles
//! us); exceeding the memory limit is fatal. So the dynamic pressure signal
//! that actually matters is **memory**, which is cgroup-aware here. There is
//! deliberately no `cpu.stat` reader: feeding a bespoke cgroup-CPU signal into
//! the scaler would prop up a scale-DOWN that the cgroup case does not want
//! (under a hard quota you want to USE your whole budget; CFS bounds it).
//!
//! ## What `ScalingInput::cpu_util` is for
//!
//! The host-wide CPU sample (via `sysinfo`) drives scale-DOWN as a
//! **bare-metal / unlimited-deployment good-neighbour heuristic** -- backing
//! off when sharing an un-capped node. Under a cgroup `cpu.max` it is largely
//! redundant with CFS and the cgroup-aware static sizing above. If in-process
//! scheduler busyness is ever needed as a finer signal, the existing wheel is
//! `tokio-metrics` (runtime busy-ratio), not a hand-rolled parser.

use std::sync::Arc;
use std::time::Duration;

use sysinfo::System;
use tokio_util::sync::CancellationToken;

use super::pool::AdaptiveWorkerPool;

/// Inputs to the watermark scaling algorithm.
#[derive(Debug, Clone)]
pub struct ScalingInput {
    /// Host-wide CPU utilisation (0.0-1.0), a bare-metal good-neighbour
    /// scale-down heuristic -- NOT a cgroup mechanism. See the module docs
    /// for the CPU-vs-memory asymmetry and why there is no cgroup-CPU signal.
    pub cpu_util: f64,
    pub memory_pressure: f64,
    pub current: usize,
    pub min_threads: usize,
    pub max_threads: usize,
    pub grow_below: f64,
    pub shrink_above: f64,
    pub emergency_above: f64,
    pub memory_pressure_cap: f64,
}

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
    pub fn evaluate(input: &ScalingInput) -> Self {
        // Memory pressure overrides everything -- prevent OOM
        if input.memory_pressure > input.memory_pressure_cap {
            return Self {
                target: input.min_threads,
                direction: "memory_cap",
            };
        }

        let (raw_target, direction) = if input.cpu_util < input.grow_below {
            (input.current.saturating_add(2), "up")
        } else if input.cpu_util <= input.shrink_above {
            (input.current, "steady")
        } else if input.cpu_util <= input.emergency_above {
            (input.current.saturating_sub(1), "down")
        } else {
            (input.current.saturating_sub(2), "emergency_down")
        };

        // Clamp to [min, max]
        let target = raw_target.clamp(input.min_threads, input.max_threads);

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

        // Memory signal, container-first. Priority:
        //   1. Attached MemoryGuard pressure (app-tracked bytes vs the cgroup
        //      limit) -- the most accurate signal for THIS service.
        //   2. The cgroup's own current/limit -- what the OOM killer acts on.
        //   3. Host used/total (sysinfo) -- ONLY as a bare-metal fallback.
        // Host memory must NOT drive container decisions: on a large shared
        // host it is unrelated to this container's cgroup limit (it can be
        // high from other tenants, or mask this container nearing its own cap).
        #[cfg(feature = "memory")]
        let guard_pressure = self
            .pool
            .memory_guard
            .lock()
            .as_ref()
            .map(|g| g.pressure_ratio());
        #[cfg(not(feature = "memory"))]
        let guard_pressure: Option<f64> = None;

        #[cfg(feature = "memory")]
        let cgroup_pressure = crate::memory::detect_memory_pressure();
        #[cfg(not(feature = "memory"))]
        let cgroup_pressure: Option<f64> = None;

        let effective_memory_pressure = guard_pressure.or(cgroup_pressure).unwrap_or_else(|| {
            self.system.refresh_memory();
            if self.system.total_memory() > 0 {
                self.system.used_memory() as f64 / self.system.total_memory() as f64
            } else {
                0.0
            }
        });

        // Read config (may have been hot-reloaded via Arc<RwLock<>>)
        let cfg = self.pool.config.read().clone();

        // Control input is the CURRENT TARGET (the ceiling we are adjusting),
        // not the in-flight count -- the watermark algorithm evolves the
        // target up/down from where it currently sits.
        let current_target = self.pool.target_threads();

        let decision = ScalingDecision::evaluate(&ScalingInput {
            cpu_util,
            memory_pressure: effective_memory_pressure,
            current: current_target,
            min_threads: cfg.min_threads,
            max_threads: cfg.max_threads,
            grow_below: cfg.grow_below,
            shrink_above: cfg.shrink_above,
            emergency_above: cfg.emergency_above,
            memory_pressure_cap: cfg.memory_pressure_cap,
        });

        if decision.direction == "steady" {
            tracing::debug!(
                cpu = format!("{cpu_util:.2}"),
                current = current_target,
                "Worker pool steady"
            );
        } else {
            tracing::debug!(
                cpu = format!("{cpu_util:.2}"),
                mem = format!("{effective_memory_pressure:.2}"),
                current = current_target,
                target = decision.target,
                direction = decision.direction,
                "Worker pool scaling"
            );
            metrics::counter!("worker_pool_scale_events_total", "direction" => decision.direction)
                .increment(1);
        }

        // Apply the new target concurrency.
        self.pool.semaphore.set_target(decision.target);

        // Emit operational metrics -- active (leased, in-flight) and target
        // (admission ceiling) are DISTINCT; do not conflate them.
        let leased = self.pool.active_threads();
        metrics::gauge!("worker_pool_active_threads").set(leased as f64);
        metrics::gauge!("worker_pool_target_threads").set(decision.target as f64);
        metrics::gauge!("worker_pool_available_threads")
            .set(decision.target.saturating_sub(leased) as f64);
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
