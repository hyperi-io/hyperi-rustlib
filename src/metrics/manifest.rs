// Project:   hyperi-rustlib
// File:      src/metrics/manifest.rs
// Purpose:   Metric manifest types and registry for /metrics/manifest endpoint
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Metric manifest types for the `/metrics/manifest` endpoint.
//!
//! Provides [`MetricDescriptor`], [`MetricRegistry`], and [`ManifestResponse`]
//! for exposing machine-readable metric metadata. Field semantics align with
//! [OpenMetrics](https://prometheus.io/docs/specs/om/open_metrics_spec/) (type,
//! description, unit) and [OTel Advisory Parameters](https://opentelemetry.io/docs/specs/otel/metrics/api/)
//! (labels, buckets). Novel fields (`group`, `use_cases`, `dashboard_hint`)
//! are HyperI extensions.
//!
//! ## Standards Alignment
//!
//! | Field | Standard |
//! |-------|----------|
//! | `type` | OpenMetrics `TYPE` |
//! | `description` | OpenMetrics `HELP` |
//! | `unit` | OpenMetrics `UNIT` |
//! | `labels` | OTel Advisory `Attributes` |
//! | `buckets` | OTel Advisory `ExplicitBucketBoundaries` |
//! | `group`, `use_cases`, `dashboard_hint` | HyperI extensions |

use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

/// Describes a single registered metric for the manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricDescriptor {
    /// Full metric name including namespace prefix.
    pub name: String,
    /// Metric type (aligns with OpenMetrics TYPE).
    #[serde(rename = "type")]
    pub metric_type: MetricType,
    /// Human-readable description (aligns with OpenMetrics HELP).
    pub description: String,
    /// Unit suffix (aligns with OpenMetrics UNIT). Empty for counters.
    pub unit: String,
    /// Known label keys (aligns with OTel Advisory Attributes).
    pub labels: Vec<String>,
    /// Metric group membership. Always present. Defaults to `"custom"`.
    pub group: String,
    /// Histogram bucket boundaries (aligns with OTel Advisory ExplicitBucketBoundaries).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buckets: Option<Vec<f64>>,
    /// Operational guidance: when to alert, what dashboard to use.
    /// Novel HyperI extension. Omitted from JSON when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub use_cases: Vec<String>,
    /// Suggested Grafana panel type. Novel HyperI extension.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboard_hint: Option<String>,
}

/// Metric type discriminator (aligns with OpenMetrics TYPE).
///
/// Derives `Copy` for efficient pattern matching and comparison.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MetricType {
    Counter,
    Gauge,
    Histogram,
}

/// JSON response for `GET /metrics/manifest`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestResponse {
    pub schema_version: u32,
    pub app: String,
    pub version: String,
    pub commit: String,
    pub registered_at: String,
    pub metrics: Vec<MetricDescriptor>,
}

/// Inner state of the metric registry.
struct MetricRegistryInner {
    descriptors: Vec<MetricDescriptor>,
    app: String,
    version: String,
    commit: String,
    registered_at: String,
}

/// Cloneable handle to the metric registry.
///
/// Obtained via [`super::MetricsManager::registry`]. Safe to clone into
/// axum route handlers or share across tasks.
#[derive(Clone)]
pub struct MetricRegistry {
    inner: Arc<RwLock<MetricRegistryInner>>,
}

impl MetricRegistry {
    /// Create a new registry for the given app namespace.
    pub(crate) fn new(app: &str) -> Self {
        Self {
            inner: Arc::new(RwLock::new(MetricRegistryInner {
                descriptors: Vec::new(),
                app: app.to_string(),
                version: String::new(),
                commit: String::new(),
                registered_at: now_rfc3339(),
            })),
        }
    }

    /// Push a metric descriptor into the registry.
    pub(crate) fn push(&self, descriptor: MetricDescriptor) {
        if let Ok(mut inner) = self.inner.write() {
            inner.descriptors.push(descriptor);
        }
    }

    /// Set the application version and commit.
    pub(crate) fn set_build_info(&self, version: &str, commit: &str) {
        if let Ok(mut inner) = self.inner.write() {
            inner.version = version.to_string();
            inner.commit = commit.to_string();
        }
    }

