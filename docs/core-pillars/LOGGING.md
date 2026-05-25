# Logging

`logger::setup_default()` installs a `tracing-subscriber` once at startup, then
every module reaches for `tracing::info!` / `tracing::error!` / `#[instrument]`
without passing handles around. Output format is decided at install time by
sniffing the terminal: TTY without `NO_COLOR` gets human-readable coloured
text, anything else (containers, CI, piped output) gets line-delimited JSON
with RFC 3339 UTC timestamps.

The subscriber also wraps stderr in a `MaskingWriter` that scans every line
for sensitive field names and replaces their values with `[REDACTED]` before
flushing. The mask runs in both JSON and text modes — `password=secret123`
and `"password":"secret123"` both come out redacted. There's no per-call-site
opt-in; you get masking by default and disable it explicitly if you don't
want it.

JSON output is also enriched on the fly with `service` / `version` (from
`SERVICE_NAME` / `SERVICE_VERSION` env vars or `DfeApp` auto-population) and
K8s context fields (`pod_name`, `namespace`, `node_name`) sourced from
[`env::runtime_context`](../../src/env.rs). On bare metal those K8s fields
are absent.

---

## Setup

```rust
use hyperi_rustlib::logger;

// Env-driven defaults — what DfeApp calls
logger::setup_default()?;

// Or explicit
use hyperi_rustlib::logger::{setup, LoggerOptions, LogFormat};
setup(LoggerOptions {
    level: tracing::Level::DEBUG,
    format: LogFormat::Json,
    enable_masking: true,
    ..Default::default()
})?;
```

`setup_default()` reads:

| Var | Effect |
|---|---|
| `LOG_LEVEL` / `RUST_LOG` | Level filter — falls back to `EnvFilter` parsing for module-specific filters (`hyper=warn,my_app=debug`) |
| `LOG_FORMAT` | `json` / `text` / `auto` (default) |
| `NO_COLOR` | Disable ANSI colour even on a TTY |
| `LOG_THROTTLE_ENABLED` | Turn on global `tracing-throttle` token bucket (default off) |
| `LOG_THROTTLE_BURST` | Burst capacity (default 50) |
| `LOG_THROTTLE_RATE` | Recovery tokens/sec (default 1.0) |
| `SERVICE_NAME` / `SERVICE_VERSION` | Injected into JSON lines |

`LogFormat::Auto` resolves to `Text` when stderr is a TTY and `NO_COLOR` is
unset, `Json` otherwise.

---

## Sensitive-field masking

The built-in list (case-insensitive substring match on field name) covers
password / token / api_key / secret / credential / auth / bearer / private_key
/ client_secret / refresh_token / access_token / ssn / credit_card / cvv / pin.
See [`default_sensitive_fields()`](../../src/logger/masking.rs).

JSON mode parses each line, walks the object tree, and redacts at any depth.
Text mode scans for `name=value` and `name="value"` patterns. Both write
`[REDACTED]` in place.

Extend the list rather than replace it:

```rust
let mut fields = logger::default_sensitive_fields();
fields.push("internal_session_id".into());
setup(LoggerOptions { sensitive_fields: fields, ..Default::default() })?;
```

For values that don't have a recognisable field name (e.g. a token embedded
in a URL), use `SensitiveString` from [config](CONFIG.md) — it serialises
as `***REDACTED***` regardless of caller. The masking layer is the catch-all,
not the primary defence.

---

## Flood-control helpers

Per-call-site rate limiting without a new dep, built on lock-free atomics in
[`logger/helpers.rs`](../../src/logger/helpers.rs). All three are `~5 ns` when
suppressed.

```rust
use std::sync::atomic::{AtomicBool, AtomicU64};
use hyperi_rustlib::logger::{log_state_change, log_sampled, log_debounced};

// 1. Sustained conditions — log only on the transition
static PRESSURE_HIGH: AtomicBool = AtomicBool::new(false);
if log_state_change(&PRESSURE_HIGH, current > threshold) {
    tracing::warn!(current, threshold, "memory pressure crossed threshold");
}

// 2. Hot-path errors — log first + every Nth
static SEND_ERRORS: AtomicU64 = AtomicU64::new(0);
if log_sampled(&SEND_ERRORS, 1000) {
    tracing::error!("kafka send failed");
}

// 3. Tight poll loops — log at most once per N ms
static LAST_WARN: AtomicU64 = AtomicU64::new(0);
if log_debounced(&LAST_WARN, 5_000) {
    tracing::warn!("udp recv backlog");
}
```

Pair sampled / debounced calls with a metric counter — the helper controls
emission, the metric carries the count.

For service-wide deduplication of identical events (different sites, same
signature), enable `LOG_THROTTLE_ENABLED=1` — the `tracing-throttle` layer
deduplicates by event signature with a token bucket. High-cardinality fields
(`request_id`, `trace_id`, `span_id`) are excluded from the signature by
default so per-request log lines don't collapse into one.

---

## API surface

| Item | Purpose |
|---|---|
| `logger::setup_default()` | Env-driven install — `DfeApp` calls this |
| `logger::setup(opts)` | Explicit install |
| `LoggerOptions` | `level`, `format`, `add_source`, `enable_masking`, `sensitive_fields`, `span_events`, `throttle`, `service_name`, `service_version` |
| `LogFormat::{Json, Text, Auto}` | Format selection; `Auto` resolves on `setup` |
| `ThrottleConfig` | `enabled`, `burst`, `rate`, `max_signatures`, `excluded_fields` |
| `logger::default_sensitive_fields() -> Vec<String>` | Baseline mask list — extend, don't replace |
| `logger::mask_sensitive_string(input, patterns) -> String` | Ad-hoc redaction for strings outside the logger |
| `MaskingLayer` | Detector struct, exposed for custom integrations |
| `MaskingWriter<W>` | The writer wrapper installed by `setup` |
| `log_state_change(&AtomicBool, new) -> bool` | Transition gate |
| `log_sampled(&AtomicU64, every_n) -> bool` | Nth-occurrence gate |
| `log_debounced(&AtomicU64, period_ms) -> bool` | Time-window gate |
| `SecurityEvent` / `SecurityOutcome` | Pre-built event types in [`logger/security.rs`](../../src/logger/security.rs) for audit-log discipline |

---

## Testing

The subscriber installs globally, once per process. Tests that need a fresh
logger should not call `setup_default` — write to a `tracing` `MakeWriter`
captured in the test instead, or skip logger init entirely (the `tracing`
macros are no-ops when no subscriber is set).

For tests that need to mutate env vars, use the `temp-env` crate —
`std::env::set_var` is `unsafe` in edition 2024 and the crate sets
`#![forbid(unsafe_code)]`.

---

## Related

- [CONFIG.md](CONFIG.md) — `SensitiveString` for value-level redaction
- [METRICS.md](METRICS.md) — `tracing::warn!` and metrics counters often go together
- [TRACING.md](TRACING.md) — `tracing::span!` and `#[instrument]` feed OTel
- [../AUTO-WIRING.md](../AUTO-WIRING.md) — singleton install pattern
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) — `logger`
- Source: [`src/logger/mod.rs`](../../src/logger/mod.rs), [`src/logger/helpers.rs`](../../src/logger/helpers.rs), [`src/logger/masking.rs`](../../src/logger/masking.rs)
