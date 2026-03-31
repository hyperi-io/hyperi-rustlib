// Project:   hyperi-rustlib
// File:      src/worker/pool.rs
// Purpose:   Rayon pool + semaphore management, process_batch(), fan_out_async()
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use parking_lot::RwLock;
use rayon::ThreadPool;

use super::config::WorkerPoolConfig;

/// Adaptive worker pool with hybrid rayon (CPU) + tokio (async I/O) execution.
///
/// Provides two APIs:
/// - [`process_batch`](Self::process_batch) — CPU-bound work via rayon
///   (JSON parsing, transforms, compression, CEL evaluation)
/// - [`fan_out_async`](Self::fan_out_async) — async I/O via tokio
///   (enrichment, external APIs, storage writes)
///
/// The pool auto-scales active threads based on CPU/memory pressure using
/// watermark bands. All thresholds are config-cascade overridable and emitted
/// as gauge metrics.
pub struct AdaptiveWorkerPool {
    pub(crate) config: Arc<RwLock<WorkerPoolConfig>>,
    rayon_pool: ThreadPool,
    pub(crate) semaphore: Arc<Semaphore>,
    #[cfg(feature = "memory")]
    pub(crate) memory_guard: parking_lot::Mutex<Option<Arc<crate::memory::MemoryGuard>>>,
    #[cfg(feature = "scaling")]
    pub(crate) scaling_pressure: parking_lot::Mutex<Option<Arc<crate::scaling::ScalingPressure>>>,
}

/// Counting semaphore for throttling rayon thread usage.
///
/// Rayon pools cannot be resized, so we use a semaphore to control how many
/// threads actively pick up work. Threads that cannot acquire a permit sleep
/// on [`std::thread::yield_now`].
pub(crate) struct Semaphore {
    permits: AtomicUsize,
    max_permits: usize,
}

impl Semaphore {
    fn new(initial_permits: usize, max_permits: usize) -> Self {
        Self {
            permits: AtomicUsize::new(initial_permits),
            max_permits,
        }
    }

    /// Acquire a permit (blocking). Returns a guard that releases on drop.
    fn acquire(&self) -> SemaphoreGuard<'_> {
        let start = Instant::now();
        loop {
            let current = self.permits.load(Ordering::Acquire);
            if current > 0
                && self
                    .permits
                    .compare_exchange_weak(
                        current,
                        current - 1,
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    )
                    .is_ok()
            {
                return SemaphoreGuard {
                    semaphore: self,
                    wait_duration: start.elapsed(),
                };
            }
            std::thread::yield_now();
        }
    }

    /// Set the number of available permits (called by scaler).
    pub(crate) fn set_permits(&self, count: usize) {
        let clamped = count.min(self.max_permits);
        self.permits.store(clamped, Ordering::Release);
    }

    /// Current number of available (unacquired) permits.
    pub(crate) fn available_permits(&self) -> usize {
        self.permits.load(Ordering::Relaxed)
    }
}

struct SemaphoreGuard<'a> {
    semaphore: &'a Semaphore,
    #[allow(dead_code)]
    wait_duration: std::time::Duration,
}

impl Drop for SemaphoreGuard<'_> {
    fn drop(&mut self) {
        self.semaphore.permits.fetch_add(1, Ordering::Release);
    }
}

impl AdaptiveWorkerPool {
    /// Create a new worker pool with the given configuration.
    ///
    /// Resolves `max_threads = 0` to the detected CPU count.
    /// Creates a fixed rayon thread pool and a semaphore starting at `min_threads`.
    #[must_use]
    pub fn new(config: WorkerPoolConfig) -> Self {
        let mut resolved = config;
        resolved.resolve_max_threads();

        let max_threads = resolved.max_threads;
        let min_threads = resolved.min_threads;

        let rayon_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(max_threads)
            .thread_name(|i| format!("worker-{i}"))
            .build()
            .expect("Failed to create rayon thread pool");

        let semaphore = Arc::new(Semaphore::new(min_threads, max_threads));

        Self {
            config: Arc::new(RwLock::new(resolved)),
            rayon_pool,
            semaphore,
            #[cfg(feature = "memory")]
            memory_guard: parking_lot::Mutex::new(None),
            #[cfg(feature = "scaling")]
            scaling_pressure: parking_lot::Mutex::new(None),
        }
    }

