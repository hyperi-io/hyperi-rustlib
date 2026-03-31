# TODO - hyperi-rustlib

**Project Goal:** Opinionated Rust framework for high-throughput data pipelines at PB scale

**Target:** Production-ready library with auto-wiring config, logging, metrics, tracing, health, and graceful shutdown

---

## Current Tasks

### v1.20.0 Release

Release branch and v1.20.0 tag exist. Core pillar work done.
- [x] Release-merge to release branch
- [ ] Verify crates.io publication succeeded
- [x] Docs consolidation (TRANSPORT.md, CORE-PILLARS.md)
- [x] Redis vs Kafka comparison table in TRANSPORT.md

### Metrics Manifest (v1.22)

- [x] **Metrics manifest infrastructure** — `MetricDescriptor`, `MetricRegistry`, `ManifestResponse` types
  - Standards-aligned: OpenMetrics (type/description/unit), OTel Advisory (labels/buckets)
  - Novel HyperI extensions: `group`, `use_cases`, `dashboard_hint`
  - `MetricRegistry` (Arc<RwLock>) tightly coupled into `MetricsManager`
  - Every `counter()`/`gauge()`/`histogram()` call auto-pushes descriptor
  - New `_with_labels()` methods for declaring label keys and groups
  - `set_build_info()`, `set_use_cases()`, `set_dashboard_hint()` enrichment
- [x] **`/metrics/manifest` endpoint** — JSON contract on both axum and raw server paths
  - Correct path ordering (manifest checked before /metrics in raw server)
  - Explicit Content-Type: application/json
- [x] **`DfeMetrics::register(&MetricsManager)` breaking change** — platform metrics tightly coupled
  - All 24 dfe_* metrics auto-appear in manifest with correct labels and group="platform"
- [x] **dfe_groups updated** — all 8 groups use `_with_labels()` internally
  - `AppMetrics::new()` calls `set_build_info()` automatically
  - Label-based metrics push descriptors with correct key names
  - Histogram buckets captured in manifest
- [x] **Downstream dfe-* projects updated** (committed, not pushed — waiting for parallelism remediation)
  - dfe-loader, dfe-receiver, dfe-archiver, dfe-fetcher, dfe-transform-vector, dfe-transform-vrl
  - Phase 3 (dfe-transform-wasm, dfe-transform-elastic, dfe-transform-splack) deferred
- [ ] **Phase 2 enrichment** — `set_use_cases()` / `set_dashboard_hint()` content (deferred to parallelism remediation)
- [ ] **Regenerate container + metrics contract artefacts** per project (deferred to push time)

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

- Use `CARGO_BUILD_JOBS=2` for all cargo commands
- Transport backends: Kafka, gRPC, Memory, File, Pipe, HTTP, Redis/Valkey
- Core pillars plan: `docs/superpowers/plans/2026-03-26-core-pillars.md`
- Two deployment modes: Kafka-mediated (persistence) vs direct gRPC (low latency)
- Routed transport is receiver/fetcher only — all other stages are 1:1
- v1.20.0 released with breaking transport trait split (feat!: commit)
