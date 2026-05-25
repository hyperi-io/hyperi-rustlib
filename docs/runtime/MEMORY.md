# Memory

`MemoryGuard` is the cgroup-aware OOM-prevention layer. It tracks
application-level memory usage against a detected or configured
limit and exposes a fast atomic check that hot-path code uses to
decide whether to accept more work or shed load.

Critically — it's a *guard*, not a *limit*. Rustlib does not
OOM-kill its own process. It surfaces pressure so callers can
backpressure their producers, spill to disk, return 503, or do
whatever load-shedding their domain requires.

---

## Why "guard" not "limit"

The kernel already kills the process when the cgroup limit is hit
— that's an OOM-kill, and it's the worst possible outcome:
in-flight requests vanish, on-disk state may be torn, and K8s
restarts the pod with no graceful drain.

`MemoryGuard` exists so the process can refuse new work *before*
the kernel reaches for the axe. The consumer of the pressure
signal decides what "refuse" looks like:

| Consumer | Behaviour on `under_pressure()` |
|----------|--------------------------------|
| HTTP server | Return 503 Service Unavailable |
| Kafka receiver | Pause partition assignment |
| `TieredSink` | Spill the in-memory buffer to disk |
| `BatchEngine` | Smaller batch sizes, more frequent flushes |
| `ScalingPressure` | Bump KEDA pressure signal toward 1.0 |

The guard owns none of those policies. It just publishes the
ratio.

---

## Limit detection

`MemoryGuard::new(config)` reads `config.limit_bytes`. If zero (the
default), it auto-detects via [`detect_memory_limit`](../../src/memory/cgroup.rs):

| Priority | Source | File |
|----------|--------|------|
| 1 | cgroup v2 | `/sys/fs/cgroup/memory.max` |
| 2 | cgroup v1 | `/sys/fs/cgroup/memory/memory.limit_in_bytes` |
| 3 | system memory | `sysinfo::System::total_memory()` |

The detected raw limit is then scaled by `cgroup_headroom` (default
0.85) to leave room for process overhead — code, stack,
jemalloc fragmentation, kernel page cache attributed to the
cgroup. Rust has no GC so there's no spike headroom needed; the
15% gap is for the runtime cost of being a process.

A 4 GiB cgroup with the defaults becomes an effective limit of
~3.4 GiB, and backpressure triggers at 80% of that (~2.72 GiB,
~68% of the actual cgroup limit). That matches the OTel
Collector's `limit_percentage: 80` philosophy.

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
    High,    // ratio >= pressure_threshold — apply backpressure
}
```

`pressure_threshold` is the only knob the hot path cares about —
once the ratio crosses it, `under_pressure()` flips to `true` and
stays there until usage drops back below.

There's no separate "warn / soft / hard" tier. The hot-path API is
binary (`under_pressure()` → bool); the three-level `pressure()`
helper exists for logging and metric labels.

---

## Hot-path API

`MemoryGuard` uses lock-free atomics throughout — every operation
is one or two `Relaxed` loads/stores.

```rust
let guard = Arc::new(MemoryGuard::new(MemoryGuardConfig::from_env("DFE")));

// On data arrival — atomic check-and-add, rolls back if it would exceed:
if !guard.try_reserve(payload_len) {
    return Err(BackpressureError::MemoryFull);   // 503 / pause / spill
}

// After data is flushed/sent/dropped:
guard.release(payload_len);

