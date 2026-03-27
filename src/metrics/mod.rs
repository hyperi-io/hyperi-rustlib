// Project:   hyperi-rustlib
// File:      src/metrics/mod.rs
// Purpose:   Prometheus metrics with process and container awareness
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Metrics with Prometheus and/or OpenTelemetry backends.
//!
//! Provides production-ready metrics collection with support for:
//!
//! - **`metrics` feature only:** Prometheus scrape endpoint via `/metrics`
//! - **`otel-metrics` feature only:** OTLP push to OTel-compatible backends
//! - **Both features:** Fanout recorder sends to both Prometheus AND OTel
//!
//! ## Features
//!
//! - Counter, Gauge, Histogram metric types
//! - Automatic process metrics (CPU, memory, file descriptors)
//! - Container metrics from cgroups (memory limit, CPU limit)
//! - Built-in HTTP server for `/metrics` endpoint (Prometheus)
//! - OTLP push to HyperDX, Jaeger, Grafana, etc. (OTel)
//! - Readiness callback for `/health/ready` endpoints
//! - Optional scaling pressure endpoint (`/scaling/pressure`)
//! - Optional memory guard endpoint (`/memory/pressure`)
//! - Custom route support via [`start_server_with_routes`](MetricsManager::start_server_with_routes)
//!
//! ## Basic Example
//!
//! ```rust,no_run
//! use hyperi_rustlib::metrics::{MetricsManager, MetricsConfig};
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
//!     // Start metrics server (simple — built-in endpoints only)
//!     manager.start_server("0.0.0.0:9090").await.unwrap();
//!
//!     // Record metrics
//!     requests.increment(1);
//!     active.set(42.0);
//!     latency.record(0.123);
//! }
//! ```
//!
//! ## Advanced Example (with custom routes, scaling, memory)
//!
//! Requires features: `metrics`, `http-server`, `scaling`, `memory`.
//!
//! ```rust,ignore
//! use std::sync::Arc;
//! use hyperi_rustlib::metrics::MetricsManager;
//! use hyperi_rustlib::scaling::{ScalingPressure, ScalingPressureConfig};
//! use hyperi_rustlib::memory::{MemoryGuard, MemoryGuardConfig};
//! use axum::{Router, routing::post};
//!
//! let mut mgr = MetricsManager::new("myapp");
//!
//! // Readiness callback
//! mgr.set_readiness_check(|| true);
//!
//! // Attach scaling pressure (adds /scaling/pressure endpoint)
//! let scaling = Arc::new(ScalingPressure::new(ScalingPressureConfig::default(), vec![]));
//! mgr.set_scaling_pressure(scaling);
//!
//! // Attach memory guard (adds /memory/pressure endpoint)
//! let guard = Arc::new(MemoryGuard::new(MemoryGuardConfig::default()));
//! mgr.set_memory_guard(guard);
//!
//! // Service-specific routes
//! let custom = Router::new()
//!     .route("/test", post(|| async { "ok" }));
//!
//! // Start with everything merged into one server
//! mgr.start_server_with_routes("0.0.0.0:9090", custom).await.unwrap();
//! ```

mod container;
pub mod dfe;
mod process;

#[cfg(feature = "otel-metrics")]
pub(crate) mod otel;
#[cfg(feature = "otel-metrics")]
pub mod otel_types;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use metrics::{Counter, Gauge, Histogram, Unit};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// Readiness check callback type.
pub type ReadinessFn = Arc<dyn Fn() -> bool + Send + Sync>;

#[cfg(feature = "metrics")]
use metrics_exporter_prometheus::PrometheusHandle;

pub use container::ContainerMetrics;
pub use dfe::DfeMetrics;
#[cfg(feature = "metrics-dfe")]
pub mod dfe_groups;
pub use process::ProcessMetrics;

#[cfg(feature = "otel-metrics")]
pub use otel_types::{OtelMetricsConfig, OtelProtocol};

/// Cloneable handle for rendering Prometheus metrics text.
///
/// Obtained via [`MetricsManager::render_handle`]. Safe to clone into
/// `axum` route handlers or share across tasks.
#[cfg(feature = "metrics")]
#[derive(Clone)]
pub struct RenderHandle(PrometheusHandle);

