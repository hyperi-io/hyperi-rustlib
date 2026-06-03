// Project:   hyperi-rustlib
// File:      src/worker/pool.rs
// Purpose:   Rayon pool + semaphore management, process_batch(), fan_out_async()
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

use std::sync::Arc;

use parking_lot::{Condvar, Mutex, RwLock};
use rayon::ThreadPool;

use super::config::WorkerPoolConfig;

/// Adaptive worker pool with hybrid rayon (CPU) + tokio (async I/O) execution.
///
/// Provides two APIs:
/// - [`process_batch`](Self::process_batch) -- CPU-bound work via rayon
///   (JSON parsing, transforms, compression, CEL evaluation)
/// - [`fan_out_async`](Self::fan_out_async) -- async I/O via tokio
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

/// Concurrency limiter for throttling rayon thread usage.
///
/// Rayon pools cannot be resized, so a limiter controls how many threads
/// actively pick up work. Models explicit `target` (the scaler's desired
/// concurrency) and `leased` (permits currently held by in-flight work);
/// available headroom is the derived `target - leased`. A thread that cannot
/// lease (`leased >= target`) PARKS on a condvar -- it does not spin -- so the
/// throttle conserves CPU exactly when the scaler is trying to.
///
/// Why this shape (vs the old `available`/`max_permits` atomic): the scaler
/// sets a *target*, and `active_threads()` reports *leased* (true in-flight),
/// so an idle pool reports zero active and a downscale cannot be undone by
/// guard drops refilling toward `max` -- drops only decrement `leased`, and
/// no new lease is admitted while `leased >= target`.
pub(crate) struct Semaphore {
    state: Mutex<SemState>,
    /// Signalled when a permit frees or the target grows.
    available: Condvar,
    /// Architectural ceiling (rayon pool size); `target` never exceeds it.
    max_permits: usize,
}

struct SemState {
    /// Scaler-controlled desired concurrency, kept in `[1, max_permits]`.
    target: usize,
    /// Permits currently held by in-flight work.
    leased: usize,
}

impl Semaphore {
    fn new(initial_target: usize, max_permits: usize) -> Self {
        let max_permits = max_permits.max(1);
        Self {
            state: Mutex::new(SemState {
                target: initial_target.clamp(1, max_permits),
                leased: 0,
            }),
            available: Condvar::new(),
            max_permits,
        }
    }

    /// Lease a permit, parking until `leased < target`. Releases on drop.
    fn acquire(&self) -> SemaphoreGuard<'_> {
        let mut st = self.state.lock();
        while st.leased >= st.target {
            self.available.wait(&mut st);
        }
        st.leased += 1;
        SemaphoreGuard { semaphore: self }
    }

    /// Set the target concurrency (called by the scaler). Clamped to
    /// `[1, max_permits]`. Growing the target wakes parked acquirers so they
    /// re-check; shrinking simply stops new leases until `leased` falls below
    /// the new target -- in-flight work drains naturally.
    pub(crate) fn set_target(&self, target: usize) {
        let clamped = target.clamp(1, self.max_permits);
        let mut st = self.state.lock();
        let grew = clamped > st.target;
        st.target = clamped;
        drop(st);
        if grew {
            self.available.notify_all();
        }
    }

    /// Current target concurrency.
    pub(crate) fn target(&self) -> usize {
        self.state.lock().target
    }

    /// Permits currently leased (in-flight work).
    pub(crate) fn leased(&self) -> usize {
        self.state.lock().leased
    }

    /// Headroom: how many more permits can be leased right now.
    pub(crate) fn available(&self) -> usize {
        let st = self.state.lock();
        st.target.saturating_sub(st.leased)
    }
}

struct SemaphoreGuard<'a> {
    semaphore: &'a Semaphore,
}

impl Drop for SemaphoreGuard<'_> {
    fn drop(&mut self) {
        let mut st = self.semaphore.state.lock();
        st.leased = st.leased.saturating_sub(1);
        drop(st);
        // Wake one parked acquirer; the freed permit is now leasable
        // (subject to the current target, which it re-checks).
        self.semaphore.available.notify_one();
    }
}

/// Policy for [`AdaptiveWorkerPool::fan_out_async_with_policy`].
///
/// The plain [`fan_out_async`](AdaptiveWorkerPool::fan_out_async) helper has no
/// timeout or cancellation, and a single hung future stalls its whole chunk. At
/// fleet scale, user code eventually passes a future that ignores deadlines, so
/// reusable external-I/O fan-out needs a deadline + cancellation contract
/// (Codex review 2026-06-03).
#[derive(Debug, Clone, Default)]
pub struct FanOutPolicy {
    /// Per-item timeout. `None` = no timeout (caller must bound the future).
    pub per_item_timeout: Option<std::time::Duration>,
    /// Cancellation token. When cancelled, no NEW items are spawned; in-flight
    /// items are still awaited. Remaining unspawned items report `Cancelled`.
    pub cancel: Option<tokio_util::sync::CancellationToken>,
}

