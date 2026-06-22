# Metrics

`MetricsManager::new("dfe_loader")` installs a global `metrics` recorder, builds a
Prometheus exporter, and serves `/metrics`. Any module then calls
`metrics::counter!` / `gauge!` / `histogram!` -- the macros are no-ops with no
recorder, so library code compiles without the metrics feature.

Every counter / gauge / histogram built through `MetricsManager` also pushes a
`MetricDescriptor` into a `MetricRegistry`, rendered as JSON at `/metrics/manifest`
and to `docs/metrics-manifest.json` via the `metrics-manifest` CLI subcommand.
That manifest is the only source of metric metadata for downstream tools (Grafana
provisioning, alert validators, the DFE docs site).

Process and container metrics (RSS, CPU, FDs, cgroup limits) auto-collect on a
fixed interval via [sysinfo](https://crates.io/crates/sysinfo) when the relevant
features are on; container metrics read cgroup v1 and v2 transparently.

---

## Feature tiers

| Feature | Adds | Use |
|---|---|---|
| `metrics-core` | Macros + `MetricRegistry` | Library crates that record but don't host the exporter |
| `metrics-process` | `metrics-core` + sysinfo process probe | Single binaries wanting RSS/CPU without an HTTP server |
| `metrics` | `metrics-process` + Prometheus exporter + `/metrics` server | Services |

DFE services pull `metrics`; published library crates pull `metrics-core` so they
don't drag a TCP listener into dependents.

---

## Setup

```rust
use hyperi_rustlib::metrics::MetricsManager;

let mut mgr = MetricsManager::new("dfe_loader");

// Construct metrics -- each call also registers a descriptor
let sent = mgr.counter("transport_sent_total", "Messages sent");
let lag = mgr.histogram("send_latency_seconds", "Send latency");

// Declare label keys + group at construction:
mgr.counter_with_labels("transport_sent_total", "Messages sent",
    &["transport", "topic"], "transport");
// Apply label values at recording time:
metrics::counter!("dfe_loader_transport_sent_total",
    "transport" => "kafka", "topic" => "events").increment(1);

mgr.start_server("0.0.0.0:9090").await?;
```

Names are namespace-prefixed automatically -- `counter("foo")` records as
`dfe_loader_foo`. Use `*_with_labels` so the label keys and group land in the
manifest (i.e. nearly always; unlabeled metrics are rare in DFE pipelines).

Single-binary services use `ServiceRuntime` from `cli`, which constructs the
manager, wires the readiness callback, attaches optional `ScalingPressure` and
`MemoryGuard` endpoints, and merges service routes via `start_server_with_routes`.
See [../runtime/SERVICE-RUNTIME.md](../runtime/SERVICE-RUNTIME.md).

---

## The manifest

The registry tracks: name, type, description, unit, label keys, group, bucket
spec, use cases, dashboard hint, app version, git commit, registration timestamp.

`GET /metrics/manifest` returns the full catalogue:

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
      "type": "counter",
      "description": "Messages sent",
      "labels": ["transport", "topic"],
      "group": "transport"
    }
  ]
}
```

The `metrics-manifest` CLI subcommand renders the same JSON to disk without
running the service -- used in CI to keep `docs/metrics-manifest.json` in sync:

```bash
my-app metrics-manifest --output docs/
```

Add metadata after registration:

```rust
mgr.set_use_cases("dfe_loader_send_latency_seconds",
    &["SLO p99 < 500ms", "Page on sustained > 1s"]);