#[cfg(feature = "metrics")]
impl RenderHandle {
    /// Render current metrics in Prometheus text format.
    #[must_use]
    pub fn render(&self) -> String {
        self.0.render()
    }
}

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
    /// OTel-specific configuration (only used when `otel-metrics` feature is enabled).
    #[cfg(feature = "otel-metrics")]
    pub otel: OtelMetricsConfig,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            namespace: String::new(),
            enable_process_metrics: true,
            enable_container_metrics: true,
            update_interval: Duration::from_secs(15),
            #[cfg(feature = "otel-metrics")]
            otel: OtelMetricsConfig::default(),
        }
    }
}

/// Intermediate struct to pass recorder setup results across cfg boundaries.
struct RecorderSetup {
    #[cfg(feature = "metrics")]
    prom_handle: Option<PrometheusHandle>,
    #[cfg(feature = "otel-metrics")]
    otel_provider: Option<opentelemetry_sdk::metrics::SdkMeterProvider>,
}

/// Install the metrics recorder(s) based on enabled features.
///
/// Returns setup results containing handles/providers. When both
/// `metrics` and `otel-metrics` features are enabled, uses `metrics-util`
/// `FanoutBuilder` to compose both recorders into a single global recorder.
#[allow(unused_variables)]
fn install_recorders(config: &MetricsConfig) -> RecorderSetup {
    // --- Prometheus only (no OTel) ---
    #[cfg(all(feature = "metrics", not(feature = "otel-metrics")))]
    {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::set_global_recorder(recorder).expect("failed to install Prometheus recorder");
        RecorderSetup {
            prom_handle: Some(handle),
        }
    }

    // --- OTel only (no Prometheus) ---
    #[cfg(all(feature = "otel-metrics", not(feature = "metrics")))]
    {
        match otel::build_otel_recorder(&config.namespace, &config.otel) {
            Ok((otel_recorder, provider)) => {
                opentelemetry::global::set_meter_provider(provider.clone());
                metrics::set_global_recorder(otel_recorder)
                    .expect("failed to set OTel metrics recorder");
                RecorderSetup {
                    otel_provider: Some(provider),
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to build OTel metrics recorder");
                RecorderSetup {
                    otel_provider: None,
                }
            }
        }
    }

    // --- Both Prometheus + OTel (Fanout) ---
    #[cfg(all(feature = "metrics", feature = "otel-metrics"))]
    {
        // Build Prometheus recorder (without installing globally)
        let prom_recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let prom_handle = prom_recorder.handle();

        // Build OTel recorder
        match otel::build_otel_recorder(&config.namespace, &config.otel) {
            Ok((otel_recorder, provider)) => {
                opentelemetry::global::set_meter_provider(provider.clone());

                // Compose via Fanout: both recorders receive every measurement
                let fanout = metrics_util::layers::FanoutBuilder::default()
                    .add_recorder(prom_recorder)
                    .add_recorder(otel_recorder)
                    .build();

                metrics::set_global_recorder(fanout).expect("failed to set Fanout recorder");

                RecorderSetup {
                    prom_handle: Some(prom_handle),
                    otel_provider: Some(provider),
                }
            }
            Err(e) => {
                // Fallback: just Prometheus if OTel fails
                tracing::warn!(error = %e, "Failed to build OTel recorder, falling back to Prometheus only");
                metrics::set_global_recorder(prom_recorder)
                    .expect("failed to set Prometheus recorder");
                RecorderSetup {
                    prom_handle: Some(prom_handle),
                    otel_provider: None,
                }
            }
        }
    }
}

/// Metrics manager handling Prometheus and/or OTel exposition.
pub struct MetricsManager {
    #[cfg(feature = "metrics")]
    handle: Option<PrometheusHandle>,
    config: MetricsConfig,
    shutdown_tx: Option<oneshot::Sender<()>>,
    process_metrics: Option<ProcessMetrics>,
    container_metrics: Option<ContainerMetrics>,
    readiness_fn: Option<ReadinessFn>,
    #[cfg(all(feature = "metrics", feature = "scaling"))]
    scaling_pressure: Option<Arc<crate::scaling::ScalingPressure>>,
    #[cfg(all(feature = "metrics", feature = "memory"))]
    memory_guard: Option<Arc<crate::memory::MemoryGuard>>,
    #[cfg(feature = "otel-metrics")]
    otel_provider: Option<opentelemetry_sdk::metrics::SdkMeterProvider>,
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
    ///
    /// Installs the appropriate recorder(s) based on enabled features:
    /// - `metrics` only: Prometheus recorder
    /// - `otel-metrics` only: OTel recorder (OTLP push)
    /// - Both: Fanout recorder (Prometheus scrape + OTel OTLP push)
    #[must_use]
    pub fn with_config(config: MetricsConfig) -> Self {
        let setup = install_recorders(&config);

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
            #[cfg(feature = "metrics")]
            handle: setup.prom_handle,
            config,
            shutdown_tx: None,
            process_metrics,
            container_metrics,
            readiness_fn: None,
            #[cfg(all(feature = "metrics", feature = "scaling"))]
            scaling_pressure: None,
            #[cfg(all(feature = "metrics", feature = "memory"))]
            memory_guard: None,
            #[cfg(feature = "otel-metrics")]
            otel_provider: setup.otel_provider,
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
    ///
    /// **Note:** The `buckets` parameter is accepted for API compatibility but
    /// is currently ignored. The `metrics` crate sets histogram buckets globally
    /// at recorder installation time, not per-metric. Use
    /// `PrometheusBuilder::set_buckets_for_metric` when building the recorder
    /// if you need per-metric bucket configuration.
    ///
    /// This method exists so callers can express intent about bucket ranges
    /// without breaking if per-metric support is added later.
    #[must_use]
    pub fn histogram_with_buckets(
        &self,
        name: &str,
        description: &str,
        _buckets: &[f64],
    ) -> Histogram {
        self.histogram(name, description)
    }

    /// Get the Prometheus metrics output.
    ///
    /// Returns the rendered Prometheus text format. Only available when
    /// the `metrics` feature is enabled.
    #[cfg(feature = "metrics")]
    #[must_use]
    pub fn render(&self) -> String {
        self.handle
            .as_ref()
            .map_or_else(String::new, PrometheusHandle::render)
    }

    /// Get a cloneable render handle for use in route handlers.
    ///
    /// Returns a closure that renders the current Prometheus metrics text.
    /// The closure is `Send + Sync + Clone`, making it safe to move into
    /// `axum` route handlers or share across tasks via `Arc`.
    ///
    /// Returns `None` if no Prometheus recorder is installed.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut mgr = MetricsManager::new("myapp");
    /// let render = mgr.render_handle().expect("recorder installed");
    ///
    /// // Use in axum route
    /// let route = axum::Router::new().route("/metrics", axum::routing::get(move || {
    ///     let r = render.clone();
    ///     async move { r() }
    /// }));
    ///
    /// mgr.start_server_with_routes("0.0.0.0:9090", route).await?;
    /// ```
    #[cfg(feature = "metrics")]
    #[must_use]
    pub fn render_handle(&self) -> Option<RenderHandle> {
        self.handle.clone().map(RenderHandle)
    }

    /// Set a readiness check callback.
    ///
    /// When set, `/readyz` and `/health/ready` call this function and return
    /// 503 Service Unavailable if it returns `false`. Without a callback,
    /// these endpoints always return 200.
    pub fn set_readiness_check(&mut self, f: impl Fn() -> bool + Send + Sync + 'static) {
        self.readiness_fn = Some(Arc::new(f));
    }

    /// Attach a `ScalingPressure` instance.
    ///
    /// When set and using `start_server_with_routes`, a `/scaling/pressure`
    /// endpoint is automatically added that returns the current pressure value.
    #[cfg(all(feature = "metrics", feature = "scaling"))]
    pub fn set_scaling_pressure(&mut self, sp: Arc<crate::scaling::ScalingPressure>) {
        self.scaling_pressure = Some(sp);
    }

    /// Attach a `MemoryGuard` instance.
    ///
    /// When set and using `start_server_with_routes`, a `/memory/pressure`
    /// endpoint is automatically added that returns the current memory status.
    #[cfg(all(feature = "metrics", feature = "memory"))]
    pub fn set_memory_guard(&mut self, mg: Arc<crate::memory::MemoryGuard>) {
        self.memory_guard = Some(mg);
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
    /// Serves `/metrics` (Prometheus), `/healthz`, `/health/live`,
    /// `/readyz`, `/health/ready` endpoints.
    ///
    /// Only available when the `metrics` feature is enabled (for scraping).
    ///
    /// # Errors
    ///
    /// Returns an error if the server fails to start.
    #[cfg(feature = "metrics")]
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

        let handle = self
            .handle
            .as_ref()
            .ok_or_else(|| {
                MetricsError::ServerError(
                    "Prometheus handle not configured — MetricsManager was created without a recorder".into(),
                )
            })?
            .clone();
        let update_interval = self.config.update_interval;
        let process_metrics = self.process_metrics.clone();
        let container_metrics = self.container_metrics.clone();
        let readiness_fn = self.readiness_fn.clone();

        tokio::spawn(async move {
            run_server(
                listener,
                handle,
                shutdown_rx,
                update_interval,
                process_metrics,
                container_metrics,
                readiness_fn,
            )
            .await;
        });

        Ok(())
    }

    /// Start the metrics HTTP server with additional custom routes.
    ///
    /// Serves the same built-in endpoints as [`start_server`](Self::start_server):
    /// `/metrics`, `/healthz`, `/health/live`, `/readyz`, `/health/ready`.
    ///
    /// Additionally:
    /// - If [`set_scaling_pressure`](Self::set_scaling_pressure) was called,
    ///   adds `/scaling/pressure` returning the current pressure value.
    /// - If [`set_memory_guard`](Self::set_memory_guard) was called,
    ///   adds `/memory/pressure` returning memory status JSON.
    /// - Any routes in `extra_routes` are merged (service-specific endpoints).
    ///
    /// Requires both `metrics` and `http-server` features.
    ///
    /// # Errors
    ///
    /// Returns an error if the server fails to start.
    #[cfg(all(feature = "metrics", feature = "http-server"))]
    pub async fn start_server_with_routes(
        &mut self,
        addr: &str,
        extra_routes: axum::Router,
    ) -> Result<(), MetricsError> {
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

        let handle = self
            .handle
            .as_ref()
            .ok_or_else(|| {
                MetricsError::ServerError(
                    "Prometheus handle not configured — MetricsManager was created without a recorder".into(),
                )
            })?
            .clone();
        let update_interval = self.config.update_interval;
        let process_metrics = self.process_metrics.clone();
        let container_metrics = self.container_metrics.clone();
        let readiness_fn = self.readiness_fn.clone();

        // Build the axum router with built-in + optional + custom routes
        let metrics_handle = handle.clone();
        let readiness_for_live = readiness_fn.clone();

        let mut app = axum::Router::new()
            .route(
                "/metrics",
                axum::routing::get(move || {
                    let h = metrics_handle.clone();
                    async move { h.render() }
                }),
            )
            .route(
                "/healthz",
                axum::routing::get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        r#"{"status":"alive"}"#,
                    )
                }),
            )
            .route(
                "/health/live",
                axum::routing::get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        r#"{"status":"alive"}"#,
                    )
                }),
            )
            .route(
                "/readyz",
                axum::routing::get(move || {
                    let rf = readiness_fn.clone();
                    async move { readiness_response(rf) }
                }),
            )
            .route(
                "/health/ready",
                axum::routing::get(move || {
                    let rf = readiness_for_live.clone();
                    async move { readiness_response(rf) }
                }),
            );

        // Add scaling pressure endpoint if configured
        #[cfg(feature = "scaling")]
        if let Some(ref sp) = self.scaling_pressure {
            let sp = sp.clone();
            app = app.route(
                "/scaling/pressure",
                axum::routing::get(move || {
                    let s = sp.clone();
                    async move { format!("{:.2}", s.calculate()) }
                }),
            );
        }

        // Add memory pressure endpoint if configured
        #[cfg(feature = "memory")]
        if let Some(ref mg) = self.memory_guard {
            let mg = mg.clone();
            app = app.route(
                "/memory/pressure",
                axum::routing::get(move || {
                    let m = mg.clone();
                    async move {
                        (
                            [(axum::http::header::CONTENT_TYPE, "application/json")],
                            format!(
                                r#"{{"under_pressure":{},"ratio":{:.3},"current_bytes":{},"limit_bytes":{}}}"#,
                                m.under_pressure(),
                                m.pressure_ratio(),
                                m.current_bytes(),
                                m.limit_bytes()
                            ),
                        )
                    }
                }),
            );
        }

        // Merge service-specific routes
        app = app.merge(extra_routes);

        tokio::spawn(async move {
            run_axum_server(
                listener,
                app,
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

    /// Gracefully shut down the OTel provider (flushes pending exports).
    ///
    /// Call this before application exit to ensure all metrics are exported.
    #[cfg(feature = "otel-metrics")]
    pub fn shutdown_otel(&mut self) {
        if let Some(provider) = self.otel_provider.take()
            && let Err(e) = provider.shutdown()
        {
            tracing::warn!(error = %e, "OTel provider shutdown error");
        }
    }

    /// Get the namespace prefix (e.g. `dfe_loader`).
    ///
    /// Used by [`dfe_groups`] metric structs to build labelled metric keys.
    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.config.namespace
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
#[cfg(feature = "metrics")]
async fn run_server(
    listener: TcpListener,
    handle: PrometheusHandle,
    mut shutdown_rx: oneshot::Receiver<()>,
    update_interval: Duration,
    process_metrics: Option<ProcessMetrics>,
    container_metrics: Option<ContainerMetrics>,
    readiness_fn: Option<ReadinessFn>,
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
                    let readiness_fn = readiness_fn.clone();
                    tokio::spawn(async move {
                        handle_connection(stream, handle, readiness_fn).await;
                    });
                }
            }
        }
    }
}

