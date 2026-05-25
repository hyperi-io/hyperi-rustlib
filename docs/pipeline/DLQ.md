# DLQ

The DLQ (dead-letter queue) is the last-resort sink for messages the
primary pipeline couldn't deliver — parse errors, validation
failures, persistent transport failure, `TieredSink::SpoolFull`,
poison records. Every DFE service shares one DLQ orchestrator built
from cascade config.

The orchestrator is `Dlq` — a clone-cheap handle wrapping a
`BackgroundSink<DlqEntry>`. Calling `send` queues the entry on an
in-memory mpsc and returns. A drain task pulled out of the runtime
loop coalesces queued entries into batches and writes to one or more
backends. Callers never block on disk, Kafka, HTTP, or Redis I/O.

---

## Four backends

| Backend | Feature | Storage |
|---------|---------|---------|
| File | `dlq` (always available) | NDJSON to disk via the shared `io::NdjsonWriter`, with rotation (`Hourly` default) and gzip on rotation |
| Kafka | `dlq-kafka` (needs `transport-kafka`) | Publish to a dedicated DLQ topic — per-table (`acme.auth` → `acme.auth.dlq`) or single common topic |
| HTTP | `dlq-http` (needs `reqwest`) | POST batched entries as NDJSON |
| Redis | `dlq-redis` (needs `transport-redis`) | `XADD` onto a Redis Stream |

Backends are concrete variants of a `DlqBackend` enum (static
dispatch, no `Box<dyn>`, no `async-trait` macro). Adding a new backend
means extending the enum in rustlib — consumers never construct backend
types directly.

---

## Modes

| Mode | Behaviour |
|------|-----------|
| `Cascade` (default) | Try backends in order (Kafka → File → HTTP → Redis), stop on first success |
| `FanOut` | Write to every enabled backend, succeed if any succeed |
| `FileOnly` | File backend only — no Kafka dependency |
| `KafkaOnly` | Kafka backend only |

Cascade is the production default — Kafka primary, file fallback for
when the broker is unreachable. FanOut is for compliance setups that
need every entry mirrored to two destinations.

---

## Queue-admission semantics

`send` / `try_send` return as soon as the entry is on the in-memory
queue — **not** when it's durably written. This is the queue-admission
contract:

```rust
dlq.send(entry).await?;     // queued (non-blocking)
dlq.flush().await?;          // wait for the drain to flush every entry
                             // queued before this barrier
```

For at-least-once guarantees against backend failure, the caller
must `flush().await` before declaring the entry safe. Most callers
don't need to — DLQ delivery is best-effort by design and the drain
will eventually flush.

`try_send` returns `Err(DlqError::QueueFull)` immediately when the
in-memory queue is full (`Overflow::Drop`). The drop counter is
incremented for visibility — the caller decides whether to log,
escalate, or proceed.

---

## Shutdown

The drain finishes its in-flight batch, drains the remaining queue,
then exits. Triggered by either:

- `CancellationToken::cancel()` passed to `spawn`, or
- All `Dlq` handles dropped (channel closes naturally).

Then `Dlq::shutdown().await` joins the drain task. Idempotent — safe
to call from any clone.

---

## Configuration

```yaml
dlq:
  enabled: true
  mode: cascade
  queue_capacity: 10000         # in-memory mpsc bound
  batch_size: 256                # drain coalescence
  flush_interval_ms: 100         # partial-batch flush
  file:
    enabled: true
    path: /var/spool/dfe/dlq
    rotation: hourly             # hourly | daily | size
    max_age_days: 30
    compress_rotated: true
  kafka:                         # dlq-kafka feature
    enabled: true
    routing: per_table           # per_table | common
    topic_suffix: .dlq
    common_topic: dfe.dlq
    send_timeout_ms: 5000
  http:                          # dlq-http feature
    enabled: false
    url: https://dlq.example/ingest
  redis:                         # dlq-redis feature
    enabled: false
    url: redis://dlq:6379
    stream: dfe.dlq
```

