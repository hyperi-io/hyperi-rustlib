# TODO - hyperi-rustlib

**Project Goal:** Rust shared library equivalent to hyperi-pylib (Python) and hyperi-golib (Go)

**Target:** Production-ready library for HyperI Rust applications

---

## Current Tasks

### Core Pillars Implementation `[NEXT]`

Full plan: `docs/superpowers/plans/2026-03-26-core-pillars.md`

**Phase 1: OTel Tracing Auto-Propagation**
- [ ] Auto-initialise OTel layer in logger when `otel` feature + `OTEL_EXPORTER_OTLP_ENDPOINT` set
- [ ] gRPC trace context propagation (tonic interceptors, `traceparent` header)
- [ ] Kafka trace context propagation (message headers)
- [ ] HTTP client trace context injection
- [ ] HTTP server trace context extraction

**Phase 2: Unified HealthState**
- [ ] `src/health/` module with global `HealthRegistry` singleton
- [ ] `HealthComponent` trait — modules register at construction
- [ ] Wire transport, circuit breaker, config reloader into registry
- [ ] `/readyz` aggregates from `HealthRegistry::is_healthy()`
- [ ] `/health/detailed` JSON endpoint with per-component status

**Phase 3: Unified Graceful Shutdown**
- [ ] `src/shutdown/` module with global `CancellationToken`
- [ ] SIGTERM/SIGINT → `token.cancel()` → all modules drain
- [ ] Wire http-server, tiered-sink, config-reloader, gRPC transport

**Phase 4: New Transports**
- [ ] File transport (NDJSON, wraps existing `NdjsonWriter`)
- [ ] Pipe transport (stdin/stdout, newline-delimited)
- [ ] HTTP transport (POST to endpoint, uses `HttpClient`)
- [ ] Redis/Valkey Streams transport (`XADD`/`XREADGROUP`/`XACK`)

**Phase 5: DLQ Transport Integration**
- [ ] DLQ Kafka backend uses `Box<dyn Transport>` instead of raw producer
- [ ] DLQ can write to any transport (file, HTTP, Redis, Kafka)

**Phase 6: Always-On Defaults**
- [ ] Make config, logger, metrics, health, shutdown default features
- [ ] Downstream dfe-* app remediation (remove boilerplate)
- [ ] Audit hyperi-pylib and write alignment plan

---

### Completed Recent

- [x] **Universal metrics instrumentation** (v1.19.8) — tiered-sink, spool, dlq, cache, http-client, secrets all auto-emit Prometheus metrics via global singleton. Core pillar design decision documented in CLAUDE.md.
- [x] **Kafka transport metrics + StatsContext** (v1.19.8) — `KafkaTransport` always uses `StatsContext` for consumer and producer. `dfe_transport_*` metrics on `send()`. `rdkafka_*` metrics auto-emitted. Zero downstream code changes.
- [x] **gRPC transport metrics** (v1.19.7) — `dfe_transport_*` metrics on send/recv. Server push handler uses `try_send` with backpressure status codes.
- [x] **HTTP client module** (v1.19.6) — reqwest + reqwest-middleware + reqwest-retry, exponential backoff, config cascade
- [x] **Database URL builders** (v1.19.6) — PostgreSQL, ClickHouse, Redis/Valkey, MongoDB. Display trait redacts passwords.
- [x] **Cache module** (v1.19.6) — moka-backed concurrent in-memory cache, per-source TTL, source isolation
- [x] **Dependency update** (v1.19.6) — all deps to latest, cargo-audit ignores for transitive advisories
- [x] **Config registry** (v1.19.3-v1.19.5) — auto-registering reflectable config, `/config` admin endpoint, `SensitiveString`, heuristic redaction, change notification
- [x] **CEL expression profile** (v1.19.2) — `matches()` blocked by default, `ProfileConfig` with per-category overrides
- [x] **Config cascade wiring** (v1.19.2) — expression, memory, version_check, scaling, grpc, secrets auto-read from cascade
- [x] **MemoryGuard underflow fix** (v1.19.1) — `fetch_sub` replaced with `fetch_update` + `saturating_sub`
- [x] **Test restructure** (v1.19.1) — `tests/integration/`, `tests/e2e/`, `tests/common/`
- [x] **hyperi-ci release-merge** — CLI command replaces per-project workflow files

---

## Backlog

### Secrets Providers

- [ ] GCP Secret Manager provider (`secrets-gcp` feature)
- [ ] Azure Key Vault provider (`secrets-azure` feature)

### Kafka — Opinionated SASL-SCRAM Named Constructors

- [ ] `KafkaConfig::external_sasl_scram(brokers, username, password)` — SASL_SSL + SCRAM-SHA-512
- [ ] `KafkaConfig::internal_sasl_scram(brokers, username, password)` — SASL_PLAINTEXT + SCRAM-SHA-512

### Other

- [ ] PII anonymiser (evaluate Rust libraries)
- [ ] Python bindings for ClickHouse client (PyO3)

---

## Notes

- Use `CARGO_BUILD_JOBS=2` for all cargo commands
- Transport backends: Kafka, gRPC (native + Vector compat), Memory
- Core pillars plan: `docs/superpowers/plans/2026-03-26-core-pillars.md`
- See docs/GAP_ANALYSIS.md for detailed comparison with hyperi-pylib
