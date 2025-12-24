# Work Breakdown Structure - hs-rustlib

## Project Overview

**Objective:** Create a Rust equivalent of hs-lib (Python) and hs-golib (Go) shared libraries.

**Parity Testing:** Each component requires parity tests comparing behaviour against the original Go/Python implementations.

---

## Phase 0: Project Setup

### 0.1 Repository Structure

- [ ] Create `Cargo.toml` with workspace configuration
- [ ] Set up `src/lib.rs` with module exports
- [ ] Configure `clippy.toml` and `rustfmt.toml`
- [ ] Add `.cargo/config.toml` for build settings
- [ ] Set up feature flags for optional components
- [ ] Create test directory structure:
  - [ ] `tests/common/` - Shared test fixtures and utilities
  - [ ] `tests/parity/` - Cross-implementation tests (vs Go/Python)
  - [ ] `tests/integration/` - Docker/K8s/external service tests
  - [ ] `tests/e2e/` - Full pipeline tests
- [ ] Create `benches/` for Criterion benchmarks

### 0.2 CI/CD Integration

- [ ] Integrate with existing `.hypersec-ci.yaml`
- [ ] Configure Artifactory publishing (create Rust repo if needed via `jf`)
- [ ] Add `cargo clippy`, `cargo fmt --check`, `cargo test` to CI
- [ ] Configure code coverage reporting

### 0.3 Documentation Setup

- [ ] Configure `cargo doc` generation
- [ ] Add README.md with usage examples
- [ ] Set up CHANGELOG.md

---

## Phase 1: Core Components (P0 - Critical)

### 1.1 Environment Detection (`env` module)

**Reference:** hs-golib `env/` package

**Tasks:**

- [ ] Define `Environment` enum (Kubernetes, Docker, Container, BareMetal)
- [ ] Implement K8s detection (service account token, env vars)
- [ ] Implement Docker detection (`.dockerenv` file)
- [ ] Implement container detection (cgroups inspection)
- [ ] Implement `detect()` function
- [ ] Add `is_container()`, `is_kubernetes()`, `is_helm()` helpers
- [ ] Write unit tests
- [ ] Write parity tests against hs-golib

**Estimated effort:** Small (~200 lines)

### 1.2 Runtime Paths (`runtime` module)

**Reference:** hs-golib `env/` (MountConfig), hs-lib `runtime/`

**Tasks:**

- [ ] Define `RuntimePaths` struct (config_dir, secrets_dir, data_dir, temp_dir, logs_dir, cache_dir)
- [ ] Implement XDG base directory support for local dev
- [ ] Implement container path defaults (`/app/config`, `/app/secrets`, etc.)
- [ ] Implement `discover_mounts()` based on detected environment
- [ ] Add path existence validation
- [ ] Write unit tests
- [ ] Write parity tests

**Estimated effort:** Small (~150 lines)

**Dependency:** 1.1 (env detection)

### 1.3 Configuration (`config` module)

**Reference:** hs-golib `config/`, hs-lib `config/`

**Crate:** `figment` (hierarchical config)

**Tasks:**

- [ ] Define `Config` struct wrapping Figment
- [ ] Implement 7-layer cascade:
  1. CLI args (via clap integration)
  2. Environment variables (with configurable prefix)
  3. `.env` file (via `dotenvy`)
  4. `settings.{env}.yaml`
  5. `settings.yaml`
  6. `defaults.yaml`
  7. Hard-coded defaults
- [ ] Implement typed getters (`get_string`, `get_int`, `get_bool`, `get_duration`)
- [ ] Implement `sub()` for scoped config access
- [ ] Implement `unmarshal()` to deserialise into structs
- [ ] Add global singleton pattern with `setup()` and `get()`
- [ ] Support YAML, TOML, JSON formats
- [ ] Write unit tests for each cascade layer
- [ ] Write parity tests against hs-golib

**Estimated effort:** Medium (~400 lines)

**Dependency:** 1.2 (runtime paths for config file locations)

### 1.4 Logger (`logger` module)

**Reference:** hs-golib `logger/`, hs-lib `logger/`

**Crate:** `tracing` + `tracing-subscriber`

**Tasks:**

- [ ] Define `LogFormat` enum (JSON, Text, Auto)
- [ ] Define `LoggerOptions` struct (level, format, output, add_source, enable_masking, sensitive_fields)
- [ ] Implement TTY detection for auto-format selection
- [ ] Implement JSON formatter with RFC 3339 timestamps (with timezone)
- [ ] Implement Text formatter with Solarised colours
- [ ] Implement sensitive data masking filter
- [ ] Define default sensitive fields list (password, token, secret, api_key, etc.)
- [ ] Implement global setup via `tracing::subscriber::set_global_default`
- [ ] Add `LOG_LEVEL`, `LOG_FORMAT`, `NO_COLOR` env overrides
- [ ] Write unit tests
- [ ] Write parity tests against hs-golib

**Estimated effort:** Medium (~350 lines)

**Dependency:** 1.1 (env detection for format auto-selection)

### 1.5 Metrics (`metrics` module)

