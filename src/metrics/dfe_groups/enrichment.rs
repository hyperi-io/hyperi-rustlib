// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED

//! Enrichment cache metrics (GeoIP, reputation, lookup tables).

use super::super::MetricsManager;
use super::super::manifest::{MetricDescriptor, MetricType};

/// Enrichment cache metrics.
///
/// Tracks cache hit/miss rates, cache size, and lookup latency.
/// The `type` label distinguishes enrichment sources (e.g., `geoip`, `reputation`).
#[derive(Clone)]
pub struct EnrichmentMetrics {
    namespace: String,
}

impl EnrichmentMetrics {
    #[must_use]
    pub fn new(manager: &MetricsManager) -> Self {
        let ns = manager.namespace();

        // enrichment_cache_hits_total — label-based
        let hits_key = if ns.is_empty() {
            "enrichment_cache_hits_total".to_string()
        } else {
            format!("{ns}_enrichment_cache_hits_total")
        };
        metrics::describe_counter!(hits_key.clone(), "Enrichment cache hits");
        manager.registry().push(MetricDescriptor {
            name: hits_key,
            metric_type: MetricType::Counter,
            description: "Enrichment cache hits".into(),
            unit: String::new(),
            labels: vec!["type".into()],
            group: "enrichment".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });

        // enrichment_cache_misses_total — label-based
        let misses_key = if ns.is_empty() {
            "enrichment_cache_misses_total".to_string()
        } else {
            format!("{ns}_enrichment_cache_misses_total")
        };
        metrics::describe_counter!(misses_key.clone(), "Enrichment cache misses");
        manager.registry().push(MetricDescriptor {
            name: misses_key,
            metric_type: MetricType::Counter,
            description: "Enrichment cache misses".into(),
            unit: String::new(),
            labels: vec!["type".into()],
            group: "enrichment".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });

        // enrichment_cache_size — label-based
        let size_key = if ns.is_empty() {
            "enrichment_cache_size".to_string()
        } else {
            format!("{ns}_enrichment_cache_size")
        };
        metrics::describe_gauge!(size_key.clone(), "Current enrichment cache entries");
        manager.registry().push(MetricDescriptor {
            name: size_key,
            metric_type: MetricType::Gauge,
            description: "Current enrichment cache entries".into(),
            unit: String::new(),
            labels: vec!["type".into()],
            group: "enrichment".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });

        // enrichment_duration_seconds — label-based
        let dur_key = if ns.is_empty() {
            "enrichment_duration_seconds".to_string()
        } else {
            format!("{ns}_enrichment_duration_seconds")
        };
        metrics::describe_histogram!(
            dur_key.clone(),
            metrics::Unit::Seconds,
            "Enrichment lookup latency"
        );
        manager.registry().push(MetricDescriptor {
            name: dur_key,
            metric_type: MetricType::Histogram,
            description: "Enrichment lookup latency".into(),
            unit: "seconds".into(),
            labels: vec!["type".into()],
            group: "enrichment".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });

        Self {
            namespace: ns.to_string(),
        }
    }

    #[inline]
    pub fn record_hit(&self, enrichment_type: &str) {
        let key = if self.namespace.is_empty() {
            "enrichment_cache_hits_total".to_string()
        } else {
            format!("{}_enrichment_cache_hits_total", self.namespace)
        };
        metrics::counter!(key, "type" => enrichment_type.to_string()).increment(1);
    }

    #[inline]
    pub fn record_miss(&self, enrichment_type: &str) {
        let key = if self.namespace.is_empty() {
            "enrichment_cache_misses_total".to_string()
        } else {
            format!("{}_enrichment_cache_misses_total", self.namespace)
        };
        metrics::counter!(key, "type" => enrichment_type.to_string()).increment(1);
    }

    #[inline]
    pub fn set_cache_size(&self, enrichment_type: &str, size: usize) {
        let key = if self.namespace.is_empty() {
            "enrichment_cache_size".to_string()
        } else {
            format!("{}_enrichment_cache_size", self.namespace)
        };
        metrics::gauge!(key, "type" => enrichment_type.to_string()).set(size as f64);
    }

    #[inline]
    pub fn record_duration(&self, enrichment_type: &str, seconds: f64) {
        let key = if self.namespace.is_empty() {
            "enrichment_duration_seconds".to_string()
        } else {
            format!("{}_enrichment_duration_seconds", self.namespace)
        };
        metrics::histogram!(key, "type" => enrichment_type.to_string()).record(seconds);
    }
}