    /// Create a new worker pool from the config cascade.
    ///
    /// # Errors
    ///
    /// Returns an error if the config cascade is not initialised or validation fails.
    pub fn from_cascade(key: &str) -> Result<Self, crate::config::ConfigError> {
        let config = WorkerPoolConfig::from_cascade(key)?;
        Ok(Self::new(config))
    }

    /// Process a batch of items in parallel using rayon (CPU-bound work).
    ///
    /// Each item is processed by the provided closure on a rayon worker thread.
    /// A semaphore limits how many threads are active simultaneously (controlled
    /// by the scaling controller). Results are returned in input order.
    ///
    /// Use this for: JSON parsing, transforms, compression, CEL evaluation, routing.
    /// Do NOT use for work that needs `.await` — use [`fan_out_async`](Self::fan_out_async).
    pub fn process_batch<T, R, E, F>(&self, items: &[T], f: F) -> Vec<Result<R, E>>
    where
        T: Sync,
        R: Send,
        E: Send,
        F: Fn(&T) -> Result<R, E> + Sync,
    {
        let sem = &self.semaphore;
        self.rayon_pool.install(|| {
            use rayon::prelude::*;
            items
                .par_iter()
                .map(|item| {
                    let _permit = sem.acquire();
                    f(item)
                })
                .collect()
        })
    }

    /// Fan out async work across tokio tasks with bounded concurrency.
    ///
    /// Each item is processed by the provided async closure on a tokio task.
    /// Concurrency is limited by `async_concurrency` config. Results are
    /// returned in input order (guaranteed via index tracking).
    ///
    /// Use this for: enrichment lookups, external API calls, storage writes.
    pub async fn fan_out_async<T, R, E, F, Fut>(&self, items: &[T], f: F) -> Vec<Result<R, E>>
    where
        T: Sync + Send,
        R: Send + 'static,
        E: Send + 'static,
        F: Fn(&T) -> Fut + Send + Sync,
        Fut: std::future::Future<Output = Result<R, E>> + Send + 'static,
    {
        let concurrency = self.config.read().async_concurrency;
        let mut results: Vec<Option<Result<R, E>>> = (0..items.len()).map(|_| None).collect();

        // Process in chunks of `concurrency` to limit in-flight tasks
        for chunk_start in (0..items.len()).step_by(concurrency) {
            let chunk_end = (chunk_start + concurrency).min(items.len());
            let mut handles = Vec::with_capacity(chunk_end - chunk_start);

            for idx in chunk_start..chunk_end {
                let fut = f(&items[idx]);
                handles.push((idx, tokio::spawn(fut)));
            }

            for (idx, handle) in handles {
                match handle.await {
                    Ok(result) => results[idx] = Some(result),
                    Err(join_err) => {
                        tracing::error!(error = %join_err, idx, "fan_out_async task panicked");
                    }
                }
            }
        }

        results.into_iter().flatten().collect()
    }

    /// Register worker pool metrics with the `MetricsManager`.
    ///
    /// Registers operational metrics and emits threshold gauges with current values.
    /// Call this once during startup after creating the pool.
    pub fn register_metrics(&self, manager: &crate::metrics::MetricsManager) {
        let config = self.config.read();
        super::metrics::register(manager, &config);
    }

    /// Start the background scaling controller.
    ///
    /// The controller samples CPU/memory every `scale_interval_secs` and adjusts
    /// the semaphore permits based on watermark bands. Stops on cancellation.
    pub fn start_scaling_loop(self: &Arc<Self>, cancel: tokio_util::sync::CancellationToken) {
        let controller = super::scaler::ScalingController::new(self.clone());
        tokio::spawn(controller.run(cancel));
    }

    /// Attach a `MemoryGuard` for dual-source memory pressure reading.
    #[cfg(feature = "memory")]
    pub fn set_memory_guard(&self, guard: Arc<crate::memory::MemoryGuard>) {
        *self.memory_guard.lock() = Some(guard);
    }

    /// Attach a `ScalingPressure` for bidirectional pressure integration.
    #[cfg(feature = "scaling")]
    pub fn set_scaling_pressure(&self, pressure: Arc<crate::scaling::ScalingPressure>) {
        *self.scaling_pressure.lock() = Some(pressure);
    }

    /// Current number of active worker threads (permits in use).
    pub fn active_threads(&self) -> usize {
        let cfg = self.config.read();
        cfg.max_threads
            .saturating_sub(self.semaphore.available_permits())
    }

    /// Maximum thread count (pool size).
    pub fn max_threads(&self) -> usize {
        self.config.read().max_threads
    }
}
