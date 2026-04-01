# TODO - hyperi-rustlib

**Project Goal:** Opinionated Rust framework for high-throughput data pipelines at PB scale

**Target:** Production-ready library with auto-wiring config, logging, metrics, tracing, health, and graceful shutdown

---

## Current Tasks

### Phase 2: Parallelism + Batching (all DFE projects)

Common patterns needed in rustlib first:
- [ ] `BatchAccumulator<T>` — bounded channel + drain-on-threshold for receiver batching
- [ ] SIMD-optimised batch processing — sonic-rs batch parse, memchr for NDJSON splitting
- [ ] Evaluate columnar batch layout — if SoA (struct-of-arrays) layout improves cache locality for batch transforms, adopt it. NOT necessarily Arrow — could be simple Vec<Field> columns. Profile before deciding.
- [ ] Mutex audit enforcement — document/lint pattern for hot-path Mutex detection

Per-project parallelism work:
- [ ] dfe-archiver — per-destination writers (remove single Mutex), parallel compression
- [ ] dfe-receiver — request batching (accumulate N bodies → batch validate+route → batch produce)
- [ ] dfe-receiver — Splunk HEC / OTLP / gRPC parallel per-event processing via fan_out_async
- [ ] dfe-fetcher — within-source parallel service fetching via fan_out_async
- [ ] dfe-transform-vrl — parallel deserialisation (Phase 1 of pipeline, currently sequential)
- [ ] Per-project: adversarial parallel tests, throughput benchmarks

Phase 2 spec: `docs/superpowers/specs/2026-04-01-phase2-parallelism-batching.md`

### Phase 3: Non-Integrated Transforms

- [ ] dfe-transform-wasm — parallel WASM invocation per batch
- [ ] dfe-transform-elastic — assess rustlib integration level, discuss
- [ ] dfe-transform-splunk — assess rustlib integration level, discuss

### Metrics Manifest Enrichment (deferred)

- [ ] `set_use_cases()` / `set_dashboard_hint()` content per metric group
- [ ] Regenerate container + metrics contract artefacts per project

### Completed Previous Sessions

- [x] **Code review remediation** — security fixes, bug fixes, panic removal, health wiring, cargo housekeeping, API polish
  - SensitiveString moved to crate root (always available, no feature gate)
  - KafkaConfig.sasl_password, CacheConfig.encryption_key → SensitiveString
  - SecretValue Debug redaction, secrets cache dir permissions (0o700)
  - recv() metric names fixed (file, pipe, redis), describe_gauge AppMetrics fix
  - expect() removed from shutdown, metrics, http_client
  - File, pipe, http, redis transports registered with HealthRegistry
  - deny.toml added, validate_table_name blocks single-dot
  - histogram_with_buckets limitation documented
- [x] **DLQ HTTP + Redis backends** — `dlq-http` and `dlq-redis` features
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

### Identity / Auth Module (Discussion)

- [ ] Token validation middleware (JWT/OIDC) for gRPC interceptor + axum middleware
- [ ] Service identity (service name + instance ID for mTLS, audit logs)
- [ ] Break-glass: static bearer token from secrets module
- [ ] Design decision: dfe-engine as SSoT, rustlib validates tokens only

### Other

- [ ] PII anonymiser (evaluate Rust libraries)
- [ ] Python bindings for ClickHouse client (PyO3)

---

## Notes

- Transport backends: Kafka, gRPC, Memory, File, Pipe, HTTP, Redis/Valkey
- Core pillars plan: `docs/superpowers/plans/2026-03-26-core-pillars.md`
- Two deployment modes: Kafka-mediated (persistence) vs direct gRPC (low latency)
- Routed transport is receiver/fetcher only — all other stages are 1:1
- Common patterns in rustlib first — if all 6 DFE projects use the same pattern, implement in rustlib
- DFE parallelisation playbook: `/projects/dfe-loader/docs/PARALLELISE-REMEDIATION.md`
- Phase 2 spec: `docs/superpowers/specs/2026-04-01-phase2-parallelism-batching.md`
