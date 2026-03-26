# TODO - hyperi-rustlib

**Project Goal:** Rust shared library equivalent to hyperi-pylib (Python) and hyperi-golib (Go)

**Target:** Production-ready library for HyperI Rust applications

---

## Current Tasks

### Kafka Transport Metrics Parity with gRPC `[NEXT]`

gRPC transport (v1.19.7) auto-emits `dfe_transport_*` metrics. Kafka transport does not — two gaps:

1. **Add metrics instrumentation to `KafkaTransport::send()`** — emit `dfe_transport_sent_total{transport="kafka"}`, `dfe_transport_send_errors_total{transport="kafka"}`, `dfe_transport_backpressured_total{transport="kafka"}`, and send duration histogram, matching gRPC parity.

2. **Wire `StatsContext` into `KafkaTransport`** — currently `new_with_context()` is a stub that ignores the context. The struct uses `DefaultConsumerContext` / plain `FutureProducer`, so `statistics.interval.ms` callbacks (set to 1000ms by all profiles) go to a no-op. Need to either make the struct generic over context (complicates `Transport` trait impl) or use `StatsContext` by default when the `metrics` feature is enabled. This enables `rdkafka_broker_rtt_avg_seconds`, `rdkafka_global_msg_cnt`, `rdkafka_topic_partition_consumer_lag`, etc.

Downstream impact: dfe-fetcher, dfe-receiver, dfe-loader all use `KafkaTransport` and would get these for free once wired.

### Gap Analysis P2 — HTTP Client, Database URLs, Cache

- [ ] HTTP client module with retry middleware (reqwest + reqwest-middleware + reqwest-retry)
  - Wrap reqwest with exponential backoff, configurable timeouts
  - Auto-register config via `unmarshal_key_registered`
  - Metrics integration (request count, duration, errors)
- [ ] Database URL builders (PostgreSQL, ClickHouse, Redis)
  - Build connection strings from env vars with standard prefixes
  - `SensitiveString` for password fields
- [ ] Cache module with disk/memory backends
  - Consolidate secrets cache pattern into reusable module
  - TTL, stale-while-revalidate, size bounds

### Completed Recent

- [x] **Config registry** (v1.19.3-v1.19.5) — auto-registering reflectable config, `/config` admin endpoint, `SensitiveString`, heuristic redaction, change notification, `ConfigReloader` hook
- [x] **CEL expression profile** (v1.19.2) — `matches()` blocked by default, `ProfileConfig` with per-category overrides, string literal false-positive prevention
- [x] **Config cascade wiring** (v1.19.2) — expression, memory, version_check, scaling, grpc, secrets auto-read from figment cascade
- [x] **MemoryGuard underflow fix** (v1.19.1) — `fetch_sub` replaced with `fetch_update` + `saturating_sub`
- [x] **Test restructure** (v1.19.1) — `tests/integration/`, `tests/e2e/`, `tests/common/` per testing standard
- [x] **hyperi-ci release-merge** — CLI command replaces per-project workflow files
- [x] **Rust edition 2024** — migrated from 2021; `temp-env` replaces unsafe `set_var`/`remove_var` in tests across 6 files
- [x] **async-trait removal** — public traits (`Sink`, `Transport`, `SecretProvider`) now use `fn ... -> impl Future + Send` (Rust 1.75+ native)
- [x] **kafka_config module** — `config_from_file`, 7 named profiles, `merge_with_overrides`; librdkafka settings loaded from config git dir (only cascade exception)
- [x] **File output sink** — `src/io/`, `src/output/`, `output-file` feature
- [x] **CLI module** — CommonArgs, StandardCommand, DfeApp trait (`cli` feature)
- [x] **Top module** — ratatui TUI dashboard, Prometheus parser, oneshot mode (`top` feature)
- [x] **CI gating fix** — Semantic Release now gated on CI success via workflow_run

---

## Completed

- [x] Vector compat integration tests — 6 tests using real Vector binary + VectorCompatClient (fetch-vector.sh + YAML)
- [x] vault_env integration tests fixed — clear_vault_env() prevents VAULT_TOKEN leakage
- [x] Dependency update sweep — all crates to latest, tonic/prost 0.14 migration (v1.8.4)
- [x] Stale hs-rustlib removed from JFrog hypersec-cargo-local and hyperi-cargo-local
- [x] MaskingLayer fixed — writer-based redaction for both JSON and text formats (v1.8.4)
- [x] Logger output capturing tests — 10 tests (JSON, text, filtering, masking)
- [x] Coloured log output — custom FormatEvent with owo-colors colour scheme
- [x] Metrics graceful shutdown tests — 4 tests (shutdown, rapid cycle, render after stop, concurrent)
- [x] gRPC transport integration tests — 8 tests (send/recv, ordering, large payload, compression)
- [x] gRPC transport with Vector wire protocol compatibility (v1.8.0)
  - tonic-based gRPC replacing Zenoh transport
  - DFE native proto (`dfe.transport.v1`) + vendored Vector proto
  - Vector compat source/sink for migration from Vector pipelines
  - build.rs for conditional proto code generation
