// Project:   hyperi-rustlib
// File:      src/worker/metrics.rs
// Purpose:   Metric registration and threshold gauge emission
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use crate::metrics::MetricsManager;

use super::config::WorkerPoolConfig;

/// Register all worker pool metrics with the `MetricsManager`.
///
/// This registers both operational metrics and threshold gauges.
/// Threshold gauges are emitted immediately with current config values.
pub fn register(manager: &MetricsManager, config: &WorkerPoolConfig) {
    // Operational metrics — register descriptions (return values intentionally unused)
    let _ = manager.gauge(
        "worker_pool_active_threads",
        "Current active worker threads",
    );
    let _ = manager.gauge(
        "worker_pool_target_threads",
        "Target thread count from scaler",
    );
    let _ = manager.gauge("worker_pool_max_threads", "Maximum pool threads");
    let _ = manager.gauge(
        "worker_pool_cpu_utilisation",
        "Current CPU utilisation sample",
    );
    let _ = manager.gauge(
        "worker_pool_memory_utilisation",
        "Effective memory pressure",
    );
    let _ = manager.gauge(
        "worker_pool_saturation",
        "Pool saturation ratio (active/max)",
    );
    let _ = manager.counter(
        "worker_pool_tasks_total",
        "Total tasks submitted to rayon pool",
    );
    let _ = manager.histogram(
        "worker_pool_task_duration_seconds",
        "Per-task execution time",
    );
    let _ = manager.histogram(
        "worker_pool_batch_duration_seconds",
        "End-to-end batch processing time",
    );
    let _ = manager.histogram(
        "worker_pool_semaphore_wait_seconds",
        "Time waiting for semaphore permit",
    );
    let _ = manager.counter("worker_pool_scale_events_total", "Scaling events");
    let _ = manager.gauge(
        "worker_pool_async_inflight",
        "Current async fan-out tasks in flight",
    );

    // Threshold gauges (config values as metrics — emitted immediately)
    emit_thresholds(config);
}

/// Emit threshold gauge values (called at startup and on config reload).
///
/// Metric names match config keys exactly for mechanical derivation:
/// config key `grow_below` → metric `worker_pool_grow_below`.
pub fn emit_thresholds(config: &WorkerPoolConfig) {
    metrics::gauge!("worker_pool_min_threads").set(config.min_threads as f64);
    metrics::gauge!("worker_pool_max_threads").set(config.max_threads as f64);
    metrics::gauge!("worker_pool_grow_below").set(config.grow_below);
    metrics::gauge!("worker_pool_shrink_above").set(config.shrink_above);
    metrics::gauge!("worker_pool_emergency_above").set(config.emergency_above);
    metrics::gauge!("worker_pool_memory_pressure_cap").set(config.memory_pressure_cap);
    metrics::gauge!("worker_pool_scale_interval_secs").set(config.scale_interval_secs as f64);
    metrics::gauge!("worker_pool_async_concurrency").set(config.async_concurrency as f64);
    metrics::gauge!("worker_pool_health_saturation_timeout_secs")
        .set(config.health_saturation_timeout_secs as f64);
}
