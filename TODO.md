# TODO - hyperi-rustlib

**Project Goal:** Rust shared library equivalent to hyperi-pylib (Python) and hyperi-golib (Go)

**Target:** Production-ready library for HyperI Rust applications

---

## Current Tasks

### High Priority

- [x] Add integration tests for metrics HTTP server - 2025-01-19
- [x] Implement parity tests against hyperi-pylib (config cascade behavior) - 2025-01-19
- [x] Gap analysis hyperi-pylib - 2025-01-19 (see docs/GAP_ANALYSIS.md)
- [x] Add example application demonstrating all features - 2025-01-19
- [x] Python bindings discussion for clickhouse - 2025-01-19 (see docs/CLICKHOUSE_PYTHON_BINDINGS.md)

### Medium Priority

- [x] Add more comprehensive config cascade tests (YAML file loading) - 2025-01-19
- [ ] Implement log output capturing for logger tests
- [ ] Add metrics server graceful shutdown tests

### Low Priority

- [ ] Benchmark config loading performance
- [ ] Add colored log output for text format
- [ ] Document environment variable naming conventions

---

## Completed

- [x] Project setup with feature flags - 2025-12-24
- [x] Environment detection module (K8s/Docker/Container/BareMetal) - 2025-12-24
- [x] Runtime paths module (XDG + container awareness) - 2025-12-24
- [x] Configuration module (7-layer cascade with figment) - 2025-12-24
- [x] Logger module (structured JSON, RFC3339, masking) - 2025-12-24
- [x] Metrics module (Prometheus + process/container) - 2025-12-24
- [x] All 36 unit tests passing - 2025-12-24
- [x] Clippy passing with pedantic warnings - 2025-12-24
- [x] Initial commit and push to GitHub - 2025-12-24
- [x] Transport module (Kafka/Zenoh/Memory abstraction) - 2025-01-XX
- [x] ClickHouse client (Arrow protocol) - 2025-01-XX
- [x] Integration tests for metrics server (14 tests) - 2025-01-19
- [x] Parity tests for config cascade (19 tests) - 2025-01-19
- [x] Parity tests for env detection (4 tests) - 2025-01-19
- [x] Gap analysis vs hyperi-pylib documented - 2025-01-19
- [x] Example applications (quickstart, full_demo) - 2025-01-19
- [x] Python bindings proposal documented - 2025-01-19
- [x] HTTP server module (axum-based) - 2025-01-19
- [x] Spool module (disk-backed queue) - 2025-01-19
- [x] TieredSink module (disk spillover with circuit breaker) - 2025-01-19
- [x] Dependency audit and migration to safer/maintained alternatives - 2025-01-19
  - Replaced `serde_yml` with `serde-yaml-ng` (security fix)
  - Migrated `queue-file` to `yaque` (async-native, maintained)
  - Replaced `once_cell` with `std::sync::LazyLock` (MSRV 1.80)
  - Added OpenTelemetry support (`otel`, `otel-metrics`, `otel-tracing` features)
  - Added `tower-resilience` for circuit breaker patterns

---

## Blocked

None currently.

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
- [ ] PII anonymizer (evaluate Rust libraries)
- [ ] Python bindings for ClickHouse client (PyO3)

---

## Notes

- Use `CARGO_BUILD_JOBS=2` for all cargo commands
- Feature flags: `config`, `logger`, `metrics`, `runtime`, `env`, `transport`, `clickhouse-arrow`, `http-server`, `spool`, `tiered-sink`
- MVP complete - iterate based on usage feedback
- See docs/GAP_ANALYSIS.md for detailed comparison with hyperi-pylib
- See docs/CLICKHOUSE_PYTHON_BINDINGS.md for Python binding proposal

---

**Last Updated:** 2025-01-19
