# Metrics

`MetricsManager::new("dfe_loader")` installs a global `metrics` recorder, builds
a Prometheus exporter, and exposes `/metrics` on the metrics HTTP server. Once
installed, any module can call `metrics::counter!`, `metrics::gauge!`,
`metrics::histogram!` — the macros are no-ops when no recorder is set, so
library code remains safe to compile without the metrics feature.

Every counter / gauge / histogram constructed through `MetricsManager` also
pushes a `MetricDescriptor` into a `MetricRegistry`. That registry is rendered
as JSON at `/metrics/manifest` and persisted to `docs/metrics-manifest.json`
via the `metrics-manifest` CLI subcommand. The manifest is the catalogue
that downstream tools (Grafana auto-provisioning, alert validators, the DFE
docs site) consume — there is no other source of metric metadata.

Process and container metrics (RSS, CPU, FDs, cgroup limits) auto-collect on
a fixed interval via [sysinfo](https://crates.io/crates/sysinfo) when the
relevant features are on. The container metrics read cgroup v1 and v2
transparently.

---

## Feature tiers

| Feature | Adds | Use |
|---|---|---|
| `metrics-core` | Macros + `MetricRegistry` | Pure library crates that record but don't host the exporter |
| `metrics-process` | `metrics-core` + sysinfo process probe | Single-binary tools that want RSS/CPU without an HTTP server |
| `metrics` | `metrics-process` + Prometheus exporter + `/metrics` HTTP server | Services |

DFE services pull `metrics`. Library crates published to consumers pull
`metrics-core` so they don't drag a TCP listener into their dependents.

---

## Setup

```rust
use hyperi_rustlib::metrics::MetricsManager;

let mut mgr = MetricsManager::new("dfe_loader");

// Construct metrics — every call also registers a descriptor
let sent = mgr.counter("transport_sent_total", "Messages sent");
let lag = mgr.histogram("send_latency_seconds", "Send latency");

// Apply labels at recording time, not at construction:
mgr.counter_with_labels(
    "transport_sent_total",
    "Messages sent",
    &["transport", "topic"],   // declared in the manifest
    "transport",                // group
);
metrics::counter!("dfe_loader_transport_sent_total",
    "transport" => "kafka", "topic" => "events").increment(1);

// Start the HTTP server
mgr.start_server("0.0.0.0:9090").await?;
```

Names are prefixed with the namespace automatically — `counter("foo")` registers
as `dfe_loader_foo`. Use `counter_with_labels` / `gauge_with_labels` /
`histogram_with_labels` when label keys belong in the manifest (i.e. always —
unlabeled metrics are rare in DFE pipelines).

The single-binary use case is `ServiceRuntime` from `cli`, which constructs
the manager, wires the readiness callback, attaches optional `ScalingPressure`
and `MemoryGuard` endpoints, and merges service-specific routes via
`start_server_with_routes`. See [../runtime/SERVICE-RUNTIME.md](../runtime/SERVICE-RUNTIME.md).

---

## The manifest

The registry tracks: name, type, description, unit, label keys, group, bucket
spec, operational use cases, dashboard hint, app version, git commit, and
registration timestamp.

`GET /metrics/manifest` returns the full catalogue as JSON:

```json
{
  "schema_version": 1,
  "app": "dfe_loader",
  "version": "x.y.z",
  "commit": "<git-sha>",
  "registered_at": "<rfc3339-timestamp>",
  "metrics": [
    {
      "name": "dfe_loader_transport_sent_total",
      "metric_type": "counter",
      "description": "Messages sent",
      "labels": ["transport", "topic"],
      "group": "transport"
    }
  ]
}
```

The `metrics-manifest` CLI subcommand renders the same JSON to disk without
running the service — used in CI to keep `docs/metrics-manifest.json`
in sync:

```bash
my-app metrics-manifest --output docs/
```

Set extra metadata after registration:

```rust
mgr.set_use_cases("dfe_loader_send_latency_seconds",
    &["SLO p99 < 500ms", "Page on sustained > 1s"]);
mgr.set_dashboard_hint("dfe_loader_send_latency_seconds", "heatmap");
mgr.set_build_info(env!("CARGO_PKG_VERSION"), env!("GIT_COMMIT"));
```

`DfeMetrics::register(&mgr)` (under `metrics-dfe`) registers the canonical DFE
metric set — transport, batch engine, worker pool, memory, scaling — in one
call so every DFE app exports the same group of metrics with matching labels.

---

## Endpoints

| Path | Body |
|---|---|
| `/metrics` | Prometheus text |
| `/metrics/manifest` | JSON catalogue |
| `/healthz`, `/health/live` | `{"status":"alive"}` — process alive |
| `/startupz`, `/health/startup` | 503 until `mgr.mark_started()`, then 200 |
| `/readyz`, `/health/ready` | 200 if readiness callback + [`HealthRegistry`](HEALTH.md) both pass, else 503 |
| `/scaling/pressure` | Float `0.0–1.0` (requires `scaling` + `set_scaling_pressure`) |
| `/memory/pressure` | JSON with ratio + bytes (requires `memory` + `set_memory_guard`) |

Path-order note: `/metrics/manifest` is matched before `/metrics` in the
prefix-match handler. Don't reorder.

---

## OTel mode

With `otel-metrics` enabled, `MetricsManager` installs an OTel SDK meter
provider and pushes via OTLP. With **both** `metrics` and `otel-metrics`, a
`metrics-util` `FanoutBuilder` composes the two recorders so every macro
records to both — `/metrics` for scrape, OTLP for push. Call
`mgr.shutdown_otel()` before exit to flush the batch exporter, otherwise the
last interval's data is lost.

See [`OtelMetricsConfig`](../../src/metrics/otel_types.rs) for endpoint /
protocol / batching config.

---

## API surface

| Item | Purpose |
|---|---|
| `MetricsManager::new(namespace)` | Construct + install recorder |
| `MetricsManager::with_config(MetricsConfig)` | Custom namespace, intervals, OTel config |
| `MetricsManager::new_for_test(namespace)` *(test only)* | No global recorder install — safe for parallel tests |
| `counter` / `gauge` / `histogram` | Construct + auto-register |
| `*_with_labels(name, desc, labels, group)` | Same, with manifest label keys + group |
| `histogram_with_buckets` | Custom bucket spec (captured in manifest) |
| `set_readiness_check(fn)` | Wire into `/readyz` |
| `mark_started()` | Flip `/startupz` to 200 |
| `set_scaling_pressure(Arc<ScalingPressure>)` | Adds `/scaling/pressure` |
| `set_memory_guard(Arc<MemoryGuard>)` | Adds `/memory/pressure` |
| `set_build_info(version, commit)` | Manifest metadata |
| `set_use_cases(name, &[&str])` | Manifest annotation |
| `set_dashboard_hint(name, hint)` | Manifest annotation |
| `registry() -> MetricRegistry` | Cloneable handle for embedding `/metrics/manifest` in custom routers |
| `render_handle() -> Option<RenderHandle>` | Cloneable Prometheus text renderer for axum routes |
| `start_server(addr)` | Built-in router |
| `start_server_with_routes(addr, extra)` | Merge service-specific routes |
| `shutdown_otel()` | Flush OTLP batch exporter |
| `DfeMetrics::register(&mgr)` | Canonical DFE metric set (feature `metrics-dfe`) |
| `latency_buckets()`, `size_buckets()` | Standard histogram bucket presets |

---

## Testing

Parallel tests panic if multiple call `MetricsManager::new()` — the global
Prometheus recorder installs once per process. Use `MetricsManager::new_for_test()`
in unit tests: it skips the recorder install but keeps the registry, descriptor
push, and namespacing intact. The `metrics::*` macros become no-ops, which
is fine — most tests verify descriptor registration and naming, not record
values.

End-to-end recording is verified in the integration test suite where a
single fixture installs the recorder once.

---

## Related

- [CONFIG.md](CONFIG.md) — `MetricsConfig` sources from the cascade
- [LOGGING.md](LOGGING.md) — sampled log + counter is the standard pair
- [HEALTH.md](HEALTH.md) — `/readyz` consults `HealthRegistry`
- [TRACING.md](TRACING.md) — OTel-metrics is configured separately from OTel-tracing
- [../runtime/SERVICE-RUNTIME.md](../runtime/SERVICE-RUNTIME.md) — `ServiceRuntime` wires the manager
- [../AUTO-WIRING.md](../AUTO-WIRING.md), [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md)
- Source: [`src/metrics/mod.rs`](../../src/metrics/mod.rs), [`src/metrics/manifest.rs`](../../src/metrics/manifest.rs), [`src/metrics/dfe.rs`](../../src/metrics/dfe.rs)