/// Per-item outcome from [`AdaptiveWorkerPool::fan_out_async_with_policy`].
#[derive(Debug)]
pub enum FanOutResult<R, E> {
    /// The future completed with `Ok`.
    Ok(R),
    /// The future completed with `Err`.
    Err(E),
    /// The future exceeded `per_item_timeout`.
    TimedOut,
    /// The task panicked (logged at `error`).
    Panicked,
    /// Not spawned because the cancellation token fired first.
    Cancelled,
}

impl AdaptiveWorkerPool {
    /// Create a new worker pool with the given configuration, validating it.
    ///
    /// Resolves `max_threads = 0` to the detected CPU count, then validates the
    /// resolved config (rejects `min_threads > max_threads`, bad watermark
    /// ordering, zero `async_concurrency`, etc.) before building. This prevents
    /// invalid runtime state that would otherwise panic later in the scaler's
    /// `clamp(min, max)` (Codex review 2026-06-03).
    ///
    /// # Errors
    ///
    /// Returns `ConfigError` if the resolved config is invalid.
    pub fn try_new(config: WorkerPoolConfig) -> Result<Self, crate::config::ConfigError> {
        let mut resolved = config;
        resolved.resolve_max_threads();
        resolved.validate()?;
        Ok(Self::build(resolved))
    }

    /// Create a new worker pool with the given configuration.
    ///
    /// Resolves `max_threads = 0` to the detected CPU count.
    ///
    /// # Panics
    ///
    /// Panics immediately with a clear message if the config is invalid (e.g.
    /// `min_threads > max_threads`). Use [`try_new`](Self::try_new) for fallible
    /// construction from untrusted config.
    #[must_use]
    pub fn new(config: WorkerPoolConfig) -> Self {
        Self::try_new(config).expect("invalid WorkerPoolConfig (use try_new to handle the error)")
    }

    /// Build the pool from an already-resolved, validated config.
    fn build(resolved: WorkerPoolConfig) -> Self {
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
        Self::try_new(config)
    }

    /// Process a batch of items in parallel using rayon (CPU-bound work).
    ///
    /// Each item is processed by the provided closure on a rayon worker thread.
    /// A semaphore limits how many threads are active simultaneously (controlled
    /// by the scaling controller). Results are returned in input order.
    ///
    /// Use this for: JSON parsing, transforms, compression, CEL evaluation, routing.
    /// Do NOT use for work that needs `.await` -- use [`fan_out_async`](Self::fan_out_async).
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
    /// Concurrency is limited by `async_concurrency` config.
    ///
    /// # Return contract
    ///
    /// The returned `Vec` has the same length as `items` and entries
    /// correspond by index (input-order preserved):
    ///
    /// - `Some(Ok(r))` -- task completed successfully with result `r`
    /// - `Some(Err(e))` -- task returned `Err(e)`
    /// - `None` -- task panicked; the panic was logged at `error` level
    ///   with the input index. The wrapping `Option` exists so the
    ///   panic doesn't silently shorten the result vector (which was
    ///   the previous behaviour and violated the input-order contract).
    ///
    /// Use this for: enrichment lookups, external API calls, storage writes.
    pub async fn fan_out_async<T, R, E, F, Fut>(
        &self,
        items: &[T],
        f: F,
    ) -> Vec<Option<Result<R, E>>>
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

            for (idx, item) in items
                .iter()
                .enumerate()
                .skip(chunk_start)
                .take(chunk_end - chunk_start)
            {
                let fut = f(item);
                handles.push((idx, tokio::spawn(fut)));
            }