// Cheap hot-path probe — single Relaxed load:
if guard.under_pressure() {
    return shed_load();
}
```

| Operation | Cost |
|-----------|------|
| `try_reserve(n)` | `fetch_add` + branch + optional `fetch_sub` rollback |
| `add_bytes(n)` | `fetch_add` + threshold update |
| `release(n)` | `fetch_update` (saturating sub) + threshold update |
| `under_pressure()` | One `Relaxed` load on the `AtomicBool` |
| `pressure_ratio()` | One `Relaxed` load + one float division |

`release` is saturating — over-release floors at zero rather than
wrapping. That matters for telemetry counters that must never
panic.

---

## Pressure signal for ScalingPressure

`pressure_ratio() -> f64` returns the current ratio (0.0 – 1.0+,
where >1.0 means the guard is misconfigured against its limit).
That's the value `ScalingPressure` consumes when memory is one of
its weighted components.

`ScalingPressure` also treats memory specially via a hard gate —
when the memory ratio exceeds ~0.9, the autoscaler signal goes
straight to maximum to force a scale-up before OOM-kill, bypassing
the weighted composite. See
[../pipeline/SCALING.md](../pipeline/SCALING.md).

---

## Integration with ServiceRuntime

When the `memory` feature is on (default for `cli-service`),
`ServiceRuntime::build` constructs the guard automatically:

```rust
let memory_guard = Arc::new(MemoryGuard::new(
    MemoryGuardConfig::from_env(env_prefix),
));
```

The runtime then hands the `Arc<MemoryGuard>` to:

- `AdaptiveWorkerPool` via `set_memory_guard(...)` — pool scales
  down when memory is high.
- `BatchEngine` via `auto_wire(..., Some(&memory_guard))` — engine
  reduces batch size under pressure.
- The app, as `runtime.memory_guard`, for transport-level
  backpressure decisions.

Apps that want explicit control over the guard config can ignore
the runtime's instance and build their own — there's no
singleton, the guard is an `Arc`-passed object.

---

## Config sources

| Source | Method |
|--------|--------|
| Defaults | `MemoryGuardConfig::default()` — auto-detect, 0.80 threshold, 0.85 headroom |
| Cascade | `MemoryGuardConfig::from_cascade()` — `memory` key, registers in section registry |
| Env vars (cascade-aware) | `MemoryGuardConfig::from_env("DFE")` — reads `DFE_MEMORY_*` via flat-env bridge |
| Env vars (raw) | `MemoryGuardConfig::from_env_raw("DFE")` — same vars but no `config` feature required |

Env-var names:

- `{PREFIX}_MEMORY_LIMIT_BYTES` — explicit override (0 or unset = auto)
- `{PREFIX}_MEMORY_PRESSURE_THRESHOLD` — float, default 0.80
- `{PREFIX}_MEMORY_CGROUP_HEADROOM` — float, default 0.85

`ServiceRuntime` uses `from_env(env_prefix)` so K8s flat env vars
(`DFE_LOADER_MEMORY_LIMIT_BYTES`) work without bridging.

---

## API surface

| Item | Purpose |
|------|---------|
| `MemoryGuard::new(config)` | Construct the guard; auto-detects the limit if `config.limit_bytes == 0` |
| `MemoryGuard::try_reserve(n) -> bool` | Atomic check-and-add; rolls back if over limit |
| `MemoryGuard::add_bytes(n)` | Unchecked tracking — use when data is already accepted |
| `MemoryGuard::release(n)` | Saturating subtract on the tracked counter |
| `MemoryGuard::under_pressure() -> bool` | Hot-path probe — single atomic load |
| `MemoryGuard::pressure() -> MemoryPressure` | Three-level enum for logging / metric labels |
| `MemoryGuard::pressure_ratio() -> f64` | Current usage as fraction of effective limit |
| `MemoryGuard::current_bytes() -> u64` | Tracked bytes |
| `MemoryGuard::limit_bytes() -> u64` | Effective limit (after headroom) |
| `MemoryGuardConfig` | Serde-deserialisable config struct |
| `MemoryGuardConfig::from_cascade()` | Build from the config cascade's `memory` key |
| `MemoryGuardConfig::from_env(prefix)` | Build from flat env vars |
| `MemoryPressure` | `Low` / `Medium` / `High` |
| `cgroup::detect_memory_limit() -> u64` | Standalone limit-detection helper |

---

## Layered defence

```text
Layer 1 (opt-in): Allocator cap — hard limit, abort on alloc failure (last resort)
Layer 2 (default): MemoryGuard — application-level tracking, backpressure signal
```

Layer 2 is what 99% of services need. Layer 1 (cap-allocator) is a
seatbelt for binaries that can't trust every dependency to honour
backpressure — it crashes the process cleanly via
`alloc::handle_alloc_error` instead of letting the kernel issue an
OOM-kill.

---

## Related

- [SERVICE-RUNTIME.md](SERVICE-RUNTIME.md) — runtime stores the `Arc<MemoryGuard>`
- [RUNTIME-CONTEXT.md](RUNTIME-CONTEXT.md) — cgroup detection background
- [../pipeline/SCALING.md](../pipeline/SCALING.md) — KEDA pressure consumes the memory ratio
- [../core-pillars/CONFIG.md](../core-pillars/CONFIG.md) — `memory` cascade section
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) — `memory`
- Source: [../../src/memory/guard.rs](../../src/memory/guard.rs),
  [../../src/memory/cgroup.rs](../../src/memory/cgroup.rs)
