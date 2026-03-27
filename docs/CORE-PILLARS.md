// Project:   hyperi-rustlib
// File:      docs/CORE-PILLARS.md
// Purpose:   Core infrastructure pillars — the auto-wiring principle and integration guide
// Language:  Markdown
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

# Core Infrastructure Pillars

Every module in hyperi-rustlib is expected to integrate with six global infrastructure
pillars: Config, Logging, Metrics, Tracing, Health, and Shutdown. The guiding principle is
simple — **if rustlib owns the abstraction, rustlib owns the observability**. An application
that calls `logger::setup_default()`, `config::setup(opts)`, and `MetricsManager::new()`
at startup automatically receives structured logging, configuration, Prometheus metrics, OTel
tracing, health endpoints, and coordinated graceful shutdown across every rustlib module it
uses. No handles to thread, no opt-in wiring, no extra code.

The six pillars are listed below in the order you should initialise them.

---

## Pillar summary

| Pillar | Singleton | Crate feature | Pattern |
|--------|-----------|---------------|---------|
| Config | `OnceLock<Config>` | `config` | `T::from_cascade()` reads from global figment |
| Logging | Global `tracing` subscriber | `logger` | `tracing::info!()` macros — always available |
| Metrics | Global `metrics` recorder | `metrics` | `metrics::counter!()` macros — no-op if no recorder |
| Tracing | Global OTel subscriber | `otel` | W3C `traceparent` auto-propagated in gRPC/Kafka/HTTP |
| Health | Global `HealthRegistry` | `health` | Modules auto-register; `/readyz` aggregates |
| Shutdown | `OnceLock<CancellationToken>` | `shutdown` | SIGTERM/SIGINT → all modules drain gracefully |

---

## 1. Config

### What it is