/// Handle a single HTTP connection.
#[cfg(feature = "metrics")]
async fn handle_connection(
    mut stream: tokio::net::TcpStream,
    handle: PrometheusHandle,
    readiness_fn: Option<ReadinessFn>,
) {
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
        let callback_ready = readiness_fn.as_ref().is_none_or(|f| f());

        #[cfg(feature = "health")]
        let registry_ready = crate::health::HealthRegistry::is_ready();
        #[cfg(not(feature = "health"))]
        let registry_ready = true;

        let ready = callback_ready && registry_ready;
        if ready {
            ("200 OK", r#"{"status":"ready"}"#.to_string())
        } else {
            (
                "503 Service Unavailable",
                r#"{"status":"not_ready"}"#.to_string(),
            )
        }
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

/// Readiness response helper for axum endpoints.
///
/// Checks the caller-supplied readiness callback AND (when the `health`
/// feature is enabled) the global [`HealthRegistry`](crate::health::HealthRegistry).
/// Both must be true for a 200 response.
#[cfg(all(feature = "metrics", feature = "http-server"))]
fn readiness_response(rf: Option<ReadinessFn>) -> axum::response::Response {
    use axum::response::IntoResponse;

    let callback_ready = rf.as_ref().is_none_or(|f| f());

    #[cfg(feature = "health")]
    let registry_ready = crate::health::HealthRegistry::is_ready();
    #[cfg(not(feature = "health"))]
    let registry_ready = true;

    let ready = callback_ready && registry_ready;
    if ready {
        (
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            r#"{"status":"ready"}"#,
        )
            .into_response()
    } else {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            r#"{"status":"not_ready"}"#,
        )
            .into_response()
    }
}

/// Run the axum-based metrics HTTP server with custom routes.
#[cfg(all(feature = "metrics", feature = "http-server"))]
async fn run_axum_server(
    listener: TcpListener,
    app: axum::Router,
    shutdown_rx: oneshot::Receiver<()>,
    update_interval: Duration,
    process_metrics: Option<ProcessMetrics>,
    container_metrics: Option<ContainerMetrics>,
) {
    let mut interval = tokio::time::interval(update_interval);

    // Spawn the metrics update loop
    let (update_stop_tx, mut update_stop_rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut update_stop_rx => break,
                _ = interval.tick() => {
                    if let Some(ref pm) = process_metrics {
                        pm.update();
                    }
                    if let Some(ref cm) = container_metrics {
                        cm.update();
                    }
                }
            }
        }
    });

    // Run axum server with graceful shutdown
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        })
        .await
        .unwrap_or_else(|e| tracing::error!(error = %e, "Metrics axum server error"));

    let _ = update_stop_tx.send(());
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
