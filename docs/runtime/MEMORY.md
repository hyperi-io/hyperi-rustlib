# Memory

`MemoryGuard` is the cgroup-aware OOM-prevention layer. It tracks
process memory against a detected or configured limit and exposes a
fast atomic `under_pressure()` check that hot-path code reads to
decide whether to accept more work or shed load.

It is a *guard*, not a *limit*. Rustlib never OOM-kills its own
process and takes no allocator dependency (`#![forbid(unsafe_code)]`).
It surfaces pressure; the caller decides what to do with it.

---

## Why guard, not limit

The kernel OOM-kills the process when the cgroup limit is hit -- the
worst outcome: in-flight requests vanish, on-disk state can tear, K8s
restarts the pod with no graceful drain. `MemoryGuard` lets the
process refuse new work *before* the kernel reaches for the axe.

Memory pressure brakes INBOUND intake only -- never the outbound
drain. Gating the drain deadlocks (you stop the thing that frees
memory). See the backpressure doctrine in SELF-REGULATION.

| Consumer | Behaviour on `under_pressure()` |
|----------|--------------------------------|
| HTTP server | 503 Service Unavailable |
| Kafka receiver | Pause partition assignment |
| `TieredSink` | Spill in-memory buffer to disk |
| `BatchEngine` | Smaller batches, more frequent flushes |
| `ScalingPressure` | Bump KEDA signal toward 1.0 |

The guard publishes the ratio; it owns none of these policies.

---

## Limit detection

`MemoryGuard::new(config)` reads `config.limit_bytes`. Zero (the
default) auto-detects via `detect_memory_limit`:

| Priority | Source | File |
|----------|--------|------|
| 1 | cgroup v2 | `/sys/fs/cgroup/memory.max` |
| 2 | cgroup v1 | `/sys/fs/cgroup/memory/memory.limit_in_bytes` |
| 3 | system memory | `sysinfo::System::total_memory()` |

Detected limit is scaled by `cgroup_headroom` (default 0.85) to leave
room for code, stack, allocator fragmentation, and cgroup-attributed
page cache. Rust has no GC, so no spike headroom is needed -- the 15%
gap is just the cost of being a process.

A 4 GiB cgroup with defaults -> ~3.4 GiB effective limit; backpressure
at 80% of that (~2.72 GiB, ~68% of the real cgroup limit). Matches the
OTel Collector's `limit_percentage: 80` philosophy.

---

## Heap source (allocator-agnostic, opt-in)

`memory::set_heap_source(fn() -> usize)` registers a process-wide
total-live-heap reader (set once at startup). With it registered, the
guard reads the *true total process heap* instead of summing per-batch
reservations:

- `try_reserve(n)` becomes a projected-admission check
  (`heap() + n <= limit`) and does NOT mutate the counter -- the
  allocator already accounts the bytes and frees them on drop, so no
  `release` is needed.
- `under_pressure()`, `pressure_ratio()`, and `current_bytes()` all
  compute live from the true heap.

The source is allocator-agnostic: pass `cap::Cap::allocated`, a
jemalloc `stats.allocated` reader (advance the epoch inside the
closure), or any `fn() -> usize`. Rustlib itself depends on no
allocator -- the choice is the binary's. Without a registered source,
the guard falls back to the classic per-batch reservation counter.

---

## Thresholds

```yaml
memory:
  limit_bytes: 0           # 0 = auto-detect
  pressure_threshold: 0.80 # backpressure at 80% of effective limit
  cgroup_headroom: 0.85    # use 85% of detected cgroup limit
```

```rust
pub enum MemoryPressure {
    Low,     // ratio < 0.5
    Medium,  // 0.5 <= ratio < pressure_threshold
    High,    // ratio >= pressure_threshold -- apply backpressure
}
```

`pressure_threshold` is the only knob the hot path cares about. There
is no separate warn/soft/hard tier: the hot-path API is binary
(`under_pressure() -> bool`); `pressure()` is for log/metric labels.

---

## Hot-path API

Lock-free atomics throughout -- every operation is one or two
`Relaxed` loads/stores.

```rust
let guard = Arc::new(MemoryGuard::new(MemoryGuardConfig::from_env("DFE")));

// On data arrival -- atomic check, rolls back if it would exceed:
if !guard.try_reserve(payload_len) {
    return Err(BackpressureError::MemoryFull);   // 503 / pause / spill
}

// After data is flushed/sent/dropped (no-op semantics under a heap source):
guard.release(payload_len);

// Cheap hot-path probe:
if guard.under_pressure() {
    return shed_load();
}
```

