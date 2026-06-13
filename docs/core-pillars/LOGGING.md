# Logging

`logger::setup_default()` installs a `tracing-subscriber` once at startup; every
module then uses `tracing::info!` / `error!` / `#[instrument]` with no handle
passing. Format is decided at install time by sniffing the terminal: TTY without
`NO_COLOR` gets coloured human-readable text, everything else (containers, CI,
pipes) gets line-delimited JSON with RFC 3339 UTC timestamps.

The subscriber wraps stderr in a `MaskingWriter` that redacts sensitive field
values at the write boundary -- both `password=secret123` and
`"password":"secret123"` come out `[REDACTED]`. Masking is on by default; disable
it explicitly if you must. JSON lines are also enriched with `service` /
`version` (from `SERVICE_NAME` / `SERVICE_VERSION` or `DfeApp`) and K8s context
(`pod_name`, `namespace`, `node_name`) from
[`env::runtime_context`](../../src/env.rs); those K8s fields are absent on bare
metal.

---

## Setup

```rust
use hyperi_rustlib::logger;
logger::setup_default()?;                       // env-driven -- what DfeApp calls

// or explicit
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
| `LOG_LEVEL` / `RUST_LOG` | Level filter; falls back to `EnvFilter` for per-module filters (`hyper=warn,my_app=debug`) |
| `LOG_FORMAT` | `json` / `text` / `auto` (default) |
| `NO_COLOR` | Disable ANSI colour even on a TTY |
| `LOG_THROTTLE_ENABLED` | Global `tracing-throttle` token bucket (default off) |
| `LOG_THROTTLE_BURST` | Burst capacity (default 50) |
| `LOG_THROTTLE_RATE` | Recovery tokens/sec (default 1.0) |
| `SERVICE_NAME` / `SERVICE_VERSION` | Injected into JSON lines |

`LogFormat::Auto` resolves to `Text` on a TTY with `NO_COLOR` unset, `Json`
otherwise.

---

## Sensitive-field masking

Applied at the write boundary, so it is the catch-all regardless of how a value
reaches a log line. Default list (case-insensitive substring match on field name)
covers password / token / api_key / secret / credential / auth / bearer /
private_key / client_secret / refresh_token / access_token / ssn / credit_card /
cvv / pin and their common spellings. See
[`default_sensitive_fields()`](../../src/logger/masking.rs).

JSON mode walks the object tree and redacts at any depth; text mode scans for
`name=value` and `name="value"`. Both write `[REDACTED]` in place.

Extend rather than replace:

```rust
let mut fields = logger::default_sensitive_fields();
fields.push("internal_session_id".into());
setup(LoggerOptions { sensitive_fields: fields, ..Default::default() })?;
```

For values with no recognisable field name (e.g. a token in a URL), use
`SensitiveString` from [config](CONFIG.md) -- it serialises as `***REDACTED***`
regardless of caller.

---

## Flood-control helpers

Per-call-site rate limiting on lock-free atomics in
[`logger/helpers.rs`](../../src/logger/helpers.rs); all three are ~5 ns when
suppressed.

```rust
use std::sync::atomic::{AtomicBool, AtomicU64};
use hyperi_rustlib::logger::{log_state_change, log_sampled, log_debounced};

// 1. Sustained conditions -- log only on the transition
static PRESSURE_HIGH: AtomicBool = AtomicBool::new(false);
if log_state_change(&PRESSURE_HIGH, current > threshold) {
    tracing::warn!(current, threshold, "memory pressure crossed threshold");
}

// 2. Hot-path errors -- log first + every Nth
static SEND_ERRORS: AtomicU64 = AtomicU64::new(0);
if log_sampled(&SEND_ERRORS, 1000) { tracing::error!("kafka send failed"); }

// 3. Tight poll loops -- log at most once per N ms
static LAST_WARN: AtomicU64 = AtomicU64::new(0);
if log_debounced(&LAST_WARN, 5_000) { tracing::warn!("udp recv backlog"); }
```

Pair sampled / debounced calls with a metric counter -- the helper gates
emission, the metric carries the count.

For service-wide dedup of identical events (different sites, same signature), set
`LOG_THROTTLE_ENABLED=1`. The `tracing-throttle` layer dedups by event signature
with a token bucket; high-cardinality fields (`request_id`, `trace_id`,
`span_id`) are excluded from the signature by default so per-request lines don't
collapse into one.

---

## API surface

| Item | Purpose |
|---|---|
| `logger::setup_default()` | Env-driven install -- `DfeApp` calls this |
| `logger::setup(opts)` | Explicit install |
| `LoggerOptions` | `level`, `format`, `add_source`, `enable_masking`, `sensitive_fields`, `span_events`, `throttle`, `service_name`, `service_version` |
| `LogFormat::{Json, Text, Auto}` | Format; `Auto` resolves on `setup` |
| `ThrottleConfig` | `enabled`, `burst`, `rate`, `max_signatures`, `excluded_fields` |
| `logger::default_sensitive_fields() -> Vec<String>` | Baseline mask list -- extend, don't replace |
| `logger::mask_sensitive_string(input, patterns) -> String` | Ad-hoc redaction outside the logger |
| `MaskingLayer` / `MaskingWriter<W>` | Detector + the writer wrapper `setup` installs |
| `log_state_change(&AtomicBool, new) -> bool` | Transition gate |
| `log_sampled(&AtomicU64, every_n) -> bool` | Nth-occurrence gate |
| `log_debounced(&AtomicU64, period_ms) -> bool` | Time-window gate |
| `SecurityEvent` / `SecurityOutcome` | Audit-log event types in [`logger/security.rs`](../../src/logger/security.rs) |

---

## Testing

The subscriber installs globally, once per process. Tests needing a fresh logger
should not call `setup_default` -- capture a `tracing` `MakeWriter`, or skip
logger init (the macros are no-ops with no subscriber). For env-var mutation use
`temp-env` -- `std::env::set_var` is `unsafe` in edition 2024 and this crate
forbids unsafe.

---

## Related

- [CONFIG.md](CONFIG.md) -- `SensitiveString` for value-level redaction
- [METRICS.md](METRICS.md) -- sampled log + metric counter is the standard pair
- [TRACING.md](TRACING.md) -- spans and `#[instrument]` feed OTel
- [../AUTO-WIRING.md](../AUTO-WIRING.md) -- singleton install pattern
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) -- `logger`
- Source: [`src/logger/mod.rs`](../../src/logger/mod.rs), [`src/logger/helpers.rs`](../../src/logger/helpers.rs), [`src/logger/masking.rs`](../../src/logger/masking.rs)
