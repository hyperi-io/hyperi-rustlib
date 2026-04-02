# TODO - hyperi-rustlib

**Project Goal:** Opinionated Rust framework for high-throughput data pipelines at PB scale

**Target:** Production-ready library with auto-wiring config, logging, metrics, tracing, health, and graceful shutdown

---

## Current Tasks

### DFE Full Remediation — Rustlib v2.0→HEAD Catch-Up

All 6 DFE apps need full catch-up covering BatchEngine, ServiceRuntime,
TopicResolver, metrics manifest, RuntimeContext, deployment contract, SIMD
parse, pre-route filtering, field interning, etc.

Plan: `docs/superpowers/plans/2026-04-02-dfe-full-remediation.md`

Remediation order:
- [ ] dfe-loader `[IN PROGRESS]`
  - Current state: waiting for CI to publish new rustlib version
  - Next: bump rustlib version, adopt ServiceRuntime + BatchEngine, delete bespoke TopicResolver
  - Heaviest — ~500 line delta
- [ ] dfe-receiver
  - BatchEngine standalone `process_mid_tier` (HTTP inbound)
  - BatchAccumulator for Splunk HEC request batching
- [ ] dfe-archiver
  - BatchEngine `run_raw` (passthrough, no parse)
  - TopicResolver auto-discovery (`topics: []`)
- [ ] dfe-fetcher
  - BatchEngine standalone `process_mid_tier`
  - `fan_out_async` for within-source parallel service fetching
- [ ] dfe-transform-vector
  - Lightest — ServiceRuntime only, no BatchEngine (Vector owns pipeline)
- [ ] dfe-transform-vrl
  - BatchEngine `run()` replaces pipeline loop
  - sonic-rs replaces serde_json

Per-app remediation includes:
- Bump hyperi-rustlib to latest
- Push committed DfeMetrics::register(&mgr) fix
- Adopt ServiceRuntime (remove manual MetricsManager/MemoryGuard/pool/shutdown wiring)
- Adopt BatchEngine (replace manual recv→parse→transform loop)
- TopicResolver where applicable
- Generate deployment contract artefacts

### Deferred

- [ ] Columnar (SoA) batch layout — backlogged, benchmark later
- [ ] String interning deeper integration — custom Value type with interned keys
- [ ] Disk buffer improvements (rkyv zero-copy) — separate concern (tiered-sink/spool)
- [ ] Metrics manifest enrichment — `set_use_cases()` / `set_dashboard_hint()` content
- [ ] Mutex audit documentation (in standards)
- [ ] Phase 3: dfe-transform-wasm, dfe-transform-elastic, dfe-transform-splunk

### Completed This Session (2026-04-02)

- [x] **BatchEngine** — SIMD-optimised batch processing framework (15 commits)
  - sonic-rs SIMD JSON parse (2-4x faster than serde_json)
  - Pre-route field extraction via `get_from_slice` (skip full parse for filtered/DLQ)
  - FieldInterner (DashMap concurrent field name dedup)
  - Two tiers: mid-tier (materialised DOM) and full-tier (raw passthrough)
  - Transport-wired `run()` loop + standalone `process_mid_tier()`/`process_raw()`
  - Auto-wired from ServiceRuntime — zero boilerplate for apps
  - MemoryGuard-bounded chunking between batch chunks
  - 78 tests (unit + integration + adversarial + concurrent + 20K scale)
  - Criterion benchmarks (engine vs manual serde_json)
- [x] **TopicResolver** — Kafka topic auto-discovery in rustlib transport-kafka
  - Configurable suppression rules (default: `_load` suppresses `_land`)
  - Include/exclude regex filters
  - TopicRefreshHandle for periodic re-resolution
  - Wired into KafkaTransport::new() — auto-discovers when `topics: []`
- [x] **PipelineStats.filtered** — new atomic counter for pre-route filtered messages
- [x] **AdaptiveWorkerPool.install()** — expose rayon pool for `par_iter_mut`
- [x] **Dependencies** — sonic-rs, dashmap, bytes, regex added to worker/transport-kafka

### Completed Previous Sessions

- [x] BatchAccumulator (bounded channel, time/count/bytes drain thresholds)
- [x] NDJSON split utilities (split_lines, count_lines)
- [x] Adversarial worker pool tests (34 tests)
- [x] RuntimeContext + startupz integration tests
- [x] ServiceRuntime (auto-wired DfeApp infrastructure)
- [x] RuntimeContext (K8s metadata, cgroup limits)
- [x] Deployment contract CI bridge (container-manifest.json, OCI labels)
- [x] Metrics manifest + CLI subcommands
- [x] AdaptiveWorkerPool (rayon + tokio hybrid)
- [x] BatchProcessor trait + BatchPipeline + PipelineStats
- [x] Code review remediation — security fixes, panic removal, health wiring
- [x] DLQ HTTP + Redis backends
- [x] Transport trait split + factory + File/Pipe/HTTP/Redis transports
- [x] HealthRegistry, Shutdown manager, OTel trace propagation
- [x] KafkaTransport StatsContext, gRPC transport metrics
- [x] Config registry, SensitiveString, /config endpoint

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
- BatchEngine spec: `docs/superpowers/specs/2026-04-02-batch-engine-design.md`
- TopicResolver spec: `docs/superpowers/specs/2026-04-02-topic-resolver-design.md`
- DFE remediation plan: `docs/superpowers/plans/2026-04-02-dfe-full-remediation.md`
- Two deployment modes: Kafka-mediated (persistence) vs direct gRPC (low latency)
- Routed transport is receiver/fetcher only — all other stages are 1:1
- Common patterns in rustlib first — if all 6 DFE projects use the same pattern, implement in rustlib
