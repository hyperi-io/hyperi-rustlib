// Project:   hyperi-rustlib
// File:      src/cli/runtime.rs
// Purpose:   ServiceRuntime — pre-built infrastructure for DFE service apps
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Pre-built service infrastructure for DFE pipeline applications.
//!
//! [`ServiceRuntime`] is created by [`super::run_app`] before calling
//! [`DfeApp::run_service`]. It contains all the common infrastructure
//! that every DFE app needs — metrics, memory guard, scaling, shutdown,
//! worker pool, and runtime context. Apps receive it fully wired.
//!
//! This eliminates ~50 lines of identical boilerplate per DFE app.
//!
//! ## What's included (always)
//!
//! - [`MetricsManager`] — started, serving `/metrics`, `/healthz`, `/readyz`
//! - [`DfeMetrics`] — platform `dfe_*` metrics registered
//! - [`MemoryGuard`] — cgroup-aware, auto-detected from env prefix
//! - [`CancellationToken`] — signal handler installed with K8s pre-stop delay
//! - [`RuntimeContext`] — K8s/Docker/BareMetal metadata
//!
//! ## What's included (when features enabled)
//!
//! - [`AdaptiveWorkerPool`] — rayon + tokio hybrid (`worker` feature)
//! - [`ScalingPressure`] — KEDA signals (`scaling` feature)
//!
//! ## What stays app-specific
//!
//! - Readiness check criteria (each app defines "ready" differently)
//! - Config hot-reload (optional, app-specific reload logic)
//! - Pipeline creation (100% domain-specific)
//! - DLQ setup (varies per app)
//! - App-specific metric groups (ConsumerMetrics, BufferMetrics, etc.)

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::env::{RuntimeContext, runtime_context};
#[cfg(feature = "memory")]
use crate::memory::{MemoryGuard, MemoryGuardConfig};
use crate::metrics::MetricsManager;

use super::error::CliError;

/// Pre-built service infrastructure. Created by `run_app()` before `run_service()`.
///
/// Apps receive this fully wired — they just use the fields. No boilerplate needed.
///
/// On bare metal, K8s-specific features (pre-stop delay, pod metadata in logs)
/// are automatically disabled. On K8s, they're automatically enabled.
pub struct ServiceRuntime {
    /// Metrics manager — already started, serving endpoints.
    /// Use for registering app-specific metrics and metric groups.
    pub metrics: MetricsManager,

    /// Platform DFE metrics (`dfe_*` counters/gauges). Already registered.
    pub dfe: Arc<crate::metrics::DfeMetrics>,

    /// Cgroup-aware memory guard. Tracks memory usage for backpressure.
    /// Auto-detected from env prefix + cgroup limits.
    #[cfg(feature = "memory")]
    pub memory_guard: Arc<MemoryGuard>,

    /// Shutdown token. Cancelled on SIGTERM/SIGINT (with K8s pre-stop delay).
    /// Clone and pass to your pipeline loops.
    pub shutdown: CancellationToken,

    /// Runtime context — K8s/Docker/BareMetal metadata (pod_name, namespace, etc.).
    pub context: &'static RuntimeContext,

    /// Adaptive worker pool for parallel batch processing (`worker` feature).
    /// `None` if the `worker` feature is not enabled or config fails.
    #[cfg(feature = "worker-pool")]
    pub worker_pool: Option<Arc<crate::worker::AdaptiveWorkerPool>>,

    /// Batch processing engine with SIMD parsing and pre-route filtering
    /// (`worker-batch` feature). `None` if `worker-batch` is not enabled
    /// or worker pool creation failed.
    #[cfg(feature = "worker-batch")]
    pub batch_engine: Option<Arc<crate::worker::BatchEngine>>,

    /// Scaling pressure calculator for KEDA autoscaling (`scaling` feature).
    /// `None` if the `scaling` feature is not enabled.
    #[cfg(feature = "scaling")]
    pub scaling: Option<Arc<crate::ScalingPressure>>,
}