    /// Set use cases for a metric by full name. No-op if not found.
    pub(crate) fn set_use_cases(&self, metric_name: &str, use_cases: &[&str]) {
        if let Ok(mut inner) = self.inner.write() {
            if let Some(desc) = inner.descriptors.iter_mut().find(|d| d.name == metric_name) {
                desc.use_cases = use_cases.iter().map(|s| (*s).to_string()).collect();
            } else {
                #[cfg(feature = "logger")]
                tracing::warn!(
                    metric = metric_name,
                    "set_use_cases: metric not found in registry"
                );
            }
        }
    }

    /// Set dashboard hint for a metric by full name. No-op if not found.
    pub(crate) fn set_dashboard_hint(&self, metric_name: &str, hint: &str) {
        if let Ok(mut inner) = self.inner.write() {
            if let Some(desc) = inner.descriptors.iter_mut().find(|d| d.name == metric_name) {
                desc.dashboard_hint = Some(hint.to_string());
            } else {
                #[cfg(feature = "logger")]
                tracing::warn!(
                    metric = metric_name,
                    "set_dashboard_hint: metric not found in registry"
                );
            }
        }
    }

    /// Build the manifest response snapshot.
    #[must_use]
    pub fn manifest(&self) -> ManifestResponse {
        let inner = self.inner.read().expect("registry lock poisoned");
        ManifestResponse {
            schema_version: 1,
            app: inner.app.clone(),
            version: inner.version.clone(),
            commit: inner.commit.clone(),
            registered_at: inner.registered_at.clone(),
            metrics: inner.descriptors.clone(),
        }
    }
}

