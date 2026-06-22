# Tracing

`otel_tracing::build_tracer_layer(&cfg)?` builds an OpenTelemetry tracer with an
OTLP batch exporter and returns a `tracing-subscriber` layer. Add it to your
subscriber and `tracing::span!` / `#[instrument]` calls become OTel spans that
ship to a collector (Tempo, Jaeger, Honeycomb, Grafana Cloud) with no further
wiring.

It also installs the resulting `SdkTracerProvider` as the **global** OTel tracer
provider. This matters: the transport propagation layer
([`transport::propagation`](../../src/transport/propagation.rs)) reads
`opentelemetry::Context::current()` to format outgoing `traceparent` headers.
Without a global provider there is no current context to read, and the
distributed trace breaks at every transport hop.

Three features cover OTel; turning on `otel` alone does not give you W3C wire
propagation, and the metrics bridge is a different module from the tracing bridge.

---

## Feature breakdown

| Feature | Provides | Pair with |
|---|---|---|
| `otel` | OTel SDK + OTLP exporter crates for other modules | umbrella; rarely useful alone |
| `otel-metrics` | `metrics`-crate bridge to OTel meters | [METRICS.md](METRICS.md) |
| `otel-tracing` | `tracing-opentelemetry` layer bridging spans to OTel spans | this doc |
| `transport-trace` | `traceparent` formatting + injection at transport boundaries | [../transport/OVERVIEW.md](../transport/OVERVIEW.md) |

Full distributed tracing needs `otel-tracing` **and** `transport-trace`: the first
turns internal spans into exportable OTel spans, the second writes the current
context as a `traceparent` header on outgoing Kafka / gRPC / HTTP and reads it on
inbound.

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

// On graceful shutdown -- flush the batch exporter or lose pending spans:
provider.shutdown()?;
```

Env-var overrides resolve at build time, so one config covers dev and prod:

| Var | Overrides |
|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `endpoint` |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `protocol` (`grpc` / `http/protobuf`) |
| `OTEL_SERVICE_NAME` | `service_name` |

`ServiceRuntime` wires all of this when `otel-tracing` is enabled -- apps don't
build the layer manually.

---

## Span flow

```rust
use tracing::{info_span, instrument, Instrument};

// Macro form -- span scope is the closure body
info_span!("process_batch", batch_size = items.len()).in_scope(|| {
    for item in items { process(item); }   // child spans nest automatically
});

// Attribute form -- span covers the whole fn
#[instrument(skip(payload), fields(payload_bytes = payload.len()))]
async fn handle(payload: Vec<u8>) -> Result<()> {
    fetch_metadata().await?;   // span context flows through
    Ok(())
}
```

`#[instrument]` and `.instrument(span)` are the only correct ways to attach a span
across `.await`. **Do not** hold `_guard = span.enter()` across `.await`:

```rust
// WRONG -- guard leaks into other tasks via the runtime's task-locals,
// span context attaches to whatever future the runtime polls next.
let _guard = span.enter();
fetch_data().await;

// RIGHT
let response = handle(req).instrument(span).await;
```

Enforced by `clippy::await_holding_lock` and the audit script in
`standards/languages/RUST.md`.

---

## W3C propagation

With `transport-trace`, `transport::propagation::current_traceparent()` returns the
current OTel context as a W3C `traceparent` header:

```
00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01
```

Transports inject it on outbound (Kafka headers, gRPC metadata, HTTP header) and
extract it on inbound, restoring the OTel context so the inbound span attaches to
the upstream trace. The plumbing lives in each transport backend -- no per-app
wiring.

`current_traceparent()` is gated behind `transport-trace`, **not** `otel`. Without
`transport-trace` it does not compile and transports silently skip propagation.

---

## Exporter

OTLP gRPC over `tonic` by default (endpoint `http://localhost:4317`), OTLP
HTTP/protobuf optional. The batch processor coalesces spans every 5 seconds
(`batch_scheduled_delay_ms`) with a 2 048-span queue (`batch_max_queue_size`);
tune both for high-volume services.

If the collector is unreachable the SDK batch processor drops spans on a full
queue and the process keeps running -- there is no built-in retry, fallback metric,
or per-export warning. Treat the OTLP path as best-effort and rely on collector-
side monitoring for export loss.

---

## API surface

| Item | Purpose |
|---|---|
| `OtelTracingConfig` | `endpoint`, `protocol`, `service_name`, `batch_scheduled_delay_ms`, `batch_max_queue_size` |
| `OtelTracingProtocol::{Grpc, Http}` | OTLP transport choice |
| `build_tracer_layer(&cfg)` | Returns `(layer, provider)`; installs the provider as global |
| `provider.shutdown()` | Flush + stop the batch exporter (call on graceful exit) |
| `transport::propagation::current_traceparent()` *(feature `transport-trace`)* | Format current context for header injection |
| `transport::propagation::format_traceparent_raw(trace_id, span_id, flags)` | Build a header from raw IDs (test / non-OTel use) |
| `transport::propagation::is_valid_traceparent(value)` | Structural validation of an incoming header |
| `transport::propagation::TRACEPARENT_HEADER` | The header name constant |

---

## Related

- [METRICS.md](METRICS.md) -- OTel-metrics is a separate bridge with its own config
- [LOGGING.md](LOGGING.md) -- `tracing::info!` inside a span attaches the log to the span
- [../transport/OVERVIEW.md](../transport/OVERVIEW.md) -- per-transport propagation
- [../AUTO-WIRING.md](../AUTO-WIRING.md), [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md)
- Source: [`src/otel_tracing/mod.rs`](../../src/otel_tracing/mod.rs), [`src/transport/propagation.rs`](../../src/transport/propagation.rs)
