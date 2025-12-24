// Project:   hs-rustlib
// File:      src/metrics/mod.rs
// Purpose:   Prometheus metrics with process and container awareness
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Prometheus metrics with process and container awareness.
//!
//! Provides production-ready metrics collection matching hs-golib.
//!
//! ## Features
//!
//! - Counter, Gauge, Histogram metric types
//! - Automatic process metrics (CPU, memory, file descriptors)
//! - Container metrics from cgroups (memory limit, CPU limit)
//! - Built-in HTTP server for `/metrics` endpoint
//!
//! ## Example
//!
//! ```rust,no_run
//! use hs_rustlib::metrics::{MetricsManager, MetricsConfig};
//!
//! #[tokio::main]
//! async fn main() {
//!     let mut manager = MetricsManager::new("myapp");
//!
//!     // Create metrics
//!     let requests = manager.counter("requests_total", "Total requests");
//!     let active = manager.gauge("active_connections", "Active connections");
//!     let latency = manager.histogram("request_duration_seconds", "Request latency");
//!
//!     // Start metrics server
//!     manager.start_server("0.0.0.0:9090").await.unwrap();
//!
//!     // Record metrics
//!     requests.increment(1);
//!     active.set(42.0);
//!     latency.record(0.123);
//! }
//! ```

mod container;
mod process;

use std::net::SocketAddr;
use std::time::Duration;

use metrics::{Counter, Gauge, Histogram, Unit};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

pub use container::ContainerMetrics;
pub use process::ProcessMetrics;

/// Metrics errors.
#[derive(Debug, Error)]
pub enum MetricsError {
    /// Failed to build metrics exporter.
    #[error("failed to build metrics exporter: {0}")]
    BuildError(String),

    /// Failed to start metrics server.
    #[error("failed to start metrics server: {0}")]
    ServerError(String),

    /// Server already running.
    #[error("metrics server already running")]
    AlreadyRunning,

    /// Server not running.
    #[error("metrics server not running")]
    NotRunning,
}

/// Metrics configuration.
#[derive(Debug, Clone)]
pub struct MetricsConfig {
    /// Metric namespace prefix.
    pub namespace: String,
    /// Enable process metrics collection.
    pub enable_process_metrics: bool,
    /// Enable container metrics collection.
    pub enable_container_metrics: bool,
    /// Update interval for auto-collected metrics.
    pub update_interval: Duration,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            namespace: String::new(),
            enable_process_metrics: true,
            enable_container_metrics: true,
            update_interval: Duration::from_secs(15),
        }
    }
}

/// Metrics manager handling Prometheus exposition.
pub struct MetricsManager {
    handle: PrometheusHandle,
    config: MetricsConfig,
    shutdown_tx: Option<oneshot::Sender<()>>,
    process_metrics: Option<ProcessMetrics>,
    container_metrics: Option<ContainerMetrics>,
}

impl MetricsManager {
    /// Create a new metrics manager with the given namespace.
    #[must_use]
    pub fn new(namespace: &str) -> Self {
        Self::with_config(MetricsConfig {
            namespace: namespace.to_string(),
            ..Default::default()
        })
    }

    /// Create a metrics manager with custom configuration.
    #[must_use]
    pub fn with_config(config: MetricsConfig) -> Self {
        let builder = PrometheusBuilder::new();
        let handle = builder
            .install_recorder()
            .expect("failed to install Prometheus recorder");

        let process_metrics = if config.enable_process_metrics {
            Some(ProcessMetrics::new(&config.namespace))
        } else {
            None
        };

        let container_metrics = if config.enable_container_metrics {
            Some(ContainerMetrics::new(&config.namespace))
        } else {
            None
        };

        Self {
            handle,
            config,
            shutdown_tx: None,
            process_metrics,
            container_metrics,
        }
    }

    /// Create a counter metric.
    #[must_use]
    pub fn counter(&self, name: &str, description: &str) -> Counter {
        let key = self.prefixed_key(name);
        let desc = description.to_string();
        metrics::describe_counter!(key.clone(), desc);
        metrics::counter!(key)
    }

    /// Create a gauge metric.
    #[must_use]
    pub fn gauge(&self, name: &str, description: &str) -> Gauge {
        let key = self.prefixed_key(name);
        let desc = description.to_string();
        metrics::describe_gauge!(key.clone(), desc);
        metrics::gauge!(key)
    }

    /// Create a histogram metric with default buckets.
    #[must_use]
    pub fn histogram(&self, name: &str, description: &str) -> Histogram {
        let key = self.prefixed_key(name);
        let desc = description.to_string();
        metrics::describe_histogram!(key.clone(), Unit::Seconds, desc);
        metrics::histogram!(key)
    }

    /// Create a histogram metric with custom buckets.
    #[must_use]
    pub fn histogram_with_buckets(
        &self,
        name: &str,
        description: &str,
        _buckets: &[f64],
    ) -> Histogram {
        // Note: metrics crate doesn't support custom buckets per-metric,
        // they're set globally. This is a placeholder for API compatibility.
        self.histogram(name, description)
    }