| Operation | Cost |
|-----------|------|
| `try_reserve(n)` | `fetch_add` + branch + optional rollback (or one compare under a heap source) |
| `add_bytes(n)` | `fetch_add` + threshold update |
| `release(n)` | saturating `fetch_update` -- over-release floors at zero |
| `under_pressure()` | one `Relaxed` load (or one heap read + compare) |
| `pressure_ratio()` | one load + one float division; >1.0 means misconfigured limit |

---

## Self-regulation

Self-regulation (the `governor` feature) is ON by default; opt out via
`self_regulation.enabled = false`, after which nothing is constructed
and the data path is byte-identical to pre-governor. It consumes the
guard's pressure to drive the inbound brake and an AIMD byte budget.
Its metrics are namespaced `self_regulation_*`. See SELF-REGULATION.

`ScalingPressure` consumes the guard too: memory is a hard gate. When
the ratio exceeds ~0.9 the autoscaler signal jumps straight to maximum
to force scale-up before OOM-kill, bypassing the weighted composite.

---

## ServiceRuntime wiring

`ServiceRuntime::build` constructs the guard from the env prefix and
hands the `Arc<MemoryGuard>` to:

- `AdaptiveWorkerPool` via `set_memory_guard(...)` -- scales down under
  pressure.
- `BatchEngine` via `auto_wire(..., Some(&memory_guard))` -- reduces
  batch size under pressure.
- The self-regulation governor (built from the same guard).

Apps read `runtime.memory_guard` directly. Env-var overrides
(`DFE_LOADER_MEMORY_LIMIT_BYTES`) work without bridging because
`build` uses `from_env(env_prefix)`.

Env vars:

- `{PREFIX}_MEMORY_LIMIT_BYTES` -- explicit override
- `{PREFIX}_MEMORY_PRESSURE_THRESHOLD` -- float, default 0.80
- `{PREFIX}_MEMORY_CGROUP_HEADROOM` -- float, default 0.85

---

## API surface

| Item | Purpose |
|------|---------|
| `memory::set_heap_source(fn() -> usize) -> bool` | Register true-heap reader (set-once; returns false if already set) |
| `MemoryGuard::new(config)` | Construct; auto-detects limit if `limit_bytes == 0` |
| `MemoryGuard::try_reserve(n) -> bool` | Admission check; rolls back if over limit |
| `MemoryGuard::add_bytes(n)` | Unchecked tracking -- data already accepted |
| `MemoryGuard::release(n)` | Saturating subtract on the per-batch counter |
| `MemoryGuard::under_pressure() -> bool` | Hot-path probe |
| `MemoryGuard::pressure() -> MemoryPressure` | Three-level enum for logs/labels |
| `MemoryGuard::pressure_ratio() -> f64` | Usage as fraction of effective limit |
| `MemoryGuard::current_bytes() -> u64` | True heap (with source) or tracked bytes |
| `MemoryGuard::limit_bytes() -> u64` | Effective limit (after headroom) |
| `MemoryGuardConfig` | Serde-deserialisable config struct |
| `MemoryGuardConfig::from_cascade()` | Load from the 8-layer cascade |
| `MemoryGuardConfig::from_env(prefix)` | Build from `{PREFIX}_MEMORY_*` env vars |
| `MemoryPressure` | `Low` / `Medium` / `High` |
| `cgroup::detect_memory_limit() -> u64` | Standalone limit detection |
| `cgroup::detect_memory_pressure() -> Option<f64>` | This container's `current/limit` |

---

## Two-layer model

| Layer | Default | Behaviour |
|-------|---------|-----------|
| 1 -- cap allocator | opt-in | Hard cap; last-resort crash via `handle_alloc_error` instead of OOM-kill |
| 2 -- `MemoryGuard` | on | Cgroup-aware tracking + backpressure signal |

Layer 2 is what 99% of services need. Layer 1 is a seatbelt for
binaries that can't trust every dependency to honour backpressure.

---

## Related

- [SELF-REGULATION.md](SELF-REGULATION.md) -- governor, inbound brake, AIMD budget
- [RUNTIME-CONTEXT.md](RUNTIME-CONTEXT.md) -- cgroup limit detection
- [SERVICE-RUNTIME.md](SERVICE-RUNTIME.md) -- `ServiceRuntime` holds the guard
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) -- `memory`, `governor`
- Source: [../../src/memory/guard.rs](../../src/memory/guard.rs),
  [../../src/memory/cgroup.rs](../../src/memory/cgroup.rs)