impl ServiceRuntime {
    /// Build the service runtime from app configuration.
    ///
    /// This is called by `run_app()` — apps don't call it directly.
    ///
    /// # Errors
    ///
    /// Returns `CliError` if the metrics server fails to start.
    pub(crate) async fn build(
        app_name: &str,
        env_prefix: &str,
        metrics_addr: &str,
        #[cfg_attr(not(feature = "metrics-dfe"), allow(unused_variables))] version: &str,
        #[cfg_attr(not(feature = "metrics-dfe"), allow(unused_variables))] commit: &str,
        #[cfg(feature = "scaling")] scaling_components: Vec<crate::ScalingComponent>,
    ) -> Result<Self, CliError> {
        let ctx = runtime_context();

        // --- Metrics ---
        let mut metrics = MetricsManager::new(app_name);
        let dfe = Arc::new(crate::metrics::DfeMetrics::register(&metrics));

        // App info metric (version, commit, service name)
        #[cfg(feature = "metrics-dfe")]
        {
            let _app_metrics =
                crate::metrics::dfe_groups::AppMetrics::new(&metrics, version, commit);
        }

        // --- Memory guard ---
        #[cfg(feature = "memory")]
        let memory_guard = Arc::new(MemoryGuard::new(MemoryGuardConfig::from_env(env_prefix)));

        // --- Scaling pressure ---
        #[cfg(feature = "scaling")]
        let scaling = {
            let config = crate::ScalingPressureConfig::from_cascade();
            let pressure = Arc::new(crate::ScalingPressure::new(config, scaling_components));
            metrics.set_scaling_pressure(Arc::clone(&pressure));
            Some(pressure)
        };

        // --- Worker pool ---
        #[cfg(feature = "worker-pool")]
        let worker_pool = {
            match crate::worker::AdaptiveWorkerPool::from_cascade("worker_pool") {
                Ok(pool) => {
                    let pool = Arc::new(pool);
                    pool.register_metrics(&metrics);
                    #[cfg(feature = "memory")]
                    pool.set_memory_guard(Arc::clone(&memory_guard));
                    #[cfg(feature = "scaling")]
                    if let Some(ref sp) = scaling {
                        pool.set_scaling_pressure(Arc::clone(sp));
                    }
                    tracing::info!(
                        max_threads = pool.max_threads(),
                        "Adaptive worker pool enabled"
                    );
                    Some(pool)
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Worker pool not configured, falling back to sequential"
                    );
                    None
                }
            }
        };

        // --- Batch engine (worker-batch tier only) ---
        #[cfg(feature = "worker-batch")]
        let batch_engine = {
            if let Some(ref pool) = worker_pool {
                let config =
                    crate::worker::engine::BatchProcessingConfig::from_cascade("batch_processing")
                        .unwrap_or_default();
                let mut engine = crate::worker::BatchEngine::with_pool(Arc::clone(pool), config);
                engine.auto_wire(
                    &metrics,
                    #[cfg(feature = "memory")]
                    Some(&memory_guard),
                );
                Some(Arc::new(engine))
            } else {
                None
            }
        };

        // --- Shutdown ---
        let shutdown = crate::shutdown::install_signal_handler();

        // Start worker pool scaling loop after shutdown token exists
        #[cfg(feature = "worker-pool")]
        if let Some(ref pool) = worker_pool {
            pool.start_scaling_loop(shutdown.clone());
        }

        // --- Start metrics server ---
        if let Err(e) = metrics.start_server(metrics_addr).await {
            tracing::error!(error = %e, addr = metrics_addr, "Failed to start metrics server");
        }

        // --- Version check (fire-and-forget) ---
        #[cfg(feature = "version-check")]
        {
            crate::VersionCheck::new(crate::VersionCheckConfig {
                product: app_name.to_string(),
                current_version: version.to_string(),
                ..Default::default()
            })
            .check_on_startup();
        }

        // Log runtime context
        tracing::info!(
            environment = %ctx.environment,
            pod_name = ?ctx.pod_name,
            namespace = ?ctx.namespace,
            "Service runtime initialised"
        );

        Ok(Self {
            metrics,
            dfe,
            #[cfg(feature = "memory")]
            memory_guard,
            shutdown,
            context: ctx,
            #[cfg(feature = "worker-pool")]
            worker_pool,
            #[cfg(feature = "worker-batch")]
            batch_engine,
            #[cfg(feature = "scaling")]
            scaling,
        })
    }

    /// Set the readiness check callback.
    ///
    /// Each app defines its own readiness criteria. Call this in `run_service()`
    /// once you know what "ready" means for your app.
    pub fn set_readiness_check<F: Fn() -> bool + Send + Sync + 'static>(&mut self, check: F) {
        self.metrics.set_readiness_check(check);
    }

    /// Return the batch processing engine, if the `worker-batch` feature is
    /// enabled and the worker pool was successfully created.
    #[cfg(feature = "worker-batch")]
    #[must_use]
    pub fn batch_engine(&self) -> Option<&Arc<crate::worker::BatchEngine>> {
        self.batch_engine.as_ref()
    }
}
