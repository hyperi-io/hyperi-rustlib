# Health

K8s expects three distinct probe paths with three distinct semantics. The
metrics HTTP server mounts all three. Modules register a health-check
callback into a global `HealthRegistry`; the readiness endpoint aggregates
every registered check plus an optional caller-supplied callback to decide
200 vs 503.

The probe trinity is non-negotiable for any service deployed to K8s. Mixing
liveness with dependency checks is the single most common cause of cascading
restart loops — when the database goes down, restarting every replica makes
the recovery slower, not faster. Liveness exists to detect a deadlocked
process, nothing else.

---

## Probe trinity

| Endpoint | Semantics | Fails when | K8s action on failure |
|---|---|---|---|
| `/healthz`, `/health/live` | Liveness — process alive | Never (returns 200 unconditionally) | Kill + restart pod |
| `/startupz`, `/health/startup` | Startup — init complete | Until `mark_started()` is called | Wait (long timeout); then restart |
| `/readyz`, `/health/ready` | Readiness — deps OK + ready flag | Registry check unhealthy OR readiness callback false OR ready flag cleared | Remove from Service endpoints (no traffic), don't restart |

The endpoints return JSON: `{"status":"alive"}`, `{"status":"started"}`,
`{"status":"ready"}` on 200; `{"status":"not_ready"}` / `{"status":"starting"}`
on 503.

**Liveness must NEVER check downstream dependencies.** No DB probe, no
Kafka ping, no external HTTP call. Liveness exists to detect a deadlocked
process — restarting won't help a downstream outage.

**Readiness aggregates dependencies + the explicit ready flag.** The
shutdown handler clears the flag before draining so K8s removes the pod
from Service endpoints before the app starts ending in-flight work. See
[SHUTDOWN.md](SHUTDOWN.md).

---

## Registry

`HealthRegistry` is a global `OnceLock<Mutex<Vec<HealthEntry>>>` — registered
checks live for the lifetime of the process. Modules register manually at
construction time:

```rust
use hyperi_rustlib::health::{HealthRegistry, HealthStatus};
use std::sync::atomic::{AtomicBool, Ordering};

static KAFKA_HEALTHY: AtomicBool = AtomicBool::new(true);

HealthRegistry::register("kafka_consumer", || {
    if KAFKA_HEALTHY.load(Ordering::Relaxed) {
        HealthStatus::Healthy
    } else {
        HealthStatus::Unhealthy
    }
});
```

Earlier docs called registration "automatic" — that's misleading. **Registration
is manual**. Modules call `HealthRegistry::register(name, callback)` at
construction. **Aggregation is automatic** — once registered, `/readyz` and
`/healthz/ready` query every entry on every request, no per-module wiring at
the endpoint side.

The callback fires on every health request, so keep it cheap — an
`AtomicBool` load or a cached value. Don't make a syscall or network call
inside it.

---

## Three states

```rust
pub enum HealthStatus { Healthy, Degraded, Unhealthy }
```

| Status | `is_healthy()` | `is_ready()` | Use for |
|---|---|---|---|
| `Healthy` | Yes | Yes | Component fully operational |
| `Degraded` | No | Yes | Operational but impaired — circuit half-open, fallback active, elevated latency |
| `Unhealthy` | No | No | Not operational — stop sending traffic |

`is_ready()` is the readiness predicate — degraded counts as ready because
the service can still serve, just at reduced capability. `is_healthy()` is
strict (all-healthy), used for detailed status output, not the K8s probe.

Empty registry is vacuously healthy and ready — a service with no registered
checks always returns 200. Add at least one registration (transport, DB
connection, downstream API) so the probe means something.

---

## Per-component status

`HealthRegistry::components()` returns the current snapshot for detailed
health endpoints:

```rust
let components: Vec<(String, HealthStatus)> = HealthRegistry::components();
// [("kafka_consumer", Healthy), ("clickhouse", Degraded)]
```

With `serde_json` enabled, `HealthRegistry::to_json()` produces the
detailed-endpoint response:

```json
{
  "status": "degraded",
  "components": [
    {"name": "kafka_consumer", "status": "healthy"},
    {"name": "clickhouse", "status": "degraded"}
  ]
}
```

Overall status is `healthy` if all components are healthy, `degraded` if any
are degraded but none are unhealthy, `unhealthy` if any are unhealthy.

---

## K8s manifest

```yaml
spec:
  containers:
    - name: dfe-loader
      ports:
        - { name: metrics, containerPort: 9090 }
      startupProbe:
        httpGet: { path: /startupz, port: metrics }
        failureThreshold: 30          # 30 * 2s = 1 min boot budget
        periodSeconds: 2
      livenessProbe:
        httpGet: { path: /healthz, port: metrics }
        periodSeconds: 10
      readinessProbe:
        httpGet: { path: /readyz, port: metrics }
        periodSeconds: 5
```

Use the metrics port — the same HTTP server hosts both. No separate health
listener.

---

## Wire-up checklist

1. Construct `MetricsManager`. Call `mgr.set_readiness_check(|| ...)` if
   the app needs a callback gate in addition to the registry.
2. Modules register into `HealthRegistry::register()` at construction.
3. Call `mgr.mark_started()` once init is complete (DB connected, Kafka
   subscribed, initial config loaded). `/startupz` flips to 200.
4. Start the HTTP server via `mgr.start_server` or
   `mgr.start_server_with_routes`.
5. On SIGTERM, the shutdown handler clears the ready flag, waits the
   pre-stop delay, then cancels the global token. See [SHUTDOWN.md](SHUTDOWN.md).

---

## API surface

| Item | Purpose |
|---|---|
| `HealthRegistry::register(name, fn)` | Add a component health-check callback |
| `HealthRegistry::is_healthy() -> bool` | All components `Healthy` |
| `HealthRegistry::is_ready() -> bool` | No components `Unhealthy` |
| `HealthRegistry::components() -> Vec<(String, HealthStatus)>` | Snapshot for detailed output |
| `HealthRegistry::to_json() -> serde_json::Value` | Detailed-endpoint response |
| `HealthStatus::{Healthy, Degraded, Unhealthy}` | Three-state enum |
| `HealthStatus::as_str()` | JSON-friendly string |
| `MetricsManager::set_readiness_check(fn)` | Caller-supplied callback gate (ANDed with registry) |
| `MetricsManager::mark_started()` | Flip `/startupz` to 200 |

---

## Testing

The registry is process-global. Tests that register components must use
`HealthRegistry::reset()` (test-only) inside a serialised lock, otherwise
test order changes the registered set. See the test pattern in
[`src/health/registry.rs`](../../src/health/registry.rs).

For tests verifying readiness-aggregation behaviour, register a callback
that reads an `AtomicU8` and flip it between states — that's exactly how
the dynamic-state-changes test is structured.

---

## Related

- [METRICS.md](METRICS.md) — `/readyz` is served by the metrics HTTP server
- [SHUTDOWN.md](SHUTDOWN.md) — ready flag clearing before drain
- [../AUTO-WIRING.md](../AUTO-WIRING.md), [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) — `health`
- Source: [`src/health/mod.rs`](../../src/health/mod.rs), [`src/health/registry.rs`](../../src/health/registry.rs)