An 8-layer configuration cascade backed by [figment](https://docs.rs/figment). Values are
merged in priority order (highest wins):

1. CLI arguments (applied by the application after `config::setup`)
2. Environment variables (e.g. `MYAPP_DATABASE_HOST`)
3. `.env` file
4. `settings.{env}.yaml`
5. `settings.yaml`
6. `defaults.yaml`
7. Org-library built-in defaults
8. Hard-coded fallbacks

### Initialisation

Call once at application startup, before any module is constructed:

```rust
use hyperi_rustlib::config::{self, ConfigOptions};

config::setup(ConfigOptions {
    env_prefix: "MYAPP".into(),
    ..Default::default()
})?;
```

The global `CONFIG` is an `OnceLock<Config>`. After `setup`, any code anywhere can call
`config::get()` without needing a reference passed down.

### Module integration

Modules with configurable behaviour provide a `from_cascade()` associated function that reads
their own section from the global figment:

```rust
// Inside a rustlib module (e.g. src/spool/config.rs)
impl SpoolConfig {
    pub fn from_cascade() -> Result<Self, ConfigError> {
        config::get().unmarshal_key("spool")
    }
}
```

Modules that register themselves in the config registry call
`Config::unmarshal_key_registered(key)` so the admin `/config` endpoint can include their
section in the redacted snapshot.

### Sensitive values

Wrap secrets in `SensitiveString`. Its `Display`, `Debug`, and `Serialize` implementations
always emit `***REDACTED***`; call `.expose()` only where the raw value is required.

---

## 2. Logging

### What it is

Structured logging built on [tracing](https://docs.rs/tracing) +
[tracing-subscriber](https://docs.rs/tracing-subscriber). Automatically selects format based
on environment:

- **Terminal** → coloured, human-readable text
- **Container / CI** → JSON with RFC 3339 timestamps (`UtcTime::rfc_3339()`)

Sensitive field masking is applied transparently via `MaskingWriter` — fields whose names
match the default or configured list (passwords, tokens, API keys, etc.) are replaced with
`***MASKED***` in all output.

### Initialisation

```rust
use hyperi_rustlib::logger;

logger::setup_default()?;
// Respects LOG_LEVEL, LOG_FORMAT, NO_COLOR, LOG_THROTTLE_*, SERVICE_NAME, SERVICE_VERSION
```

Or with explicit options:

```rust
use hyperi_rustlib::logger::{LoggerOptions, LogFormat};
use tracing::Level;

logger::setup(LoggerOptions {
    level: Level::DEBUG,
    format: LogFormat::Json,
    ..Default::default()
})?;
```

The global `LOGGER_INIT: OnceLock<()>` ensures the subscriber is installed exactly once.
Subsequent calls return `LoggerError::AlreadyInitialised`.

### Module integration

Modules simply use tracing macros — no setup or handle required:

```rust
use tracing::{debug, info, warn, error, instrument};

#[instrument(skip(payload), fields(table = %table_name))]
pub async fn flush(&self, table_name: &str, payload: &[u8]) -> Result<()> {
    info!(bytes = payload.len(), "Flushing buffer");
    // ...
}
```

For high-frequency paths, guard formatting behind a level check:

```rust
if tracing::enabled!(tracing::Level::DEBUG) {
    debug!(first_bytes = ?&payload[..8.min(payload.len())], "Payload preview");
}
```

Log flood protection is opt-in via `ThrottleConfig`. Enable with `LOG_THROTTLE_ENABLED=true`.

---

## 3. Metrics

### What it is

A thin wrapper around the [metrics](https://docs.rs/metrics) crate facade, with support for
two backends: Prometheus (HTTP scrape endpoint) and OpenTelemetry OTLP (push). Both can be
active simultaneously — `MetricsManager` fans out to whichever backends are configured.

The `metrics::counter!`, `metrics::gauge!`, and `metrics::histogram!` macros are **no-ops**
if no recorder has been installed. This means any rustlib module can emit metrics
unconditionally and the cost is zero in applications that have not called
`MetricsManager::new()`.

### Initialisation

```rust
use hyperi_rustlib::MetricsManager;

let mut manager = MetricsManager::new("myapp");
manager.start_server("0.0.0.0:9090").await?;
// Prometheus now available at http://host:9090/metrics
```

Process metrics (CPU, RSS, open file descriptors, uptime) and container cgroup metrics
(memory limit, memory usage) are registered automatically.

### Module integration

Modules gate metric emission on the `metrics` feature so they compile cleanly in projects
that do not pull the feature in:

```rust
#[cfg(feature = "metrics")]
{
    metrics::counter!("mymodule_records_processed_total").increment(batch.len() as u64);
    metrics::histogram!("mymodule_flush_duration_seconds").record(elapsed.as_secs_f64());
}
```

Follow the [DFE Metrics Standard](dfe-metrics.md) for naming conventions:
`{namespace}_{domain}_{name}_{unit}`, counters always end in `_total`.

---

## 4. Tracing (OTel)

### What it is

Distributed tracing via OpenTelemetry, exported over OTLP (gRPC or HTTP). When the `otel`
feature is enabled, rustlib transports automatically propagate the W3C
[Trace Context](https://www.w3.org/TR/trace-context/) `traceparent` header across Kafka
messages, gRPC calls, and HTTP requests.

The `traceparent` format is `00-{32 hex trace_id}-{16 hex span_id}-{02 hex flags}` (55
characters total, e.g. `00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01`).

### Initialisation

OTel is initialised by configuring the global tracing subscriber to include an OTel layer
(typically via the `otel` feature of rustlib). The OTLP exporter endpoint is read from the
`OTEL_EXPORTER_OTLP_ENDPOINT` environment variable (default: `http://localhost:4317`).

### Module integration

Transports inject and extract `traceparent` transparently. The propagation helpers are
available for use in custom transport adapters:

```rust
use hyperi_rustlib::transport::propagation;

// Inject current span context into outgoing headers
if let Some(tp) = propagation::current_traceparent() {
    headers.insert(propagation::TRACEPARENT_HEADER, tp);
}

// Validate an incoming traceparent header before use
if propagation::is_valid_traceparent(&incoming_value) {
    // restore context...
}
```

`current_traceparent()` returns `None` if there is no active span or the `otel` feature is
not enabled, so call sites do not need feature gates of their own.

---

## 5. Health

### What it is

A global registry of named health-check callbacks. Components register a closure at
construction time; the registry aggregates statuses on demand to determine overall service
health. Two aggregation modes:

- `is_healthy()` — returns `true` only if **every** registered component is `Healthy`
- `is_ready()` — returns `true` if **no** component is `Unhealthy` (`Degraded` is acceptable)

An empty registry is considered healthy (vacuously true). The HTTP server feature exposes
`GET /healthz` (liveness, maps to `is_healthy`) and `GET /readyz` (readiness, maps to
`is_ready`).

### Initialisation

No explicit initialisation required. The `REGISTRY: OnceLock<HealthRegistry>` is
auto-initialised on first use via `get_or_init`. Applications using the `http-server` feature
get the health endpoints automatically.

### Module integration

Register at module construction, before the module starts serving traffic:

```rust
use hyperi_rustlib::health::{HealthRegistry, HealthStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

let connected = Arc::new(AtomicBool::new(false));
let flag = connected.clone();

HealthRegistry::register("kafka-consumer", move || {
    if flag.load(Ordering::Relaxed) {
        HealthStatus::Healthy
    } else {
        HealthStatus::Unhealthy
    }
});
```

The callback must be cheap — it is called on every health query. Read an `AtomicBool`, check
a cached enum, or inspect a circuit-breaker state. Never do I/O inside a health check.

For detailed health responses (e.g. a `/health/detailed` endpoint):

```rust
#[cfg(feature = "serde_json")]
let json = HealthRegistry::to_json();
// {"status":"degraded","components":[{"name":"kafka-consumer","status":"healthy"},…]}
```

---

## 6. Shutdown

### What it is

A single `CancellationToken` (from [tokio-util](https://docs.rs/tokio-util)) shared globally
via `OnceLock`. When the token is cancelled, every module that holds a clone should begin
draining in-flight work and then exit its main loop.

Signal handling covers:

- `SIGTERM` — sent by Kubernetes during pod termination
- `SIGINT` — sent by Ctrl+C during local development

### Initialisation

Install the signal handler once, at application startup:

```rust
use hyperi_rustlib::shutdown;

let token = shutdown::install_signal_handler();
// token is a clone of the global token — safe to pass to workers
```

### Module integration

Each long-running task selects on the cancellation token alongside its normal work:

```rust
use hyperi_rustlib::shutdown;

async fn run_consumer(mut rx: mpsc::Receiver<Record>) {
    let token = shutdown::token();
    loop {
        tokio::select! {
            _ = token.cancelled() => {
                // Drain any buffered records before exiting
                while let Ok(record) = rx.try_recv() {
                    process(record).await;
                }
                break;
            }
            Some(record) = rx.recv() => {
                process(record).await;
            }
        }
    }
}
```

For nested task trees, use `token.child_token()` so child tasks are cancelled along with the
parent:

```rust
let child = shutdown::token().child_token();
tokio::spawn(async move {
    child.cancelled().await;
    // clean up
});
```

Programmatic shutdown (e.g. fatal error, test teardown) is available via `shutdown::trigger()`.

---

## Minimal application bootstrap

The following ~15 lines are the minimum setup for a well-behaved DFE application:

```rust
use hyperi_rustlib::{config, logger, shutdown, MetricsManager};
use hyperi_rustlib::config::ConfigOptions;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Config — must be first (logger reads LOG_LEVEL etc. from cascade)
    config::setup(ConfigOptions {
        env_prefix: "MYAPP".into(),
        ..Default::default()
    })?;

    // 2. Logging — auto-detects terminal vs container
    logger::setup_default()?;

    // 3. Metrics — Prometheus at :9090
    let mut metrics = MetricsManager::new("myapp");
    metrics.start_server("0.0.0.0:9090").await?;

    // 4. Shutdown — install signal handler, get token for workers
    let token = shutdown::install_signal_handler();

    // 5. Run — pass token to workers; health is wired automatically
    run(token).await
}
```

Health check registration happens inside `run()` as each subsystem is constructed. The HTTP
server (if used) automatically serves `/healthz` and `/readyz`.

---

## Module integration checklist

When adding a new module or feature to rustlib, apply each rule that fits:

1. **Configurable behaviour** → provide a `from_cascade()` associated function that calls
   `config::get().unmarshal_key("your_key")`. Register the key with
   `Config::unmarshal_key_registered` if the section should appear in the admin endpoint.

2. **I/O or processing** → emit `#[cfg(feature = "metrics")]` counters, gauges, and
   histograms at natural instrumentation points (record counts, byte counts, error counts,
   latency histograms). Follow the DFE naming convention.

3. **Can fail or has interesting state** → add `tracing::` calls at appropriate levels
   (`info!` on lifecycle events, `warn!` on recoverable issues, `error!` on failures). Use
   `#[instrument]` on public async methods. Guard expensive formatting with
   `tracing::enabled!`.

4. **Affects service health** → register a callback with `HealthRegistry::register` at module
   construction. The callback reads an `AtomicBool` or similar cheap state — no I/O.

5. **Long-running background task** → accept `shutdown::token()` (or a child token) and
   select on `token.cancelled()` in the main loop. Drain gracefully before returning.

6. **Propagates context across a transport boundary** → inject `current_traceparent()` into
   outgoing headers/metadata and extract it on the receiving side to restore the OTel context.
