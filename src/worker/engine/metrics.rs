// Project:   hyperi-rustlib
// File:      src/worker/engine/metrics.rs
// Purpose:   Metric registration and threshold gauge emission for BatchEngine
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use crate::metrics::MetricsManager;

use super::config::BatchProcessingConfig;

/// Register all `BatchEngine` metrics with the `MetricsManager`.
///
/// Called by [`BatchEngine::auto_wire`](super::BatchEngine::auto_wire). Registers
/// descriptors for all operational metrics and immediately emits the current
/// config thresholds as gauges (for Grafana overlay of scaling decision lines).
pub fn register(manager: &MetricsManager, config: &BatchProcessingConfig) {
    // Counters
    let _ = manager.counter(
        "batch_engine_messages_received_total",
        "Messages received from transport",
    );
    let _ = manager.counter(
        "batch_engine_messages_parsed_total",
        "Messages successfully SIMD-parsed",
    );
    let _ = manager.counter(
        "batch_engine_messages_filtered_total",
        "Messages filtered at pre-route",
    );
    let _ = manager.counter("batch_engine_messages_dlq_total", "Messages routed to DLQ");
    let _ = manager.counter("batch_engine_parse_errors_total", "Parse failures");
    let _ = manager.counter(
        "batch_engine_memory_pressure_pauses_total",
        "MemoryGuard pause events between chunks",
    );

    // Histograms
    let _ = manager.histogram(
        "batch_engine_parse_duration_seconds",
        "SIMD parse time per chunk",
    );
    let _ = manager.histogram(
        "batch_engine_transform_duration_seconds",
        "App transform time per chunk",
    );
    let _ = manager.histogram("batch_engine_chunk_size", "Actual items per chunk");
    let _ = manager.histogram(
        "batch_engine_pre_route_duration_seconds",
        "Pre-route extraction time per chunk",
    );

    // Gauges
    let _ = manager.gauge(
        "batch_engine_intern_table_size",
        "Interned field name count",
    );

    // Config thresholds as gauges (emitted immediately).
    emit_thresholds(config);
}

/// Emit config threshold values as gauge metrics.
///
/// Called at startup (via `register`) and optionally on config reload.
/// Metric names are mechanically derived from config field names so that
/// Grafana dashboards can overlay config changes on operational graphs.
pub fn emit_thresholds(config: &BatchProcessingConfig) {
    metrics::gauge!("batch_engine_max_chunk_size").set(config.max_chunk_size as f64);
    metrics::gauge!("batch_engine_memory_pressure_pause_ms")
        .set(config.memory_pressure_pause_ms as f64);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_does_not_panic() {
        let manager = MetricsManager::new("test_engine_metrics");
        let config = BatchProcessingConfig::default();
        // Should complete without panic even with no recorder installed.
        register(&manager, &config);
    }

    #[test]
    fn emit_thresholds_does_not_panic() {
        let config = BatchProcessingConfig::default();
        // metrics macros are no-ops when no recorder is installed.
        emit_thresholds(&config);
    }

    #[test]
    fn register_returns_handles() {
        let manager = MetricsManager::new("test_engine_metrics_handles");
        let config = BatchProcessingConfig::default();
        // Calling twice should be idempotent.
        register(&manager, &config);
        register(&manager, &config);
    }
}