    /// Get the Prometheus metrics output.
    #[must_use]
    pub fn render(&self) -> String {
        self.handle.render()
    }

    /// Update process and container metrics.
    pub fn update(&self) {
        if let Some(ref pm) = self.process_metrics {
            pm.update();
        }
        if let Some(ref cm) = self.container_metrics {
            cm.update();
        }
    }

    /// Start the metrics HTTP server.
    ///
    /// # Errors
    ///
    /// Returns an error if the server fails to start.
    pub async fn start_server(&mut self, addr: &str) -> Result<(), MetricsError> {
        if self.shutdown_tx.is_some() {
            return Err(MetricsError::AlreadyRunning);
        }

        let addr: SocketAddr = addr
            .parse()
            .map_err(|e| MetricsError::ServerError(format!("invalid address: {e}")))?;

        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| MetricsError::ServerError(e.to_string()))?;

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        self.shutdown_tx = Some(shutdown_tx);

        let handle = self.handle.clone();
        let update_interval = self.config.update_interval;
        let process_metrics = self.process_metrics.clone();
        let container_metrics = self.container_metrics.clone();

        tokio::spawn(async move {
            run_server(
                listener,
                handle,
                shutdown_rx,
                update_interval,
                process_metrics,
                container_metrics,
            )
            .await;
        });

        Ok(())
    }

    /// Stop the metrics server.
    ///
    /// # Errors
    ///
    /// Returns an error if the server is not running.
    pub async fn stop_server(&mut self) -> Result<(), MetricsError> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
            Ok(())
        } else {
            Err(MetricsError::NotRunning)
        }
    }

    /// Get prefixed metric name.
    fn prefixed_key(&self, name: &str) -> String {
        if self.config.namespace.is_empty() {
            name.to_string()
        } else {
            format!("{}_{}", self.config.namespace, name)
        }
    }
}

/// Run the metrics HTTP server.
async fn run_server(
    listener: TcpListener,
    handle: PrometheusHandle,
    mut shutdown_rx: oneshot::Receiver<()>,
    update_interval: Duration,
    process_metrics: Option<ProcessMetrics>,
    container_metrics: Option<ContainerMetrics>,
) {
    let mut update_interval = tokio::time::interval(update_interval);

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => {
                break;
            }
            _ = update_interval.tick() => {
                if let Some(ref pm) = process_metrics {
                    pm.update();
                }
                if let Some(ref cm) = container_metrics {
                    cm.update();
                }
            }
            result = listener.accept() => {
                if let Ok((stream, _)) = result {
                    let handle = handle.clone();
                    tokio::spawn(async move {
                        handle_connection(stream, handle).await;
                    });
                }
            }
        }
    }
}

/// Handle a single HTTP connection.
async fn handle_connection(mut stream: tokio::net::TcpStream, handle: PrometheusHandle) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let mut reader = BufReader::new(&mut stream);
    let mut request_line = String::new();

    if reader.read_line(&mut request_line).await.is_err() {
        return;
    }

    let (status, body) = if request_line.starts_with("GET /metrics") {
        ("200 OK", handle.render())
    } else if request_line.starts_with("GET /healthz")
        || request_line.starts_with("GET /health/live")
    {
        ("200 OK", r#"{"status":"alive"}"#.to_string())
    } else if request_line.starts_with("GET /readyz")
        || request_line.starts_with("GET /health/ready")
    {
        ("200 OK", r#"{"status":"ready"}"#.to_string())
    } else {
        ("404 Not Found", "Not Found".to_string())
    };

    let content_type = if body.starts_with('{') {
        "application/json"
    } else {
        "text/plain; charset=utf-8"
    };

    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );

    let _ = stream.write_all(response.as_bytes()).await;
}

/// Standard latency histogram buckets.
#[must_use]
pub fn latency_buckets() -> Vec<f64> {
    vec![
        0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
    ]
}

/// Standard size histogram buckets.
#[must_use]
pub fn size_buckets() -> Vec<f64> {
    vec![
        100.0,
        1_000.0,
        10_000.0,
        100_000.0,
        1_000_000.0,
        10_000_000.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_config_default() {
        let config = MetricsConfig::default();
        assert!(config.namespace.is_empty());
        assert!(config.enable_process_metrics);
        assert!(config.enable_container_metrics);
        assert_eq!(config.update_interval, Duration::from_secs(15));
    }

    #[test]
    fn test_latency_buckets() {
        let buckets = latency_buckets();
        assert_eq!(buckets.len(), 12);
        assert!(buckets[0] < buckets[11]);
    }

    #[test]
    fn test_size_buckets() {
        let buckets = size_buckets();
        assert_eq!(buckets.len(), 6);
        assert!(buckets[0] < buckets[5]);
    }
}