**Reference:** hs-golib `metrics/`, hs-lib `metrics/`

**Crate:** `metrics` + `metrics-exporter-prometheus`

**Tasks:**

- [ ] Define `MetricsManager` struct
- [ ] Define `MetricsConfig` (namespace, enable_process_metrics, update_interval)
- [ ] Implement Counter, Gauge, Histogram, Summary wrappers
- [ ] Implement process metrics collection:
  - [ ] CPU usage
  - [ ] Memory usage (RSS, heap)
  - [ ] Open file descriptors
  - [ ] Goroutine equivalent (tokio task count if available)
- [ ] Implement container metrics collection (cgroups v1/v2):
  - [ ] Memory limit and usage
  - [ ] CPU limit in cores
- [ ] Implement HTTP handler for `/metrics` endpoint
- [ ] Implement standalone metrics server with `/metrics`, `/healthz`, `/readyz`
- [ ] Implement auto-update background task
- [ ] Add `start_server()` and `stop_server()` with graceful shutdown
- [ ] Provide helper functions: `latency_buckets()`, `size_buckets()`
- [ ] Write unit tests
- [ ] Write parity tests against hs-golib

**Estimated effort:** Large (~600 lines)

**Dependency:** 1.1 (env detection for container metrics)

---

## Phase 2: Extended Components (P2 - Later)

### 2.1 HTTP Client (`http` module)

**Reference:** hs-lib `http/`

**Crate:** `reqwest` + `reqwest-retry` or `tower` middleware

**Tasks:**

- [ ] Define `HttpClient` struct wrapping reqwest
- [ ] Implement automatic retry with exponential backoff
- [ ] Add configurable timeout (default 30s)
- [ ] Add base URL support
- [ ] Integrate with metrics (request count, latency histogram)
- [ ] Integrate with tracing (span per request)
- [ ] Write unit tests
- [ ] Write integration tests

**Estimated effort:** Medium (~300 lines)

### 2.2 Database URL Builders (`database` module)

**Reference:** hs-lib `database/`

**Tasks:**

- [ ] Define `DatabaseType` enum (PostgreSQL, MySQL, ClickHouse, Redis, MongoDB)
- [ ] Implement `build_database_url(db_type)` from env vars
- [ ] Implement `get_database_config(db_type)` returning config struct
- [ ] Support env var patterns: `{DB_TYPE}_HOST`, `{DB_TYPE}_PORT`, etc.
- [ ] Write unit tests
- [ ] Write parity tests against hs-lib

**Estimated effort:** Small (~150 lines)

### 2.3 Cache (`cache` module)

**Reference:** hs-lib `cache/`

**Crate:** `cached` or custom with SQLite backend

**Tasks:**

- [ ] Define cache interface with TTL support
- [ ] Implement disk-backed storage
- [ ] Implement per-source TTL configuration
- [ ] Add cache invalidation by source
- [ ] Write unit tests

**Estimated effort:** Medium (~300 lines)

---

## Phase 3: Testing and Validation

### 3.1 Parity Test Suite

- [ ] Create test harness that runs same scenarios against Rust and Go implementations
- [ ] Config cascade parity tests
- [ ] Logger output format parity tests
- [ ] Metrics exposition format parity tests
- [ ] Environment detection parity tests

### 3.2 Integration Tests

- [ ] Docker container deployment tests
- [ ] Kubernetes deployment tests (if applicable)
- [ ] Metrics scraping tests

### 3.3 Documentation and Examples

- [ ] Complete API documentation
- [ ] Example: CLI application
- [ ] Example: HTTP service with metrics
- [ ] Example: Configuration loading

---

## Dependency Graph

```text
Phase 0: Setup
    │
    ▼
┌───────────────────────────────────────────────────┐
│ 1.1 Env Detection                                 │
└───────────────────────────────────────────────────┘
    │
    ├─────────────────┬─────────────────┐
    ▼                 ▼                 ▼
┌─────────┐     ┌─────────┐       ┌─────────┐
│1.2 Paths│     │1.4 Logger│      │1.5 Metrics│
└─────────┘     └─────────┘       └─────────┘
    │
    ▼
┌─────────┐
│1.3 Config│
└─────────┘
    │
    ▼
Phase 2: HTTP, Database, Cache
    │
    ▼
Phase 3: Testing
```

---

## Milestones

| Milestone | Components | Deliverable |
| --------- | ---------- | ----------- |
| M1 | 0.x, 1.1, 1.2 | Project setup + env detection + paths |
| M2 | 1.3 | Configuration with 7-layer cascade |
| M3 | 1.4 | Structured logging with masking |
| M4 | 1.5 | Prometheus metrics with container awareness |
| M5 | 3.1 | Parity tests passing |
| M6 | 2.x | Extended components (HTTP, DB, Cache) |

---

## Success Criteria

1. All P0 components implemented and tested
2. Parity tests pass against hs-golib for equivalent functionality
3. Code coverage > 80%
4. `cargo clippy` passes with no warnings
5. Published to Artifactory
6. Documentation complete with examples

---

**Last Updated:** 2025-12-24
