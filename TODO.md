# TODO - hyperi-rustlib

**Project Goal:** Rust shared library equivalent to hyperi-pylib (Python) and hyperi-golib (Go)

**Target:** Production-ready library for HyperI Rust applications

---

## Current Tasks

### Local File Output Sink

**Goal:** Extract shared NDJSON writer from DLQ, add file output sink module

1. [x] Create `src/io/` shared module (NdjsonWriter, FileWriterConfig, RotationPeriod)
2. [x] Refactor `dlq/file.rs` to use NdjsonWriter (no regressions)
3. [x] Create `src/output/` module (FileOutput, FileOutputConfig)
4. [x] Add `output-file` feature flag to Cargo.toml
5. [x] Tests + clippy clean for all feature combinations

### TUI Metrics Dashboard (like `vector top`)

**Goal:** Local real-time metrics viewer for DFE services — equivalent to `vector top`

Challenges:
- Metrics are nested/treed (e.g. loader: data in from multiple topics, multiple table buffer buckets)
- Need to consume existing Prometheus metrics endpoint (`/metrics`)
- Should work for all rustlib-based services by default (loader, receiver, etc.)
- Consider ratatui + crossterm stack (see STATE.md library research)

1. [ ] Spike: prototype ratatui dashboard consuming `/metrics` endpoint
2. [ ] Tree/nested view for per-topic and per-table-buffer metrics
3. [ ] Sparkline widgets for throughput rates
4. [ ] Feature-gated module in rustlib (`tui` or `top` feature)

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

---

## Notes

- Use `CARGO_BUILD_JOBS=2` for all cargo commands
- Transport backends: Kafka, gRPC (native + Vector compat), Memory (Zenoh removed in v1.8.0)
- See docs/GAP_ANALYSIS.md for detailed comparison with hyperi-pylib
- See docs/CLICKHOUSE_PYTHON_BINDINGS.md for Python binding proposal
