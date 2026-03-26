# TODO - hyperi-rustlib

**Project Goal:** Opinionated Rust framework for high-throughput data pipelines at PB scale

**Target:** Production-ready library with auto-wiring config, logging, metrics, tracing, health, and graceful shutdown

---

## Current Tasks

### v2.0.0 Release `[NEXT]`

All core pillar work is done. Need to:
- [ ] Release-merge to release branch (feat!: breaking change → v2.0.0)
- [ ] Verify crates.io publication
- [ ] Docs consolidation (TRANSPORT.md, CORE-PILLARS.md, per-feature docs)
- [ ] Add Redis vs Kafka comparison table to transport docs

### DLQ Transport Integration

- [ ] DLQ Kafka backend uses `Box<dyn TransportSender>` / `AnySender` instead of raw producer
- [ ] DLQ can write to any transport (file, HTTP, Redis, Kafka)

### Identity / Auth Module (Discussion)

- [ ] Token validation middleware (JWT/OIDC) for gRPC interceptor + axum middleware
- [ ] Service identity (service name + instance ID for mTLS, audit logs)
- [ ] Break-glass: static bearer token from secrets module
- [ ] Design decision: dfe-engine as SSoT, rustlib validates tokens only

### Downstream Remediation

- [ ] Migrate dfe-loader to v2.0.0 (transport factory, remove boilerplate)
- [ ] Migrate dfe-receiver to v2.0.0 (RoutedSender, transport factory)
- [ ] Migrate dfe-archiver to v2.0.0
- [ ] Migrate dfe-fetcher to v2.0.0
- [ ] Migrate dfe-transform-wasm to v2.0.0
- [ ] Migrate dfe-transform-vrl to v2.0.0
- [ ] Audit hyperi-pylib and write alignment plan

---

### Completed This Session

- [x] **Transport trait split** — `Transport` split into `TransportBase` + `TransportSender` + `TransportReceiver` with blanket `Transport` impl
- [x] **Transport factory** — `AnySender` enum dispatch from config, `RoutedSender` for per-key dispatch (receiver/fetcher only)
- [x] **File transport** — NDJSON with position tracking, commit persistence, rotation
- [x] **Pipe transport** — stdin/stdout for Unix pipeline composition
- [x] **HTTP transport** — POST send + embedded axum receive (bidirectional)
- [x] **Redis/Valkey Streams transport** — XADD/XREADGROUP/XACK with consumer groups
- [x] **HealthRegistry** — global singleton, modules auto-register health check closures, `/readyz` aggregates, `/health/detailed` JSON
- [x] **Shutdown manager** — global CancellationToken, SIGTERM/SIGINT handler, modules listen on token
- [x] **OTel trace propagation** — W3C traceparent auto-injected/extracted in gRPC, Kafka, HTTP transports
- [x] **Universal metrics** — all modules auto-emit Prometheus metrics via global recorder
- [x] **Logging + config cascade** — added to all new transports
- [x] **Health wiring** — Kafka, gRPC, CircuitBreaker, HttpServer auto-register
- [x] **Shutdown wiring** — HttpServer, TieredSink drainer, ConfigReloader listen on global token
- [x] **KafkaTransport StatsContext** — always-on, rdkafka_* metrics auto-emitted
- [x] **gRPC transport metrics** — dfe_transport_* parity with Kafka
- [x] **HTTP client, database URL builders, cache modules** (v1.19.6)
- [x] **Config registry, SensitiveString, /config endpoint** (v1.19.3-v1.19.5)
- [x] **Dependency updates, cargo-audit ignores** (v1.19.6)

---

## Backlog

### Secrets Providers

- [ ] GCP Secret Manager provider (`secrets-gcp` feature)
- [ ] Azure Key Vault provider (`secrets-azure` feature)

### Kafka — Opinionated SASL-SCRAM Named Constructors

- [ ] `KafkaConfig::external_sasl_scram(brokers, username, password)`
- [ ] `KafkaConfig::internal_sasl_scram(brokers, username, password)`

### Other

- [ ] PII anonymiser (evaluate Rust libraries)
- [ ] Python bindings for ClickHouse client (PyO3)

---

## Notes

- Use `CARGO_BUILD_JOBS=2` for all cargo commands
- Transport backends: Kafka, gRPC, Memory, File, Pipe, HTTP, Redis/Valkey
- Core pillars plan: `docs/superpowers/plans/2026-03-26-core-pillars.md`
- Two deployment modes: Kafka-mediated (persistence) vs direct gRPC (low latency)
- Routed transport is receiver/fetcher only — all other stages are 1:1
- Breaking change: `feat!:` commit triggers v2.0.0 via semantic-release
