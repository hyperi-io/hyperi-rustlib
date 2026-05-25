# Backends

Seven concrete backends behind the
[transport traits](OVERVIEW.md). Each is gated behind its own
feature flag — apps pull only what they ship.

| Backend | Feature flag | Native dep | Use case |
|---------|--------------|------------|----------|
| Kafka | `transport-kafka` | `librdkafka1` (runtime), `librdkafka-dev` (build) | Production default, persistence, replay |
| gRPC | `transport-grpc` | None (pure Rust — `tonic`) | Inter-service mesh, low latency |
| Memory | `transport-memory` | None | Unit tests, same-process pipelines |
| File | `transport-file` | None | Debugging, audit trails, replay |
| Pipe | `transport-pipe` | None | Unix pipeline composition |
| HTTP | `transport-http` | None | Webhook delivery, REST ingest |
| Redis | `transport-redis` | None (uses `redis` crate) | Edge deployments, lightweight pub/sub |

The Vector-compat shim lives behind `transport-grpc-vector-compat` —
it isn't a separate backend, it's a wire-protocol overlay on the
gRPC server.

---

## Two deployment models (Kafka vs gRPC)

The picture below applies to the Kafka and gRPC backends — the other
five don't make a transit-network choice.

| Model | Persistence | Replay | Latency | Failure mode | Use when |
|-------|-------------|--------|---------|--------------|----------|
| **Kafka-mediated** | Yes (broker disk) | Yes | ~ms | Producer keeps writing if consumer down | Default for staged pipelines, audit-trail required, consumer-failure tolerance matters |
| **Direct gRPC** | No | No | ~µs | Sender fails fast if receiver down | Tight DFE mesh, latency-sensitive, broker overhead unacceptable |

Apps pick per-stage. A typical DFE deployment runs
`receiver → Kafka → loader` (durability at ingress) and
`loader → gRPC → archiver` (latency on the sink) — same binary set,
config-only difference.

---

## Kafka

`rdkafka` 0.39+ with dynamic linking against system librdkafka — see
[../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) for the package matrix.
Profile-based config (`production`, `devtest`) with
`librdkafka_overrides` for fine control. Supports auto-discovery
(`auto_discover: true` with include/exclude regex), SASL/SSL,
suppression rules (`_load` masks `_land` by DFE convention).

```yaml
transport:
  output:
    type: kafka
    kafka:
      profile: production
      brokers: ["kafka-0:9092", "kafka-1:9092"]
      group: dfe-loader
      topics: ["events.land"]
      security_protocol: sasl_ssl
      sasl_mechanism: SCRAM-SHA-512
      sasl_username: dfe
      sasl_password: ${KAFKA_PASSWORD}
```

- **Cancellation safety**: `recv` uses `rdkafka`'s internal poll —
  safe to drop at any `.await`.
- **`is_healthy()`**: tracks an `AtomicBool` flipped to `false` on
  fatal producer/consumer error or on `close()`. Does not probe the
  broker per call.
- **`commit()`**: commits consumer offsets via `rdkafka`'s store-offset
  + commit-async path.

Source: [../../src/transport/kafka/](../../src/transport/kafka/).

---

## gRPC

`tonic` 0.14+, pure Rust. Each gRPC backend can be client-only
(`endpoint` set), server-only (`listen` set), or both. Default
recv-buffer 10k, max-message 16 MB, gzip optional. The
[transport filter engine](FILTER-ENGINE.md) is wired in the same as
every other backend.

```yaml
transport:
  output:
    type: grpc
    grpc:
      endpoint: "http://dfe-loader:6000"
      max_message_size: 16777216
      compression: false
```

- **Cancellation safety**: `recv` reads from an internal mpsc, safe
  to drop. `send` is a single unary RPC — drop cancels cleanly.
- **`is_healthy()`**: `AtomicBool`, also emits `dfe_transport_healthy{transport="grpc"}`
  gauge on every read.
- **`commit()`**: no-op — gRPC has no persistence to advance.

Source: [../../src/transport/grpc/](../../src/transport/grpc/).

### `transport-grpc-vector-compat`

Wire-compat shim for `vector.Vector/PushEvents`. Only used by
`dfe-transform-vector` so legacy Vector sinks can target a DFE gRPC
endpoint without recompile. Enable with `vector_compat: true` in the
gRPC config — the server then accepts both DFE and Vector RPCs on
the same listener. Not a separate backend, not for any other app.

Source: [../../src/transport/vector_compat/](../../src/transport/vector_compat/).

---

## Memory

