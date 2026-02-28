# TODO - hyperi-rustlib

**Project Goal:** Rust shared library equivalent to hyperi-pylib (Python) and hyperi-golib (Go)

**Target:** Production-ready library for HyperI Rust applications

---

## Current Tasks

### High Priority

- [x] Rename package from hs-rustlib to hyperi-rustlib (v1.4.3 published)
- [x] Update all downstream consumers (dfe-loader, dfe-archiver, dfe-receiver)
- [x] Complete HyperSec -> HyperI rebrand across all references
- [ ] Remove stale `hs-rustlib` crate from JFrog `hypersec-cargo-local` registry

### Medium Priority

- [ ] Implement log output capturing for logger tests
- [ ] Add metrics server graceful shutdown tests

### Low Priority

- [ ] Benchmark config loading performance
- [ ] Add coloured log output for text format

---

## Completed

- [x] Package rename: hs-rustlib -> hyperi-rustlib, published v1.4.3 to JFrog
- [x] Rebrand: HyperSec -> HyperI across source, docs, configs, workflows
- [x] Registry migration: hypersec registry -> hyperi registry
- [x] Submodule URLs: hypersec-io -> hyperi-io
- [x] CI config: .hypersec-ci.yaml -> .hyperi-ci.yaml
- [x] Directory-config store with git2 integration (v1.4.0)
- [x] OpenTelemetry metrics support (v1.4.0)
- [x] Secrets management module (OpenBao/Vault, AWS) (v1.3.x)
- [x] HTTP server module (axum-based) (v1.2.0)
- [x] Transport module (Kafka/Zenoh/Memory abstraction)
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
- See docs/GAP_ANALYSIS.md for detailed comparison with hyperi-pylib
- See docs/CLICKHOUSE_PYTHON_BINDINGS.md for Python binding proposal
