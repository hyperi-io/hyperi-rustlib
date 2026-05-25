# Routing

`RoutedSender` dispatches `send(key, payload)` to one of N backend
senders based on the key — different topics, tenants, or stream IDs
land on different transports. Sits on top of [`AnySender`](OVERVIEW.md);
no new backend, no new trait.

---

## When to use it

| Stage | Routed? | Why |
|-------|---------|-----|
| `dfe-receiver` | **Yes** | Push ingress fans tenant traffic out to per-tenant Kafka topics |
| `dfe-fetcher` | **Yes** | Pull ingress (AWS, Azure, M365, GCP) maps each source to its own destination |
| `dfe-loader` | No | One ClickHouse sink — 1:1 transport |
| `dfe-archiver` | No | One object-storage sink — 1:1 transport |
| `dfe-transform-vrl` | No | Transform stage — 1:1 in, 1:1 out |
| `dfe-transform-vector` | No | Vector owns its own routing config — anomaly |

**Constraint**: only data originators route. Mid-tier transforms,
loaders, and archivers see a single inbound stream and produce a
single outbound stream — they don't need `RoutedSender`. Per the
DFE routing model, push the routing decision as close to ingress as
possible.

---

## Typical use case

A receiver accepts gRPC pushes from many tenants on one listen
socket, then fans messages out to per-tenant Kafka topics (durability)
while a small set of audit events go to a dedicated gRPC archiver
for low-latency capture.

```text
              ┌─────────────────────────────────────────┐
gRPC ingress  │  RoutedSender                            │
─────────────►│   "events.land" → Kafka topic events.land│
              │   "events.load" → Kafka topic events.load│
              │   "audit.land"  → gRPC archiver:6000     │
              │   default       → Kafka (catch-all)      │
              └─────────────────────────────────────────┘
```

---

## Config shape

```yaml
transport:
  output:
    type: routed
    default:
      type: kafka
      kafka:
        brokers: ["kafka:9092"]
    routes:
      events.land:
        type: kafka
        kafka:
          brokers: ["kafka:9092"]
      events.load:
        type: kafka
        kafka:
          brokers: ["kafka:9092"]
      audit.land:
        type: grpc
        grpc:
          endpoint: "http://archiver:6000"
```

Each route value is a full `TransportConfig` — any backend
[BACKENDS.md](BACKENDS.md) supports is fair game. The `default` is
optional; without it, an unknown key returns `SendResult::Fatal`.

---

## API

```rust
use std::collections::HashMap;
use hyperi_rustlib::transport::{
    AnySender, RoutedSender, TransportConfig, TransportSender,
};

// Build from config structs:
let mut routes = HashMap::new();
routes.insert("events.land".into(), kafka_cfg.clone());
routes.insert("audit.land".into(), grpc_cfg.clone());

let sender = RoutedSender::from_route_configs(routes, Some(default_cfg)).await?;

sender.send("events.land", payload).await;   // → Kafka topic
sender.send("audit.land", payload).await;    // → gRPC archiver
sender.send("anything-else", payload).await; // → default
```

Construct directly from pre-built senders when the config indirection
isn't useful (tests, dynamic wiring):

```rust
let mut routes = HashMap::new();
routes.insert("a".into(), AnySender::Memory(/* ... */));
let sender = RoutedSender::new(routes, Some(default_sender));
```

---

## Composition with `AnySender`

`RoutedSender` **owns** N `AnySender`s — one per route plus the
default. Each `AnySender` is itself enum-dispatched over the seven
backends. So `RoutedSender::send`:

1. `HashMap::get(key)` to find the route (or fall back to default).
2. `AnySender::send(key, payload).await` on the chosen sender.
3. Backend's own `send` runs — Kafka, gRPC, etc.

Two layers of dispatch, both monomorphised by the compiler. The
route lookup is a `HashMap<String, AnySender>::get` — single hash
+ equality compare, no allocation when the key is `&str`.

`RoutedSender` itself implements `TransportSender` — anywhere an
app expects `impl TransportSender`, a routed sender drops in. It
implements `TransportBase` too — `close()` cascades to every route
and the default, `is_healthy()` reports `false` if any constituent
sender is unhealthy.

---

## Performance

Per `send()` call, on top of the chosen backend's own cost:

| Step | Cost |
|------|------|
| `HashMap::get(&str)` lookup | ~20-40 ns (SipHash + compare) |
| Match on `AnySender` variant | <5 ns (jump table) |
| Backend `send` | µs to ms — dominates |

The routing overhead is at most 1% of any real backend's send cost.
No allocation, no `Arc::clone`, no async indirection. The metric
`dfe_transport_sent_total{transport="routed", route=<key>}` records
the route taken.

---

## Empty / missing routes

Behaviour when `key` is not in `routes`:

| Config | Result |
|--------|--------|
| `default` is set | Falls through to the default sender |
| `default` is unset | `SendResult::Fatal(TransportError::Config(...))` |

Mark a default unless the calling code is OK with the fatal — for
ingress paths this is usually wanted (unknown tenant → catch-all
"unknown.tenant" topic for ops to triage). For audit paths the fatal
is the right default (no silent drop).

For "route exists but send fails", `RoutedSender` returns the
chosen backend's `SendResult` unchanged — backpressure, fatal, and
filter-DLQ propagate up. Caller distinguishes by matching on the
result.

---

## API surface

| Item | Purpose |
|------|---------|
| `RoutedSender::new(routes, default)` | Construct from pre-built `AnySender`s |
| `RoutedSender::from_route_configs(routes, default).await` | Construct from per-route `TransportConfig`s |
| `RoutedSender::send(key, payload).await` | Dispatch by key, fall to default if missing |
| `RoutedSender::route_keys() -> Vec<&str>` | List configured route keys |
| `RoutedSender::has_route(key) -> bool` | Check if a specific key has a route |
| `RoutedSender::has_default() -> bool` | Check if a default sender is wired |
| `RoutedSender::close().await` | Cascade close to every route + default |
| `RoutedSender::is_healthy() -> bool` | True only if every constituent sender is healthy |
| `RoutedSender::name() -> &'static str` | Returns `"routed"` |

Source: [../../src/transport/routed.rs](../../src/transport/routed.rs).

---

## Related

- [OVERVIEW.md](OVERVIEW.md) — traits, `AnySender`, enum dispatch
- [BACKENDS.md](BACKENDS.md) — concrete backends each route can pick
- [FILTER-ENGINE.md](FILTER-ENGINE.md) — filters run per-backend, after routing
- [../ARCHITECTURE.md](../ARCHITECTURE.md) — DFE stage model
- [../INTEGRATION.md](../INTEGRATION.md) — wiring for receiver/fetcher
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) — feature flags per backend