---

## Migration note (v2.7.1+)

The API changed in v2.7.1. The old `Dlq::file_only` / `Dlq::with_kafka`
constructors are gone — every backend mix now goes through `Dlq::spawn`
with the `DlqMode` selecting routing. `DlqBackend` was a trait object
and is now an enum (static dispatch). New methods on the orchestrator:

- `try_send` — non-blocking, returns `QueueFull` on overflow
- `flush` — barrier for durable-write
- `shutdown` — drain + join

`send` semantics changed from "wait for durable write" to
"queue-admission" — see above. All six core DFE apps migrated
2026-05-14.

---

## API surface

| Item | Purpose |
|------|---------|
| `Dlq::disabled()` | No-op handle — `send` succeeds, nothing written |
| `Dlq::spawn(config, service_name, kafka_config, shutdown)` | Build backends, spawn drain, return cloneable handle |
| `try_send(entry) -> Result<(), DlqError>` | Sync-shape queue submission; `QueueFull` on overflow |
| `send(entry).await` | Async submission that awaits queue space |
| `send_batch(entries).await` | Queue many entries (drain coalesces) |
| `flush().await` | Barrier — wait until every entry queued before this call is durably written |
| `shutdown().await` | Wait for drain task to exit cleanly |
| `is_enabled() / mode() / pending() / dropped()` | Introspection |
| `DlqEntry::new(service, error_type, payload)` + `.with_destination(...)`, `.with_source(...)`, `.with_metadata(...)` | Entry builder |
| `DlqSource::kafka(topic, partition, offset) / ::http(url) / ...` | Provenance for the entry |
| `DlqBackend` (enum) | `File / Kafka / Http / Redis` — feature-gated variants |
| `DlqMode` | `Cascade / FanOut / FileOnly / KafkaOnly` |
| `DlqError` | `QueueFull / Closed / File / Kafka / Http / Redis / AllBackendsFailed / NotConfigured` |

`Dlq` is `Clone` — clones share the same drain. The single-owner
shutdown handle lives inside `Arc<AsyncMutex<Option<...>>>` so any
clone can call `shutdown()`.

---

## Source

- [`../../src/dlq/mod.rs`](../../src/dlq/mod.rs)
- [`../../src/dlq/orchestrator.rs`](../../src/dlq/orchestrator.rs) — `Dlq`, `DlqDrain`, cascade/fan-out dispatch
- [`../../src/dlq/backend.rs`](../../src/dlq/backend.rs) — `DlqBackend` enum
- [`../../src/dlq/config.rs`](../../src/dlq/config.rs)
- [`../../src/dlq/entry.rs`](../../src/dlq/entry.rs) — `DlqEntry`, `DlqSource`
- [`../../src/dlq/file.rs`](../../src/dlq/file.rs)
- [`../../src/dlq/kafka.rs`](../../src/dlq/kafka.rs)
- [`../../src/dlq/http.rs`](../../src/dlq/http.rs)
- [`../../src/dlq/redis_dlq.rs`](../../src/dlq/redis_dlq.rs)

---

## Related

- [TIERED-SINK.md](TIERED-SINK.md) — common upstream caller (routes `SpoolFull` / `Fatal` to DLQ)
- [BATCH-ENGINE.md](BATCH-ENGINE.md) — parse errors and pre-route DLQ outcomes flow here
- [../transport/OVERVIEW.md](../transport/OVERVIEW.md) — Kafka backend reuses `KafkaConfig`
- [../transport/FILTER-ENGINE.md](../transport/FILTER-ENGINE.md) — wire-level filter drains DLQ entries here
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) — `dlq`, `dlq-kafka`, `dlq-http`, `dlq-redis`
- [../AUTO-WIRING.md](../AUTO-WIRING.md)
- [../ARCHITECTURE.md](../ARCHITECTURE.md)