/// Format current UTC time as RFC 3339 with second precision (no sub-seconds).
///
/// Output: `2026-03-31T02:00:00Z`
///
/// Pure function, no global state, trivially thread-safe.
pub(crate) fn now_rfc3339() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = d.as_secs();

    let days = (total_secs / 86400) as i64;
    let time_of_day = total_secs % 86400;

    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm)
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{y:04}-{m:02}-{d:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metric_type_serializes_to_snake_case() {
        assert_eq!(
            serde_json::to_string(&MetricType::Counter).unwrap(),
            "\"counter\""
        );
        assert_eq!(
            serde_json::to_string(&MetricType::Gauge).unwrap(),
            "\"gauge\""
        );
        assert_eq!(
            serde_json::to_string(&MetricType::Histogram).unwrap(),
            "\"histogram\""
        );
    }

    #[test]
    fn test_metric_descriptor_serializes_type_as_type() {
        let desc = MetricDescriptor {
            name: "test_total".into(),
            metric_type: MetricType::Counter,
            description: "A test counter".into(),
            unit: String::new(),
            labels: vec![],
            group: "custom".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        };
        let json = serde_json::to_value(&desc).unwrap();
        assert_eq!(json["type"], "counter");
        assert!(json.get("metric_type").is_none());
    }

    #[test]
    fn test_empty_use_cases_omitted_from_json() {
        let desc = MetricDescriptor {
            name: "test_gauge".into(),
            metric_type: MetricType::Gauge,
            description: "A gauge".into(),
            unit: String::new(),
            labels: vec![],
            group: "custom".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        };
        let json = serde_json::to_value(&desc).unwrap();
        assert!(json.get("use_cases").is_none());
        assert!(json.get("buckets").is_none());
        assert!(json.get("dashboard_hint").is_none());
    }

    #[test]
    fn test_populated_use_cases_included() {
        let desc = MetricDescriptor {
            name: "test_hist".into(),
            metric_type: MetricType::Histogram,
            description: "A histogram".into(),
            unit: "seconds".into(),
            labels: vec!["backend".into()],
            group: "sink".into(),
            buckets: Some(vec![0.01, 0.1, 1.0]),
            use_cases: vec!["Alert when p99 > 5s".into()],
            dashboard_hint: Some("heatmap".into()),
        };
        let json = serde_json::to_value(&desc).unwrap();
        assert_eq!(
            json["use_cases"],
            serde_json::json!(["Alert when p99 > 5s"])
        );
        assert_eq!(json["buckets"], serde_json::json!([0.01, 0.1, 1.0]));
        assert_eq!(json["dashboard_hint"], "heatmap");
    }

    #[test]
    fn test_manifest_response_round_trips() {
        let manifest = ManifestResponse {
            schema_version: 1,
            app: "test_app".into(),
            version: "1.0.0".into(),
            commit: "abc123".into(),
            registered_at: "2026-03-31T00:00:00Z".into(),
            metrics: vec![MetricDescriptor {
                name: "test_total".into(),
                metric_type: MetricType::Counter,
                description: "test".into(),
                unit: String::new(),
                labels: vec![],
                group: "custom".into(),
                buckets: None,
                use_cases: vec![],
                dashboard_hint: None,
            }],
        };
        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: ManifestResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.schema_version, 1);
        assert_eq!(parsed.app, "test_app");
        assert_eq!(parsed.metrics.len(), 1);
        assert_eq!(parsed.metrics[0].metric_type, MetricType::Counter);
    }

    #[test]
    fn test_counter_unit_is_empty_not_total() {
        let desc = MetricDescriptor {
            name: "requests_total".into(),
            metric_type: MetricType::Counter,
            description: "Requests".into(),
            unit: String::new(),
            labels: vec![],
            group: "custom".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        };
        let json = serde_json::to_value(&desc).unwrap();
        assert_eq!(json["unit"], "");
    }

    #[test]
    fn test_now_rfc3339_format() {
        let ts = now_rfc3339();
        assert_eq!(ts.len(), 20);
        assert!(ts.ends_with('Z'));
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[7..8], "-");
        assert_eq!(&ts[10..11], "T");
        assert_eq!(&ts[13..14], ":");
        assert_eq!(&ts[16..17], ":");
    }

    #[test]
    fn test_registry_push_and_manifest() {
        let reg = MetricRegistry::new("test_app");
        reg.push(MetricDescriptor {
            name: "test_app_requests_total".into(),
            metric_type: MetricType::Counter,
            description: "Total requests".into(),
            unit: String::new(),
            labels: vec!["method".into()],
            group: "app".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });
        let manifest = reg.manifest();
        assert_eq!(manifest.app, "test_app");
        assert_eq!(manifest.schema_version, 1);
        assert_eq!(manifest.metrics.len(), 1);
        assert_eq!(manifest.metrics[0].name, "test_app_requests_total");
        assert_eq!(manifest.metrics[0].labels, vec!["method"]);
    }

    #[test]
    fn test_registry_set_build_info() {
        let reg = MetricRegistry::new("test_app");
        reg.set_build_info("2.0.0", "def456");
        let manifest = reg.manifest();
        assert_eq!(manifest.version, "2.0.0");
        assert_eq!(manifest.commit, "def456");
    }

    #[test]
    fn test_registry_set_use_cases() {
        let reg = MetricRegistry::new("test_app");
        reg.push(MetricDescriptor {
            name: "my_metric".into(),
            metric_type: MetricType::Gauge,
            description: "test".into(),
            unit: String::new(),
            labels: vec![],
            group: "custom".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });
        reg.set_use_cases("my_metric", &["Alert when > 90%"]);
        let manifest = reg.manifest();
        assert_eq!(manifest.metrics[0].use_cases, vec!["Alert when > 90%"]);
    }

    #[test]
    fn test_registry_set_use_cases_nonexistent_is_noop() {
        let reg = MetricRegistry::new("test_app");
        // Should not panic
        reg.set_use_cases("nonexistent", &["some use case"]);
    }

    #[test]
    fn test_registry_set_dashboard_hint() {
        let reg = MetricRegistry::new("test_app");
        reg.push(MetricDescriptor {
            name: "my_metric".into(),
            metric_type: MetricType::Gauge,
            description: "test".into(),
            unit: String::new(),
            labels: vec![],
            group: "custom".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        });
        reg.set_dashboard_hint("my_metric", "stat");
        let manifest = reg.manifest();
        assert_eq!(manifest.metrics[0].dashboard_hint, Some("stat".to_string()));
    }

    #[test]
    fn test_group_always_present_in_json() {
        let desc = MetricDescriptor {
            name: "test".into(),
            metric_type: MetricType::Counter,
            description: "test".into(),
            unit: String::new(),
            labels: vec![],
            group: "custom".into(),
            buckets: None,
            use_cases: vec![],
            dashboard_hint: None,
        };
        let json = serde_json::to_value(&desc).unwrap();
        assert_eq!(json["group"], "custom");
    }
}
