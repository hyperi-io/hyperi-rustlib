# Transport Layer

The transport layer provides a unified abstraction for moving messages between
DFE pipeline stages. Applications interact with a common trait interface while
the concrete backend (Kafka, gRPC, file, etc.) is selected at runtime via
configuration.

## Trait Architecture

The transport abstraction is split into three complementary traits:

| Trait | Purpose | Key Methods |
|-------|---------|-------------|
| `TransportBase` | Lifecycle management | `close()`, `is_healthy()`, `name()` |
| `TransportSender` | Send capability | `send(key, payload) -> SendResult` |
| `TransportReceiver<Token>` | Receive + commit | `recv() -> Vec<Message>`, `commit(tokens)` |

A blanket `Transport` impl exists for any type implementing both `TransportSender`
and `TransportReceiver`:

```rust
pub trait Transport: TransportSender + TransportReceiver {}
impl<T: TransportSender + TransportReceiver> Transport for T {}
```

Each transport defines its own `CommitToken` type (e.g., `KafkaToken` wraps an
offset, `GrpcToken` wraps a sequence number). This keeps the receiver side
type-safe while the sender side uses enum dispatch.

## Factory Pattern

`AnySender` is an enum (not `dyn Trait`) because `TransportSender::send()` returns
`impl Future`, which is not object-safe. The factory reads transport type from
the config cascade:

```rust
use hyperi_rustlib::transport::factory::AnySender;

let sender = AnySender::from_config("transport.output")?;
sender.send("events", b"{\"msg\":\"hello\"}").await?;
```

`AnySender` has variants for each feature-gated backend. It implements
`TransportBase` and `TransportSender` but not `TransportReceiver` — it is
send-side only by design.

## Backends

### Kafka (`transport-kafka`)

Production default. Consumer groups, SASL/SCRAM, TLS, rdkafka stats.

```yaml
transport:
  output:
    type: kafka
    brokers: ["kafka1:9092", "kafka2:9092"]
    topics: ["events"]
    security_protocol: sasl_ssl
    sasl_mechanism: SCRAM-SHA-512
    sasl_username: svc-loader
    sasl_password: ${KAFKA_PASSWORD}
```

**System dependency:** `librdkafka-dev` (build), `librdkafka1` (runtime).

### gRPC (`transport-grpc`)

Low-latency DFE-to-DFE mesh. Bidirectional streaming. Vector-compatible
protocol via `transport-grpc-vector-compat`.

```yaml
transport:
  output:
    type: grpc
    endpoint: "http://transform:4317"
    buffer_size: 1000
```

### HTTP (`transport-http`)

Webhook delivery (send) and embedded axum receiver (recv, requires `http-server`).

```yaml
transport:
  output:
    type: http
    endpoint: "http://loader:8080/ingest"
    timeout_ms: 5000
```

### Redis/Valkey Streams (`transport-redis`)

`XADD`/`XREADGROUP`/`XACK` with consumer groups. Works with both Redis and
Valkey (same wire protocol).

```yaml
transport:
  output:
    type: redis
    url: "redis://redis:6379"
    stream: events
    group: dfe-loader
    consumer: worker-1
```

### File (`transport-file`)

NDJSON files with position tracking via `.pos` sidecar. Useful for debugging,
audit trails, and replay.

```yaml
transport:
  output:
    type: file
    path: /data/events.ndjson
    append: true
```

### Pipe (`transport-pipe`)

stdin/stdout for Unix pipeline composition. Useful for CLI tool integration
and sidecar patterns.

```yaml
transport:
  input:
    type: pipe
    recv_timeout_ms: 1000
```

### Memory (`transport-memory`)

In-process bounded channel for unit testing. No health registration,
no metrics (test infrastructure only).

```yaml
transport:
  output:
    type: memory
    buffer_size: 100
```

## Deployment Modes

Two primary modes for connecting pipeline stages:

| Mode | Transport | Persistence | Replay | Latency |
|------|-----------|-------------|--------|---------|
| Kafka-mediated | Kafka | Disk, configurable retention | Yes | Higher (broker hop) |
| Direct gRPC | gRPC | None (in-flight only) | No | Low (point-to-point) |

**Kafka-mediated** is the default for production pipelines where persistence,
consumer group rebalancing, and replay are needed.

**Direct gRPC** is used where low latency matters more than persistence
(e.g., receiver → transform in the same pod).

## Redis vs Kafka Comparison

| Dimension | Kafka | Redis Streams |
|-----------|-------|---------------|
| Persistence | Disk, configurable retention | Memory + optional AOF/RDB |
| Throughput | PB/day proven at scale | Lower, single-threaded command processing |
| Consumer groups | Native, automatic rebalancing | `XREADGROUP`, manual consumer management |
| Ordering | Per-partition | Per-stream |
| Backpressure | Consumer lag, automatic | `MAXLEN` trimming, no native backpressure |
| Operational | Requires broker cluster | Requires Redis/Valkey instance |
| Use case | Production pipelines | Dev/test, low-volume, cache-adjacent workloads |
| DFE support | Full (producer, consumer, stats) | Full (XADD, XREADGROUP, XACK) |

Use Kafka for production data pipelines. Use Redis Streams for development,
edge deployments, or where Redis is already in the stack and volume is modest.

## Auto-Emitted Metrics

All transports automatically emit Prometheus metrics when a `MetricsManager`
recorder is installed:

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `dfe_transport_sent_total` | counter | `transport` | Messages sent |
| `dfe_transport_send_errors_total` | counter | `transport` | Send failures |
| `dfe_transport_backpressured_total` | counter | `transport` | Backpressure events |
| `dfe_transport_refused_total` | counter | `transport` | Refused (closed/unhealthy) |
| `dfe_transport_received_total` | counter | `transport` | Messages received |
| `dfe_transport_healthy` | gauge | `transport` | 1 = healthy, 0 = unhealthy |
| `dfe_transport_queue_size` | gauge | `transport` | Current queue depth |
| `dfe_transport_queue_capacity` | gauge | `transport` | Queue capacity |
| `dfe_transport_inflight` | gauge | `transport` | In-flight messages |
| `dfe_transport_send_duration_seconds` | histogram | `transport` | Send latency |

The Kafka transport also emits `rdkafka_*` metrics from librdkafka's
internal stats when `statistics.interval.ms` is configured.

## Health Registry Integration

All transports register with the global `HealthRegistry` when the `health`
feature is enabled. Registration names follow the pattern `transport:{name}`:

- `transport:kafka`
- `transport:grpc`
- `transport:file`
- `transport:pipe`
- `transport:http`
- `transport:redis`

The Memory transport does not register (test infrastructure).

Health status is derived from the transport's `closed` state — a closed
transport reports `Unhealthy`. The `/readyz` endpoint aggregates all
registered component health checks.
