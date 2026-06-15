// Project:   hyperi-rustlib
// File:      src/cli/runtime.rs
// Purpose:   ServiceRuntime -- pre-built infrastructure for DFE service apps
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Pre-built service infrastructure for DFE pipeline applications.
//!
//! [`ServiceRuntime`] is created by [`super::run_app`] before calling
//! [`DfeApp::run_service`]. Apps receive it fully wired -- eliminates
//! ~50 lines of identical boilerplate per DFE app.
//!
//! ## What's included (always)
//!
//! - [`MetricsManager`] -- started, serving `/metrics`, `/healthz`, `/readyz`
//! - [`DfeMetrics`] -- platform `dfe_*` metrics registered
//! - [`MemoryGuard`] -- cgroup-aware, auto-detected from env prefix
//! - [`CancellationToken`] -- signal handler installed with K8s pre-stop delay
//! - [`RuntimeContext`] -- K8s/Docker/BareMetal metadata
//!
//! ## What's included (when features enabled)
//!
//! - [`AdaptiveWorkerPool`] -- rayon + tokio hybrid (`worker` feature)
//! - [`ScalingPressure`] -- KEDA signals (`scaling` feature)
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
/// Apps receive this fully wired -- they just use the fields. No boilerplate needed.
///
/// On bare metal, K8s-specific features (pre-stop delay, pod metadata in logs)
/// are automatically disabled. On K8s, they're automatically enabled.
pub struct ServiceRuntime {
    /// Metrics manager -- already started, serving endpoints.
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

    /// Runtime context -- K8s/Docker/BareMetal metadata (pod_name, namespace, etc.).
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

    /// Horizontal scaling-pressure ENGINE (CEL over local, correlated metrics).
    /// Emits `{ns}_scaling_pressure{name}` per configured pressure plus the
    /// gratis compound `{ns}_transport_{inbound,outbound}_pressure_ratio` and
    /// `{ns}_scaling_circuit_open`. `None` unless both `scaling` + `expression`
    /// are enabled. Runs its own periodic tick (CPU sampled internally).
    #[cfg(all(feature = "scaling", feature = "expression"))]
    pub scaling_engine: Option<Arc<crate::scaling::ScalingEngine>>,

    /// Lock-free cell for pushing per-pod transport scaling signals (kafka
    /// assigned-lag, in-flight, shed rate, circuit, ...) that the
    /// `scaling_engine` reads each tick. Update it from
    /// your receive/send loops; CPU is sampled by the engine itself. When no
    /// signals are pushed, the smart default reduces to CPU-only (ACR F2).
    #[cfg(feature = "scaling")]
    pub scaling_signals: Arc<crate::scaling::ScalingSignalsCell>,

    /// Self-regulation governor (`governor` feature). Default-ON, opt-out via
    /// `self_regulation.enabled = false`. `None` when disabled -- nothing is
    /// constructed and the data path is byte-identical to pre-governor.
    ///
    /// Thread [`pressure`](crate::SelfRegulationGovernor::pressure) into your
    /// receive transports' inbound gate / `with_pressure` hooks. The
    /// [`budget`](crate::SelfRegulationGovernor::budget) is already wired into
    /// the [`batch_engine`](Self::batch_engine) governed run path.
    #[cfg(feature = "governor")]
    pub governor: Option<crate::SelfRegulationGovernor>,
}

impl ServiceRuntime {
    /// Build the service runtime from app configuration.
    ///
    /// This is called by `run_app()` -- apps don't call it directly.
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

        // --- Self-regulation governor (default-ON, opt-out) ---
        //
        // Constructed HERE -- before the worker pool, batch engine, and the
        // transports the app builds in run_service() -- so the shared pressure
        // and byte budget can be threaded into all of them. When
        // `self_regulation.enabled = false`, `build` returns None and nothing
        // is constructed: every downstream Option stays None and the data path
        // is byte-identical to pre-governor behaviour.
        #[cfg(feature = "governor")]
        let governor = crate::SelfRegulationConfig::from_cascade().build(Arc::clone(&memory_guard));

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
                // Wire the governor's byte-budget lever so the engine's governed
                // run path streams in budget-sized sub-blocks. None (governor
                // off) leaves the engine on the whole-batch loop.
                #[cfg(feature = "governor")]
                if let Some(ref gov) = governor {
                    engine.set_byte_budget(gov.budget());
                }
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

        // --- Horizontal scaling-pressure engine (CEL over local metrics) ---
        #[cfg(feature = "scaling")]
        let scaling_signals = Arc::new(crate::scaling::ScalingSignalsCell::new());

        #[cfg(all(feature = "scaling", feature = "expression"))]
        let scaling_engine = {
            let sp_cfg = crate::scaling::ScalingEngineConfig::from_cascade();
            let inbound = sp_cfg.transport.inbound.as_deref().map_or(
                crate::scaling::ScalingTransport::Other,
                crate::scaling::ScalingTransport::from_label,
            );
            let outbound = sp_cfg.transport.outbound.as_deref().map_or(
                crate::scaling::ScalingTransport::Other,
                crate::scaling::ScalingTransport::from_label,
            );
            let (engine, errors) =
                crate::scaling::ScalingEngine::new(app_name, &sp_cfg, inbound, outbound);
            for e in &errors {
                tracing::error!(target: "scaling", "{e}");
            }
            let engine = Arc::new(engine);
            if engine.is_enabled() {
                tokio::spawn(run_scaling_pressure_loop(
                    Arc::clone(&engine),
                    Arc::clone(&scaling_signals),
                    sp_cfg.interval_secs,
                    #[cfg(feature = "memory")]
                    Arc::clone(&memory_guard),
                    shutdown.clone(),
                ));
            }
            Some(engine)
        };

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

