# TODO - hyperi-rustlib

**Project Goal:** Opinionated Rust framework for high-throughput data pipelines at PB scale

**Target:** Production-ready library with auto-wiring config, logging, metrics, tracing, health, and graceful shutdown

---

## Current Tasks

### Step 1: Finish Parallelism Wiring (all DFE Rust apps)

Rustlib common patterns (done):
- [x] `BatchAccumulator<T>` — bounded channel + drain-on-threshold (9 tests)
- [x] NDJSON split utilities — `split_lines()`, `count_lines()` (11 tests)

Actually parallelised (process_batch wired + tested):
- [x] dfe-loader — parallel parse+route+CEL+enrich (3 parallel tests)
- [x] dfe-transform-vrl — parallel deser + VRL eval (2 parallel tests)

NOT yet parallelised (architecture prepared but process_batch NOT wired):
- [ ] dfe-archiver — wire process_batch for parallel compression of staged batches
- [ ] dfe-receiver — wire BatchAccumulator for request batching + NDJSON split + parallel validate+route
- [ ] dfe-receiver — Splunk HEC / OTLP / gRPC parallel per-event processing via fan_out_async
- [ ] dfe-fetcher — within-source concurrent service fetching (needs Source trait + Arc<Self> or futures crate)

Mutex audit:
- [ ] Document hot-path Mutex patterns to avoid (in standards)
- [ ] dfe-archiver transport Mutex — evaluate

### Step 2: A/B Benchmark — Columnar vs Row Batch Layout

- [ ] Create `benches/batch_layout_benchmark.rs` in rustlib
- [ ] Implement both layouts: row-based (current: Vec<Map<String,Value>>) and columnar (SoA: Vec<Field> per column)
- [ ] Benchmark with realistic workloads:
  - 10K event Kafka batch (dfe-loader/transform-vrl hot path)
  - HTTP stream batches (dfe-receiver accumulator pattern)
  - 1K, 10K, 100K event sizes
- [ ] Test transforms: field extraction, CEL evaluation, VRL evaluation, routing
- [ ] Keep BOTH implementations regardless of winner
- [ ] NOT necessarily Arrow — simple Vec<Field> columns first

### Step 3: Pick and Document Winner from A/B Testing

- [ ] Document benchmark results with numbers
- [ ] Decision: adopt columnar if >20% improvement, keep row if marginal
- [ ] If columnar wins: implement the common batch type in rustlib (`worker` module)
- [ ] If row wins: document why and close the investigation
- [ ] Update standards/rules/rust.md with the decision and rationale

### Step 4: Remediate ALL dfe- Rust App Projects

Full remediation per project (using the winning batch layout from Step 3):
- [ ] dfe-loader — update to final batch layout, deployment contract artefacts, adversarial tests
- [ ] dfe-archiver — parallel compression, deployment contract, adversarial tests
- [ ] dfe-receiver — BatchAccumulator integration, deployment contract, adversarial tests
- [ ] dfe-transform-vrl — update to final batch layout, deployment contract, adversarial tests
- [ ] dfe-fetcher — concurrent service fetching, deployment contract, adversarial tests
- [ ] dfe-transform-vector — deployment contract artefacts (no parallelism changes)
- [ ] Per-project: throughput benchmark (sequential vs parallel comparison)

### Phase 3: Non-Integrated Transforms (after Step 4)

- [ ] dfe-transform-wasm — parallel WASM invocation per batch
- [ ] dfe-transform-elastic — assess rustlib integration level, discuss
- [ ] dfe-transform-splunk — assess rustlib integration level, discuss

### Deferred

- [ ] Metrics manifest enrichment — `set_use_cases()` / `set_dashboard_hint()` content
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
