// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED

//! Schema cache metrics for apps with dynamic schema reflection.

use metrics::{Counter, Gauge};

use super::super::MetricsManager;

/// Schema cache metrics.
///
/// Tracks cache hit/miss rates, cached table count, and schema recovery events.
#[derive(Clone)]
pub struct SchemaCacheMetrics {
    pub hits: Counter,
    pub misses: Counter,
    pub tables: Gauge,
    namespace: String,
}

impl SchemaCacheMetrics {
    #[must_use]
    pub fn new(manager: &MetricsManager) -> Self {
        let ns = manager.namespace();
        let recovery_key = if ns.is_empty() {
            "schema_recovery_total".to_string()
        } else {
            format!("{ns}_schema_recovery_total")
        };
        metrics::describe_counter!(recovery_key, "Schema mismatch recovery events");

        Self {
            hits: manager.counter("schema_cache_hits_total", "Schema cache hits"),
            misses: manager.counter(
                "schema_cache_misses_total",
                "Schema cache misses (triggers fetch)",
            ),
            tables: manager.gauge("schema_cache_tables", "Number of cached table schemas"),
            namespace: ns.to_string(),
        }
    }

    #[inline]
    pub fn record_hit(&self) {
        self.hits.increment(1);
    }

    #[inline]
    pub fn record_miss(&self) {
        self.misses.increment(1);
    }

    #[inline]
    pub fn set_tables(&self, count: usize) {
        self.tables.set(count as f64);
    }

    /// Record a schema recovery event for a specific table.
    #[inline]
    pub fn record_recovery(&self, table: &str) {
        let key = if self.namespace.is_empty() {
            "schema_recovery_total".to_string()
        } else {
            format!("{}_schema_recovery_total", self.namespace)
        };
        metrics::counter!(key, "table" => table.to_string()).increment(1);
    }
}