        // Turns the previously-silent "from_cascade defaulted everything"
        // failure into one observable startup line.
        #[cfg(feature = "config")]
        log_cascade_section_summary();

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
            #[cfg(all(feature = "scaling", feature = "expression"))]
            scaling_engine,
            #[cfg(feature = "scaling")]
            scaling_signals,
            #[cfg(feature = "governor")]
            governor,
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

    /// Build a governed receive transport from config in ONE call
    /// (`governor` + `transport` features).
    ///
    /// Reads the transport config at `key` and threads the runtime's
    /// [`governor`](Self::governor) pressure into the receiver's inbound brake
    /// (Kafka pause-partitions gate, HTTP/gRPC 503/`unavailable` shed) so apps
    /// skip the `gate_actuator -> InboundGate -> with_inbound_gate` dance.
    ///
    /// When the governor is disabled (`self_regulation.enabled = false`,
    /// [`governor`](Self::governor) is `None`) this falls back to the plain
    /// [`AnyReceiver::from_config`](crate::transport::factory::AnyReceiver::from_config)
    /// -- data path stays byte-identical to pre-governor.
    ///
    /// # Errors
    ///
    /// Returns the underlying transport error if the config is missing/invalid
    /// or the backend fails to construct.
    #[cfg(all(feature = "governor", feature = "transport"))]
    pub async fn governed_receiver(
        &self,
        key: &str,
    ) -> Result<crate::transport::factory::AnyReceiver, crate::transport::TransportError> {
        use crate::transport::factory::AnyReceiver;
        match self.governor {
            Some(ref gov) => AnyReceiver::from_config_with_governor(key, gov).await,
            None => AnyReceiver::from_config(key).await,
        }
    }
}

/// Emit one startup line summarising which platform config sections were found
/// in the cascade vs defaulted. Cheap (key-presence checks, no deserialisation).
/// This is the observable counterpart to the silent pre-2.8.11 failure where
/// `from_cascade` defaulted everything because the cascade was never populated.
#[cfg(feature = "config")]
fn log_cascade_section_summary() {
    let cfg = crate::config::try_get();
    let present = |key: &str| cfg.is_some_and(|c| c.contains(key));
    tracing::info!(
        cascade_initialised = cfg.is_some(),
        self_regulation = present("self_regulation"),
        worker_pool = present("worker_pool"),
        batch_processing = present("batch_processing"),
        scaling = present("scaling"),
        expression = present("expression"),
        "Config cascade sections (true = found in config, false = using defaults)"
    );
}

/// Periodic scaling-pressure tick: sample CPU (rate of the cumulative counter
/// over the wall window / cores), read the pushed transport signals, and let the
/// engine evaluate + publish its gauges. Off the data hot-path (interval-driven).
#[cfg(all(feature = "scaling", feature = "expression"))]
async fn run_scaling_pressure_loop(
    engine: Arc<crate::scaling::ScalingEngine>,
    signals: Arc<crate::scaling::ScalingSignalsCell>,
    interval_secs: u64,
    #[cfg(feature = "memory")] memory_guard: Arc<MemoryGuard>,
    shutdown: CancellationToken,
) {
    use std::time::{Duration, Instant};

    // CPU utilisation denominator: the cgroup CPU limit, else the visible core
    // count. Never 0.
    let cores = crate::metrics::cpu_limit_cores()
        .or_else(|| {
            std::thread::available_parallelism()
                .ok()
                .map(|n| n.get() as f64)
        })
        .filter(|c| *c > 0.0)
        .unwrap_or(1.0);

    let mut last_cpu = crate::metrics::cumulative_cpu_seconds();
    let mut last_at = Instant::now();
    let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs.max(1)));
    // tokio's first interval tick fires immediately -- consume it so the first
    // real sample below spans a full interval (no divide-by-near-zero CPU spike).
    ticker.tick().await;

    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            _ = ticker.tick() => {
                let now_cpu = crate::metrics::cumulative_cpu_seconds();
                let now_at = Instant::now();
                let cpu_ratio = match (last_cpu, now_cpu) {
                    (Some(prev), Some(cur)) => {
                        let elapsed = now_at.duration_since(last_at).as_secs_f64().max(1e-3);
                        // (cur - prev) can go negative on a counter reset -> floor 0.
                        ((cur - prev).max(0.0) / elapsed) / cores
                    }
                    _ => 0.0,
                };
                last_cpu = now_cpu;
                last_at = now_at;

                #[cfg(feature = "memory")]
                let memory_ratio = memory_guard.pressure_ratio();
                #[cfg(not(feature = "memory"))]
                let memory_ratio = 0.0;

                engine.tick(&signals.snapshot(), cpu_ratio, memory_ratio);
            }
        }
    }

    tracing::info!(target: "scaling", "Scaling-pressure loop shutting down");
}