            for (idx, handle) in handles {
                match handle.await {
                    Ok(result) => results[idx] = Some(result),
                    Err(join_err) => {
                        // Leave results[idx] = None; caller can detect
                        // the panic without shrinking the output vec.
                        tracing::error!(error = %join_err, idx, "fan_out_async task panicked");
                    }
                }
            }
        }

        results
    }

    /// Concurrency-bounded async fan-out with per-item timeout + cancellation.
    ///
    /// Unlike [`fan_out_async`](Self::fan_out_async), this streams completions
    /// (a `JoinSet`) so a slow item never blocks faster ones, applies an
    /// optional per-item timeout, and stops spawning new work when the policy's
    /// cancellation token fires. Results are returned in input order; each is a
    /// [`FanOutResult`]. Emits `dfe_fanout_*` metrics (in-flight gauge, timeout/
    /// panic counters, batch-duration histogram) when the `metrics` feature is on.
    pub async fn fan_out_async_with_policy<T, R, E, F, Fut>(
        &self,
        items: &[T],
        policy: &FanOutPolicy,
        f: F,
    ) -> Vec<FanOutResult<R, E>>
    where
        T: Sync + Send,
        R: Send + 'static,
        E: Send + 'static,
        F: Fn(&T) -> Fut + Send + Sync,
        Fut: std::future::Future<Output = Result<R, E>> + Send + 'static,
    {
        let concurrency = self.config.read().async_concurrency.max(1);
        let mut results: Vec<FanOutResult<R, E>> =
            (0..items.len()).map(|_| FanOutResult::Cancelled).collect();

        #[cfg(feature = "metrics")]
        let started = std::time::Instant::now();

        let mut set: tokio::task::JoinSet<(usize, FanOutResult<R, E>)> =
            tokio::task::JoinSet::new();
        // task Id -> input idx, so a panicked task (JoinError carries no payload)
        // still maps back to its slot.
        let mut id_to_idx: std::collections::HashMap<tokio::task::Id, usize> =
            std::collections::HashMap::new();
        let mut next = 0;
        let cancelled = || policy.cancel.as_ref().is_some_and(|c| c.is_cancelled());

        loop {
            // Seed up to the concurrency limit (unless cancelled).
            while set.len() < concurrency && next < items.len() && !cancelled() {
                let fut = f(&items[next]);
                let timeout = policy.per_item_timeout;
                let idx = next;
                let handle = set.spawn(async move {
                    let outcome = match timeout {
                        Some(d) => match tokio::time::timeout(d, fut).await {
                            Ok(Ok(r)) => FanOutResult::Ok(r),
                            Ok(Err(e)) => FanOutResult::Err(e),
                            Err(_) => FanOutResult::TimedOut,
                        },
                        None => match fut.await {
                            Ok(r) => FanOutResult::Ok(r),
                            Err(e) => FanOutResult::Err(e),
                        },
                    };
                    (idx, outcome)
                });
                id_to_idx.insert(handle.id(), idx);
                next += 1;
            }

            #[cfg(feature = "metrics")]
            ::metrics::gauge!("dfe_fanout_inflight").set(set.len() as f64);

            let Some(joined) = set.join_next().await else {
                break; // nothing in flight -- done (or cancelled with none left)
            };
            match joined {
                Ok((idx, outcome)) => {
                    #[cfg(feature = "metrics")]
                    if matches!(outcome, FanOutResult::TimedOut) {
                        ::metrics::counter!("dfe_fanout_timeout_total").increment(1);
                    }
                    results[idx] = outcome;
                }
                Err(join_err) => {
                    // Panicked task -- map its task Id back to the input slot.
                    if let Some(&idx) = id_to_idx.get(&join_err.id()) {
                        results[idx] = FanOutResult::Panicked;
                    }
                    #[cfg(feature = "metrics")]
                    ::metrics::counter!("dfe_fanout_panic_total").increment(1);
                    tracing::error!(error = %join_err, "fan_out_async_with_policy task panicked");
                }
            }

            // If cancelled and nothing left in flight, stop (remaining stay Cancelled).
            if cancelled() && set.is_empty() {
                break;
            }
        }

        #[cfg(feature = "metrics")]
        {
            ::metrics::gauge!("dfe_fanout_inflight").set(0.0);
            ::metrics::histogram!("dfe_fanout_batch_duration_seconds")
                .record(started.elapsed().as_secs_f64());
        }

        results
    }

    /// Map owned items in parallel under the concurrency limiter.
    ///
    /// Like [`process_batch`](Self::process_batch) but takes OWNED items and a
    /// closure that consumes each (so the transform may mutate its own copy),
    /// returning an arbitrary `R` per item. Crucially the semaphore IS applied
    /// per item, so this path obeys the scaler target -- unlike
    /// [`install`](Self::install). Results are collected in input order.
    ///
    /// Used by `BatchEngine`'s parsed mid-tier transform so the parsed path is
    /// throttled identically to the raw path.
    pub fn map_owned<T, R, F>(&self, items: Vec<T>, f: F) -> Vec<R>
    where
        T: Send,
        R: Send,
        F: Fn(T) -> R + Sync,
    {
        let sem = &self.semaphore;
        self.rayon_pool.install(|| {
            use rayon::prelude::*;
            items
                .into_par_iter()
                .map(|item| {
                    let _permit = sem.acquire();
                    f(item)
                })
                .collect()
        })
    }

    /// Execute a closure on the rayon thread pool.
    ///
    /// Provides direct access to the rayon pool for operations that need
    /// `par_iter_mut` or other rayon primitives not covered by the throttled
    /// helpers. The semaphore is NOT applied -- callers manage their own
    /// concurrency. Prefer [`process_batch`](Self::process_batch) or
    /// [`map_owned`](Self::map_owned), which respect the scaler target; reach
    /// for `install` only for genuine rayon-primitive escape hatches.
    pub fn install<R: Send>(&self, f: impl FnOnce() -> R + Send) -> R {
        self.rayon_pool.install(f)
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

    /// Number of permits currently leased -- true in-flight worker count.
    ///
    /// An idle pool reports 0 regardless of the scaler target. This is the
    /// telemetry-grade "active" count (vs [`target_threads`](Self::target_threads),
    /// the scaler's desired ceiling).
    #[must_use]
    pub fn active_threads(&self) -> usize {
        self.semaphore.leased()
    }

    /// Current scaler target concurrency (the admission ceiling).
    #[must_use]
    pub fn target_threads(&self) -> usize {
        self.semaphore.target()
    }

    /// Headroom: permits that could be leased right now (`target - leased`).
    #[must_use]
    pub fn available_threads(&self) -> usize {
        self.semaphore.available()
    }

    /// Maximum thread count (pool size).
    #[must_use]
    pub fn max_threads(&self) -> usize {
        self.config.read().max_threads
    }
}

