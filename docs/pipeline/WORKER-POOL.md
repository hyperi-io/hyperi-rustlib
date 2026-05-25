# Worker Pool

`AdaptiveWorkerPool` is the shared compute primitive for every DFE
service. Hybrid backend — rayon for CPU-bound work, tokio JoinSet for
async I/O — and a permit semaphore that the scaler resizes at runtime.

The DFE common pattern: one pool per process, built from the cascade
at startup, then handed to every component that needs parallelism
(`BatchEngine`, transforms, enrichers).

---

## Two APIs

| API | Backend | Use for | Avoid for |
|-----|---------|---------|-----------|
| `process_batch(items, f)` | rayon `par_iter` | JSON parse, transforms, compression, CEL eval, routing | Anything that needs `.await` |
| `fan_out_async(items, f)` | tokio `spawn` with concurrency cap | Enrichment lookups, external APIs, storage writes | Pure-CPU loops (steals from the runtime) |

Both preserve input order in the output and return
`Vec<Result<R, E>>`. The async fan-out chunks at `async_concurrency`
(default 32) to bound in-flight tasks.

A third method, `install(f)`, exposes the raw rayon pool for callers
that need `par_iter_mut` or other rayon primitives not covered by
`process_batch`. Used by `BatchEngine` for the mutable transform phase.

---

## Permit-throttling, not pool resize

Rayon pools cannot be resized after creation. The pool is built once
at `max_threads` (default = `available_parallelism`, cgroup-aware) and
a counting semaphore controls how many threads pick up work. Threads
that fail to acquire a permit `std::thread::yield_now`.

The scaler updates `permits` — never the underlying pool size.

```
rayon::ThreadPool (fixed at max_threads)
        ↓
    Semaphore (permits — scaler controls)
        ↓
    process_batch — each item acquires a permit, releases on drop
```

This is the same permit-throttling pattern documented in the Rust
standards section *Permit-Throttling a Non-Resizable Pool*. See
[`../../src/worker/pool.rs`](../../src/worker/pool.rs).

---

## Pressure-based scaling

A background controller (`ScalingController`, started by
`start_scaling_loop`) samples CPU and memory every
`scale_interval_secs` (default 5 s) and adjusts permits via
`Semaphore::set_permits`.

| Signal | Direction | Step |
|--------|-----------|------|
| `cpu < grow_below` (default 0.60) | up | +2 permits |
| `grow_below ≤ cpu ≤ shrink_above` (default 0.85) | steady | unchanged |
| `shrink_above < cpu ≤ emergency_above` (default 0.95) | down | −1 permit |
| `cpu > emergency_above` | emergency down | −2 permits |
| `memory_pressure > memory_pressure_cap` (default 0.80) | hard cap | clamp to `min_threads` |

Memory pressure overrides everything — when memory is hot we shrink
to the floor and stay there, regardless of CPU. The clamp is to
`[min_threads, max_threads]`. Each scaling decision emits a counter
(`worker_pool_scale_events_total{direction}`) and gauges for active
threads, target threads, CPU, memory, and saturation.

Memory pressure has two sources — sysinfo process RSS and an optional
`MemoryGuard` attached via `set_memory_guard`. The controller uses the
max of the two so either source can trigger the cap.

---

## Optional integrations

- `set_memory_guard(Arc<MemoryGuard>)` — feed cgroup/process memory
  pressure into scaling decisions.
- `set_scaling_pressure(Arc<ScalingPressure>)` — feed pool saturation
  back into the KEDA signal as the `worker_pool_saturation`
  component.

Both are no-ops when the corresponding feature is off.
`ServiceRuntime` wires both automatically.

---

## Configuration

Cascade key `worker_pool`:

```yaml
worker_pool:
  min_threads: 2            # floor for scaling
  max_threads: 0            # 0 = auto-detect (cgroup-aware), else capped at available_parallelism
  grow_below: 0.60          # CPU below → +2 permits
  shrink_above: 0.85        # CPU above → −1 permit
  emergency_above: 0.95     # CPU above → −2 permits
  memory_pressure_cap: 0.80 # memory above → clamp to min_threads
  scale_interval_secs: 5
  async_concurrency: 32     # fan_out_async chunk size
  health_saturation_timeout_secs: 30
```

`validate()` rejects out-of-order thresholds at startup
(`grow_below >= shrink_above`, `shrink_above >= emergency_above`,
`min_threads > max_threads`) — fail-fast on config typos.

---

## Usage

```rust
use hyperi_rustlib::worker::AdaptiveWorkerPool;

let pool = std::sync::Arc::new(AdaptiveWorkerPool::from_cascade("worker_pool")?);
pool.register_metrics(metrics_manager);
pool.start_scaling_loop(shutdown.clone());

// CPU parallel
let results = pool.process_batch(&batch, |msg| transform(msg));

// Async fan-out
let enriched = pool.fan_out_async(&items, |item| async move {
    lookup_enrichment(item).await
}).await;
```

`ServiceRuntime` does the construction and `start_scaling_loop`
plumbing automatically when the `worker-pool` feature is on — apps
just use `runtime.worker_pool`.

---

## API surface

| Item | Purpose |
|------|---------|
| `AdaptiveWorkerPool::new(cfg)` | Build pool with explicit config |
| `AdaptiveWorkerPool::from_cascade(key)` | Build pool from `worker_pool` cascade key |
| `process_batch(items, f)` | Rayon `par_iter` with permit throttling, results in input order |
| `fan_out_async(items, f)` | Tokio fan-out with `async_concurrency` cap, results in input order |
| `install(f)` | Raw rayon pool access — no permit throttling |
| `register_metrics(mgr)` | Register operational gauges with `MetricsManager` |
| `start_scaling_loop(cancel)` | Spawn the pressure-based scaling controller |
| `set_memory_guard(guard)` | Attach memory pressure source for the scaler |
| `set_scaling_pressure(p)` | Feed pool saturation back into the KEDA pressure signal |
| `active_threads() -> usize` | Permits currently in use |
| `max_threads() -> usize` | Pool size (fixed at construction) |

---

## Source

- [`../../src/worker/pool.rs`](../../src/worker/pool.rs) — `AdaptiveWorkerPool`, `Semaphore`
- [`../../src/worker/scaler.rs`](../../src/worker/scaler.rs) — `ScalingController`, watermark algorithm
- [`../../src/worker/config.rs`](../../src/worker/config.rs) — `WorkerPoolConfig`, validation
- [`../../src/worker/metrics.rs`](../../src/worker/metrics.rs)

---

## Related

- [BATCH-ENGINE.md](BATCH-ENGINE.md) — primary consumer of the pool
- [SCALING.md](SCALING.md) — `ScalingPressure` integration
- [../runtime/MEMORY.md](../runtime/MEMORY.md) — `MemoryGuard` integration
- [../runtime/SERVICE-RUNTIME.md](../runtime/SERVICE-RUNTIME.md)
- [../AUTO-WIRING.md](../AUTO-WIRING.md)
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) — `worker-pool`
- [../ARCHITECTURE.md](../ARCHITECTURE.md)