mgr.set_dashboard_hint("dfe_loader_send_latency_seconds", "heatmap");
mgr.set_build_info(env!("CARGO_PKG_VERSION"), env!("GIT_COMMIT"));
```

`DfeMetrics::register(&mgr)` (feature `metrics-dfe`) registers the canonical DFE
metric set -- transport, batch engine, worker pool, memory, scaling -- in one call
so every DFE app exports the same metrics with matching labels.

---

## Endpoints

| Path | Body |
|---|---|
| `/metrics` | Prometheus text |
| `/metrics/manifest` | JSON catalogue |
| `/healthz`, `/health/live` | `{"status":"alive"}` -- process alive |
| `/startupz`, `/health/startup` | 503 until `mgr.mark_started()`, then 200 |
| `/readyz`, `/health/ready` | 200 if readiness callback + [`HealthRegistry`](HEALTH.md) both pass, else 503 |
| `/scaling/pressure` | Float `0.0-1.0` (feature `scaling` + `set_scaling_pressure`) |
| `/memory/pressure` | JSON ratio + bytes (feature `memory` + `set_memory_guard`) |

`/metrics/manifest` is matched before `/metrics` in the prefix-match handler --
don't reorder.

---

## OTel mode

With `otel-metrics`, `MetricsManager` installs an OTel SDK meter provider and
pushes via OTLP. With **both** `metrics` and `otel-metrics`, a `metrics-util`
`FanoutBuilder` composes the two recorders so every macro records to both --
`/metrics` for scrape, OTLP for push. Call `mgr.shutdown_otel()` before exit to
flush the batch exporter, or lose the last interval's data. See
[`OtelMetricsConfig`](../../src/metrics/otel_types.rs) for endpoint / protocol /
batching config.

---

## API surface

| Item | Purpose |
|---|---|
| `MetricsManager::new(namespace)` | Construct + install recorder |
| `MetricsManager::with_config(MetricsConfig)` | Custom namespace, intervals, OTel config |
| `MetricsManager::new_for_test(namespace)` *(test only)* | No global install -- safe for parallel tests |
| `counter` / `gauge` / `histogram` | Construct + auto-register |
| `*_with_labels(name, desc, labels, group)` | Same, with manifest label keys + group |
| `histogram_with_buckets` | Custom bucket spec (captured in manifest) |
| `histogram_with_unit(name, desc, unit)` | Histogram with an explicit unit (bytes/count/...) |
| `histogram_count(name, desc)` | Dimensionless count-distribution histogram |
| `set_readiness_check(fn)` / `mark_started()` | Wire `/readyz` / flip `/startupz` |
| `set_scaling_pressure(Arc<ScalingPressure>)` / `set_memory_guard(Arc<MemoryGuard>)` | Add `/scaling/pressure` / `/memory/pressure` |
| `set_build_info` / `set_use_cases` / `set_dashboard_hint` | Manifest metadata |
| `registry() -> MetricRegistry` | Cloneable handle for embedding `/metrics/manifest` in custom routers |
| `render_handle() -> Option<RenderHandle>` | Cloneable Prometheus text renderer for axum routes |
| `start_server(addr)` / `start_server_with_routes(addr, extra)` | Built-in router / merge service routes |
| `shutdown_otel()` | Flush OTLP batch exporter |
| `DfeMetrics::register(&mgr)` | Canonical DFE metric set (feature `metrics-dfe`) |
| `latency_buckets()` / `size_buckets()` | Standard histogram bucket presets |

---

## Testing

Parallel tests panic if multiple call `MetricsManager::new()` -- the global
Prometheus recorder installs once per process. Use `new_for_test()`: it skips the
install but keeps the registry, descriptor push, and namespacing, and the macros
become no-ops. Most tests verify descriptor registration and naming, not recorded
values; end-to-end recording is verified in the integration suite where one
fixture installs the recorder.

---

## Scaling signals -- emit what could drive scale-out

The horizontal scaling-pressure engine (see
[../deployment/KEDA.md](../deployment/KEDA.md)) can only correlate
metrics that EXIST. **RULE:** if rustlib -- or your app -- can emit a
meaningful, useful metric for something it owns, it SHOULD, by default.
Anything that could factor into scaling (queue depth, upstream
rate-limit, cache-miss storm, per-source throttle) belongs in the
registry AND in a pressure expression. This is the deliberate
config-metrics-scaling interdependency -- see
[../ARCHITECTURE.md](../ARCHITECTURE.md).

rustlib pre-supplies by default (2.8.10): per-pod Kafka consumer-group
lag (`kafka_consumer_group_lag`, summed over THIS pod's ASSIGNED
partitions), CPU as a proper cumulative counter
(`{ns}_process_cpu_seconds_total`), http/grpc server in-flight + shed,
and the engine's own `{ns}_scaling_pressure{name}`,
`{ns}_transport_{inbound,outbound}_pressure_ratio`,
`{ns}_scaling_circuit_open`. Push per-pod transport signals via
`ServiceRuntime::scaling_signals`.

Add yours the same way: emit the metric, then reference `metrics.<name>`
in a `scaling.pressures` CEL expression.

## Units + conventions

Counters end `_total` and are monotonic; base units are `_seconds` /
`_bytes` / `_ratio`; no bool type (use a 0/1 gauge documented as state).
Histograms: `histogram()` is seconds (the latency common case); reach for
`histogram_with_unit` / `histogram_count` for byte/size/count
distributions -- never stamp a count as seconds. A metric RENAME has no
native Prometheus path, so rustlib DUAL-EMITS (old + new) for one release
then drops the old; see [../MIGRATIONS.md](../MIGRATIONS.md) for the
current window.

---

## Related

- [CONFIG.md](CONFIG.md) -- `MetricsConfig` sources from the cascade
- [LOGGING.md](LOGGING.md) -- sampled log + counter is the standard pair
- [HEALTH.md](HEALTH.md) -- `/readyz` consults `HealthRegistry`
- [TRACING.md](TRACING.md) -- OTel-metrics is configured separately from OTel-tracing
- [../runtime/SERVICE-RUNTIME.md](../runtime/SERVICE-RUNTIME.md) -- `ServiceRuntime` wires the manager
- [../AUTO-WIRING.md](../AUTO-WIRING.md), [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md)
- Source: [`src/metrics/mod.rs`](../../src/metrics/mod.rs), [`src/metrics/manifest.rs`](../../src/metrics/manifest.rs), [`src/metrics/dfe.rs`](../../src/metrics/dfe.rs)