- [x] Zenoh transport removed — replaced by gRPC (v1.8.0)
- [x] Version check module — startup check against releases.hyperi.io (v1.7.0)
- [x] Deployment validation module — Helm chart and Dockerfile contract checks (v1.7.0)
- [x] CI: ARC self-hosted runners enabled (v1.7.1–v1.8.3)
- [x] Clippy/formatting fixes — approx_constant lint, dprint float formatting (v1.8.1–v1.8.3)
- [x] Package rename: hs-rustlib -> hyperi-rustlib, published v1.4.3 to JFrog
- [x] Rebrand: HyperSec -> HyperI across source, docs, configs, workflows
- [x] Registry migration: hypersec registry -> hyperi registry
- [x] Submodule URLs: hypersec-io -> hyperi-io
- [x] CI config: .hypersec-ci.yaml -> .hyperi-ci.yaml
- [x] Directory-config store with git2 integration (v1.4.0)
- [x] OpenTelemetry metrics support (v1.4.0)
- [x] Secrets management module (OpenBao/Vault, AWS) (v1.3.x)
- [x] HTTP server module (axum-based) (v1.2.0)
- [x] Transport module (Kafka/Memory abstraction)
- [x] TieredSink module (disk spillover with circuit breaker)
- [x] Spool module (disk-backed queue)
- [x] Configuration module (7-layer cascade with figment)
- [x] Logger module (structured JSON, RFC3339, masking)
- [x] Metrics module (Prometheus + process/container)
- [x] Environment detection module
- [x] Runtime paths module (XDG + container awareness)
- [x] Dependency audit (serde_yml -> serde-yaml-ng, queue-file -> yaque, once_cell -> LazyLock)
- [x] Config cascade alignment with hyperi-pylib unified spec (v1.6.0)
  - load_home_dotenv default false, app_name support, container/user config paths
  - Created docs/CONFIG-CASCADE.md
  - PG layer documented as built-for-not-with (YAML gitops covers current needs)

---

## Backlog (P1 - Config Registry)

### Reflectable Config Registry

Central registry where every module registers its config section at startup.
Currently modules independently call `unmarshal_key()` — no visibility into
what config keys exist, their types, defaults, or descriptions.

**Goal:** Any DFE app can list/dump/expose all available config sections.

- [x] Auto-registration via `unmarshal_key_registered` — records `(key, type_name, defaults, effective)` in global registry. Zero code changes in downstream apps.
- [x] `registry::sections()` — list all registered sections
- [x] `registry::dump_effective()` — JSON map of effective values
- [x] `registry::dump_defaults()` — JSON map of defaults (via `T::default()`)
- [x] Heuristic auto-redaction (password, secret, token, key, credential, auth, private, cert, encryption)
- [x] `#[serde(skip_serializing)]` as additional layer for fields that should never appear
- [x] expression, memory, version_check, scaling, grpc, secrets wired with `from_cascade()` auto-register
- [x] Modules without defaults (tiered_sink, http_server, kafka, spool, dlq) use `unmarshal_key_registered` from downstream apps
- [x] `/config` admin endpoint (opt-in via `enable_config_endpoint`) — returns redacted effective + defaults JSON
- [x] Change notification (opt-in) — `registry::on_change(key, callback)` + `registry::update()`
  - Modules that need hot-reload subscribe; others keep `OnceLock` (init-once)
- [x] `ConfigReloader.with_registry_update(key)` connects hot-reload to registry
- [x] `SensitiveString` type — compile-time safe, `Serialize` always redacts
- [x] 19 registry + 12 sensitive string tests covering all redaction guarantees
- [ ] Migrate all dfe-* and hyperi-* apps to `unmarshal_key_registered` pattern
- [ ] Align hyperi-pylib with same registry pattern

---

## Backlog (P2 - from Gap Analysis)

### Phase 1 - Core Enterprise

- [ ] Database URL builders module (PostgreSQL, Redis)
- [ ] HTTP client module with retry middleware (reqwest-retry)

### Secrets Providers

- [ ] GCP Secret Manager provider (`secrets-gcp` feature, `google-cloud-secretmanager` crate)
- [ ] Azure Key Vault provider (`secrets-azure` feature, `azure_security_keyvault` crate)

### Phase 2 - Enhanced Features

- [ ] Cache module with disk/Redis backing
- [ ] CLI framework helpers (wrap Clap)

### Phase 3 - Advanced

- [ ] Standalone Kafka client (if transport layer insufficient)
- [ ] PII anonymiser (evaluate Rust libraries)
- [ ] Python bindings for ClickHouse client (PyO3)

### Kafka — Opinionated SASL-SCRAM Named Constructors

- [ ] Add `KafkaConfig::external_sasl_scram(brokers, username, password)` — SASL_SSL + SCRAM-SHA-512
- [ ] Add `KafkaConfig::internal_sasl_scram(brokers, username, password)` — SASL_PLAINTEXT + SCRAM-SHA-512
- [ ] Encodes the decision once: SCRAM works unchanged on Apache Kafka, AutoMQ, MSK, Confluent Cloud
- [ ] Remove per-project manual assembly of protocol + sasl + tls fields in dfe-loader, dfe-receiver

---

## Notes

- Use `CARGO_BUILD_JOBS=2` for all cargo commands
- Transport backends: Kafka, gRPC (native + Vector compat), Memory (Zenoh removed in v1.8.0)
- See docs/GAP_ANALYSIS.md for detailed comparison with hyperi-pylib
- See docs/CLICKHOUSE_PYTHON_BINDINGS.md for Python binding proposal
