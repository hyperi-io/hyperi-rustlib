# Tracing

`otel_tracing::build_tracer_layer(&cfg)?` builds an OpenTelemetry tracer with
an OTLP batch exporter and returns a `tracing-subscriber` layer. Add the
layer to your subscriber and `tracing::span!` / `#[instrument]` calls become
OTel spans that ship to a collector (Tempo, Jaeger, Honeycomb, Grafana Cloud)
without further wiring.

The function also installs the resulting `SdkTracerProvider` as the **global**
OTel tracer provider. This matters because the transport propagation layer
([`transport::propagation`](../../src/transport/propagation.rs)) reads
`opentelemetry::Context::current()` to format outgoing `traceparent` headers —
without a global provider, there is no current context to read, and the
distributed trace breaks at every transport hop.

Three separate features cover OTel, and earlier docs conflated them. Pick
what you need; turning on `otel` alone does not give you W3C wire propagation,
and the metrics bridge is a different module from the tracing bridge.

---

## Feature breakdown

| Feature | Provides | Pair with |
|---|---|---|
| `otel` | The OTel SDK + OTLP exporter crates available to other modules | — (umbrella; rarely useful alone) |
| `otel-metrics` | `metrics-util` recorder that bridges the `metrics` crate to OTel meters | [METRICS.md](METRICS.md) |
| `otel-tracing` | `tracing-opentelemetry` layer that bridges `tracing` spans to OTel spans | this doc |
| `transport-trace` | `traceparent` formatting + injection at transport boundaries | [TRACING.md](TRACING.md) |

For full distributed tracing you need `otel-tracing` **and** `transport-trace`.
The first turns internal `tracing::span!` calls into exportable OTel spans;
the second writes the current span context as a `traceparent` header on
outgoing Kafka / gRPC / HTTP messages and reads it on inbound.

---

## Setup

```rust
use hyperi_rustlib::otel_tracing::{OtelTracingConfig, build_tracer_layer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

let cfg = OtelTracingConfig {
    service_name: "dfe-loader".into(),
    endpoint: "http://otel-collector:4317".into(),
    ..Default::default()
};
let (otel_layer, provider) = build_tracer_layer(&cfg)?;

tracing_subscriber::registry()
    .with(tracing_subscriber::fmt::layer())   // stdout/stderr formatter
    .with(otel_layer)                          // OTLP export
    .init();

// On graceful shutdown — flush the batch exporter or lose pending spans:
provider.shutdown()?;
```

Env-var overrides resolve at build time, so a single config covers dev and
prod:

| Var | Overrides |
|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `endpoint` |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `protocol` (`grpc` / `http/protobuf`) |
| `OTEL_SERVICE_NAME` | `service_name` |

`ServiceRuntime` wires all of this when `otel-tracing` is enabled — apps don't
build the layer manually.

---

## Span flow

```rust
use tracing::{info_span, instrument, Instrument};

// Macro form — span scope is the closure body
info_span!("process_batch", batch_size = items.len()).in_scope(|| {
    for item in items {
        process(item);   // child spans nest automatically
    }
});

// Function-attribute form — span covers the entire fn
#[instrument(skip(payload), fields(payload_bytes = payload.len()))]
async fn handle(payload: Vec<u8>) -> Result<()> {
    fetch_metadata().await?;   // span context flows through
    Ok(())
}
```

`#[instrument]` and `.instrument(span)` are the only correct ways to attach
a span across `.await`. **Do not** hold a `_guard = span.enter()` across
`.await`:

```rust
// WRONG — guard leaks into other tasks via the runtime's task-locals,
// span context gets attached to whatever future the runtime polls next.
let _guard = span.enter();
fetch_data().await;

// RIGHT
async fn handle(req: Request) -> Response {
    fetch_data().await
}
let response = handle(req).instrument(span).await;
```

The guard form is enforced as a lint by `clippy::await_holding_lock` and
flagged in the audit script in `standards/languages/RUST.md`.

---

## W3C propagation

When `transport-trace` is enabled, `transport::propagation::current_traceparent()`
returns the current OTel context formatted as a W3C `traceparent` header:

```
00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01
```

Transports inject it on outbound (Kafka headers, gRPC metadata, HTTP
`traceparent` header) and extract it on inbound, restoring the OTel context
so the inbound span attaches to the upstream trace. The plumbing is in each
transport backend — no per-app wiring.

`current_traceparent()` is **gated behind `transport-trace`**, not `otel`.
Earlier docs implied it was on whenever any OTel feature was active; that
was wrong. Without `transport-trace` the function does not compile and
transports silently skip propagation.

---

## Exporter

OTLP gRPC over `tonic` by default, OTLP HTTP/protobuf optional. The batch
exporter coalesces spans every 5 seconds with a 2 048-span queue. Tune via
`batch_scheduled_delay_ms` and `batch_max_queue_size` for high-volume
services.

The exporter is fail-soft: if the collector is unreachable, spans are
dropped and a warning is logged. The process keeps running. The metric
`otel_tracing_export_failed_total` (when the standard DFE metric set is
registered) carries the count.

---

## API surface

| Item | Purpose |
|---|---|
| `OtelTracingConfig` | `endpoint`, `protocol`, `service_name`, `batch_scheduled_delay_ms`, `batch_max_queue_size` |
| `OtelTracingProtocol::{Grpc, Http}` | OTLP transport choice |
| `build_tracer_layer(&cfg)` | Returns `(layer, provider)`; installs as global |
| `provider.shutdown()` | Flush + stop the batch exporter (call on graceful exit) |
| `transport::propagation::current_traceparent()` *(feature `transport-trace`)* | Format current context for header injection |
| `transport::propagation::format_traceparent_raw(trace_id, span_id, flags)` | Build header from raw IDs (test / non-OTel use) |
| `transport::propagation::is_valid_traceparent(value)` | Structural validation of an incoming header |
| `transport::propagation::TRACEPARENT_HEADER` | The header name constant |

---

## Related

- [METRICS.md](METRICS.md) — OTel-metrics is a separate bridge with its own config
- [LOGGING.md](LOGGING.md) — `tracing::info!` inside a span attaches log to span
- [../transport/OVERVIEW.md](../transport/OVERVIEW.md) — per-transport propagation
- [../AUTO-WIRING.md](../AUTO-WIRING.md), [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md)
- Source: [`src/otel_tracing/mod.rs`](../../src/otel_tracing/mod.rs), [`src/transport/propagation.rs`](../../src/transport/propagation.rs)