`tokio::sync::mpsc` bounded channel. Same-process only — sender and
receiver are tied to the same `MemoryTransport` instance. **Not a
deployable backend** — for tests and in-process pipelines (e.g.
unit tests against the `BatchEngine`).

```yaml
transport:
  output:
    type: memory
    memory:
      buffer_size: 1000
      recv_timeout_ms: 100
```

- **Cancellation safety**: `recv` is a `select!` on `recv_timeout`
  and channel `recv` — safe to drop.
- **`is_healthy()`**: `!closed` — atomic flag flipped by `close()`.
- **`commit()`**: advances an internal `AtomicU64` sequence.

Source: [../../src/transport/memory/](../../src/transport/memory/).

---

## File

NDJSON file I/O. Each `send()` appends one newline-delimited line.
Read side tracks a byte offset and persists it to a `.pos` sidecar
file so reads survive restarts. `FileToken` carries the byte offset;
`commit()` writes the highest committed offset to disk.

```yaml
transport:
  output:
    type: file
    file:
      path: "/var/log/dfe/events.ndjson"
      append: true
```

- **Cancellation safety**: read/write are guarded by a `tokio::Mutex`
  — cancellation drops the lock cleanly.
- **`is_healthy()`**: `!closed` atomic flag.

Source: [../../src/transport/file.rs](../../src/transport/file.rs).

---

## Pipe

Reads from stdin, writes to stdout. Newline-delimited, one line per
message. The `key` arg to `send()` is ignored — there's only one
stdout. `PipeToken` is a monotonic sequence number; `commit()` is a
no-op because stdin is forward-only.

```yaml
transport:
  output:
    type: pipe
    pipe:
      recv_timeout_ms: 100
```

- **Cancellation safety**: read path uses `tokio::io::BufReader::read_line`
  — drop-safe.
- **`is_healthy()`**: `!closed`.

Source: [../../src/transport/pipe.rs](../../src/transport/pipe.rs).

---

## HTTP

Two halves, independent: `endpoint` enables send (POST to URL),
`listen` enables receive (embedded axum on `recv_path`, default
`/ingest`). The receive side requires the `http-server` feature
(transitively for axum). Bounded recv-buffer with backpressure.

```yaml
transport:
  output:
    type: http
    http:
      endpoint: "http://collector:8080/ingest"
      # OR for receive:
      listen: "0.0.0.0:8080"
      recv_path: "/ingest"
      recv_buffer_size: 10000
```

- **Cancellation safety**: send is `reqwest`'s async path — drop
  cancels the in-flight request. Receive drains from an internal
  mpsc, drop-safe.
- **`is_healthy()`**: `!closed`. Does not probe the endpoint.

Source: [../../src/transport/http.rs](../../src/transport/http.rs).

---

## Redis

Redis/Valkey Streams via the `redis` crate. Producer writes via
`XADD`, consumer uses `XREADGROUP` with consumer-group semantics.
`commit()` issues `XACK`. Supports `redis://`, `rediss://` (TLS),
and `unix://`. `max_stream_len` enables approximate trimming via
`MAXLEN ~`.

```yaml
transport:
  output:
    type: redis
    redis:
      url: "redis://valkey:6379"
      stream: "events.land"
      group: "dfe"
      consumer: "dfe-loader-1"
      max_stream_len: 100000
      block_ms: 5000
```

- **Cancellation safety**: the `XREADGROUP` block is a single async
  call — cancelling drops the connection back to the pool.
- **`is_healthy()`**: `!closed`. Connection failures surface via
  `SendResult::Fatal` on the next send.
- **`commit()`**: `XACK` on the configured stream/group.

Source: [../../src/transport/redis_transport.rs](../../src/transport/redis_transport.rs).

---

## Filter wiring

Every backend reads `filters_in` and `filters_out` from its own
config section and instantiates a [`TransportFilterEngine`](FILTER-ENGINE.md)
at construction. No backend-specific filter code — the engine is the
same across all seven. Tier-1 filters cost ~50-100 ns when present
and zero when absent.

---

## Related

- [OVERVIEW.md](OVERVIEW.md) — traits, factory, enum dispatch
- [FILTER-ENGINE.md](FILTER-ENGINE.md) — embedded filtering
- [ROUTING.md](ROUTING.md) — per-key dispatch over multiple backends
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) — feature-to-dep table
- [../INTEGRATION.md](../INTEGRATION.md) — DfeApp recipe
- [../pipeline/DLQ.md](../pipeline/DLQ.md) — DLQ sink backends