#[cfg(test)]
mod semaphore_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::Semaphore;

    #[test]
    fn idle_reports_zero_leased() {
        let s = Semaphore::new(2, 8);
        assert_eq!(s.leased(), 0);
        assert_eq!(s.target(), 2);
        assert_eq!(s.available(), 2);
    }

    #[test]
    fn lease_and_drop_track_leased() {
        let s = Semaphore::new(4, 8);
        {
            let _g1 = s.acquire();
            let _g2 = s.acquire();
            assert_eq!(s.leased(), 2);
            assert_eq!(s.available(), 2);
        }
        assert_eq!(s.leased(), 0, "drops release leases");
        assert_eq!(s.available(), 4);
    }

    #[test]
    fn downscale_does_not_overshoot_on_drop() {
        // Lease the full target, shrink the target while leased, then drain.
        // The old model refilled `available` toward max_permits on drop,
        // undoing the downscale; the new model derives available from the
        // target, so post-drain available == target, not max.
        let s = Semaphore::new(8, 8);
        let guards: Vec<_> = (0..8).map(|_| s.acquire()).collect();
        assert_eq!(s.leased(), 8);
        s.set_target(2);
        assert_eq!(s.target(), 2);
        assert_eq!(
            s.available(),
            0,
            "leased (8) exceeds target (2): no headroom"
        );
        drop(guards);
        assert_eq!(s.leased(), 0);
        assert_eq!(
            s.available(),
            2,
            "available equals target after drain, not max_permits"
        );
    }

    #[test]
    fn set_target_clamps_to_one_and_max() {
        let s = Semaphore::new(4, 8);
        s.set_target(0);
        assert_eq!(s.target(), 1, "target floored at 1 to avoid deadlock");
        s.set_target(100);
        assert_eq!(s.target(), 8, "target capped at max_permits");
    }

    #[test]
    fn contention_never_exceeds_target() {
        // 8 threads hammer a target=2 limiter; leased must never exceed 2.
        let s = Arc::new(Semaphore::new(2, 2));
        let max_seen = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let s = Arc::clone(&s);
                let max_seen = Arc::clone(&max_seen);
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        let _g = s.acquire();
                        max_seen.fetch_max(s.leased(), Ordering::Relaxed);
                        std::thread::yield_now();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert!(
            max_seen.load(Ordering::Relaxed) <= 2,
            "leased never exceeded target=2"
        );
        assert_eq!(s.leased(), 0);
    }

    #[test]
    fn grow_target_wakes_parked_acquirer() {
        // target=1, one lease held; a second acquirer parks until the target
        // grows -- proving wakeup on set_target, not a spin.
        let s = Arc::new(Semaphore::new(1, 4));
        let held = s.acquire();
        assert_eq!(s.leased(), 1);
        let s2 = Arc::clone(&s);
        let handle = std::thread::spawn(move || {
            let _g = s2.acquire();
            s2.leased()
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        s.set_target(2);
        let observed = handle.join().unwrap();
        assert!(observed >= 1, "parked acquirer proceeded after target grew");
        drop(held);
    }
}
