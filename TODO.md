# TODO - hyperi-rustlib

**Project Goal:** Opinionated Rust framework for high-throughput data pipelines at PB scale

**Target:** Production-ready library with auto-wiring config, logging, metrics, tracing, health, and graceful shutdown

---

## Current Tasks

### DFE Full Remediation — Phase 2: Deep Integration (AMENDED 2026-04-03)

> **Scope directive:** Don't go light. The whole point of BatchEngine and
> parallelisation is to FORCE architectural improvement in all apps. Every app
> gets: SIMD parsing, pre-route filtering, chunked rayon, field interning,
> standardised PipelineStats. Where the engine API doesn't fit, evolve the engine.
>
> **No SEP rule:** Every warning, clippy lint, doc-test failure, or bug
> encountered during this remediation is OUR problem. We own every line we
> touch AND every pre-existing issue in files we modify. Fix it there and then.

Phase 1 done (all 6 apps on rustlib >=2.4.3, ServiceRuntime, released).
Phase 2 plan: `docs/superpowers/plans/2026-04-03-dfe-phase2-deep-integration.md`

**Phase 2A: Evolve BatchEngine API (rustlib)**
- [ ] Async sink for run() / run_raw() (enables loader's async CH inserts)
- [ ] Optional ticker callback (enables flush timers inside engine loop)
- [ ] TransportReceiver impl for loader's TransportBackend (clean adapter)
- [ ] Publish rustlib with engine changes

**Phase 2B: Per-App Integration**
- [ ] dfe-loader: engine.process_mid_tier() replaces MessageProcessor
  - SIMD parse (sonic-rs), pre-route routing_field="_table", field interning
  - Transform closure wraps existing pipeline (route→extract→enrich)
  - BatchCoordinator stays (sequential buffer push, DLQ)
  - Orchestrator select! loop stays (5 arms: shutdown, config, flush, schema, recv)
- [ ] dfe-transform-vrl: engine.process_mid_tier() replaces deser+VRL phases
  - Benchmark: sonic_rs::Value → vrl::Value conversion cost first
  - Single engine call replaces two pool.process_batch() calls
  - Produce stays sequential (async I/O)
- [ ] dfe-archiver: engine.run_raw() or process_raw() + TopicResolver
  - Pre-route routing_field="_destination", TopicResolver auto-discovery
  - Flush timer as separate task (or engine ticker if evolved)
- [ ] dfe-receiver: BatchAccumulator for Splunk HEC only (additive)
  - DO NOT replace zero-copy hot path (validation, routing, enrichment)
  - Existing optimisations proven faster than generic alternatives
- [ ] dfe-fetcher: fan_out_async() + engine.process_raw() for post-fetch
  - JoinSet in each Source impl (AWS/Azure/M365/GCP)
  - Parallel enrich+filter via engine
- [ ] dfe-transform-vector: generate-artefacts only
- [ ] ALL apps: run `generate-artefacts` for deployment contract

**Phase 1 DONE (all apps):**
- [x] Bump rustlib to >=2.4.3
- [x] DfeMetrics::register(&mgr) signature fix
- [x] ServiceRuntime adoption (where not already done)
- [x] DeploymentContract schema_version + oci_labels fields
- [x] Debug/trace logging across all pipeline stages
- [x] Target symlink + orphan submodule fixes
- [x] All 6 apps released with Phase 1 changes

### clickhouse-rs

- [x] Schema cache drift detection (issue #20 fix in dfe-loader)
- [x] Expanded SchemaMismatch error classification
- [x] Auto-invalidate schema cache on end() failure
- [x] Nullable(JSON) RowBinary insert fix (c178b37)
- [x] Nullable/non-nullable tests for ALL types
- [x] Upstream PR: ClickHouse/clickhouse-rs#414
- [ ] Monitor upstream PR review feedback

### Deferred

- [ ] Columnar (SoA) batch layout — backlogged, benchmark later
- [ ] String interning deeper integration — custom Value type with interned keys
- [ ] Disk buffer improvements (rkyv zero-copy) — tiered-sink/spool
- [ ] Metrics manifest enrichment — set_use_cases() / set_dashboard_hint()
- [ ] Mutex audit documentation (in standards)
- [ ] Phase 3: dfe-transform-wasm, dfe-transform-elastic, dfe-transform-splunk
- [ ] Debug/trace logging increasing detail levels (all repos)

### Completed This Session

- [x] **BatchEngine** — 15 commits, SIMD batch processing framework
- [x] **TopicResolver** — Kafka topic auto-discovery with suppression rules
- [x] **PipelineStats.filtered** counter
- [x] **AdaptiveWorkerPool.install()** exposure
- [x] **Prometheus recorder test fix** — test-safe MetricsManager
- [x] **Rustlib v2.4.3** published to crates.io
- [x] **clickhouse-rs schema-cache-strong** branch — 3 commits + Nullable(JSON) fix
- [x] **Upstream PR** ClickHouse/clickhouse-rs#414
- [x] **dfe-loader** v1.16.4 — ServiceRuntime, TopicResolver deletion, schema cache fix, logging
- [x] **dfe-receiver** v1.14.8 — ServiceRuntime, logging, target symlink + orphan ci fix
- [x] **dfe-archiver** v1.6.1 — ServiceRuntime, logging, deployment contract fields
- [x] **dfe-fetcher** v1.1.8 — ServiceRuntime, logging, SensitiveString fix
- [x] **dfe-transform-vector** — ServiceRuntime, logging, SensitiveString fix
- [x] **dfe-transform-vrl** — rustlib bump, logging (CI pending)
- [x] **Target symlink fix** across 4 repos (receiver, archiver, fetcher, vrl)
- [x] **Orphan ci submodule** removed from dfe-receiver

### Completed Previous Sessions

- [x] BatchAccumulator, NDJSON split, adversarial worker tests
- [x] RuntimeContext, ServiceRuntime, deployment contract CI bridge
- [x] Metrics manifest + CLI subcommands
- [x] AdaptiveWorkerPool, BatchProcessor, PipelineStats
- [x] All transport backends, HealthRegistry, Shutdown manager
- [x] Config registry, SensitiveString, /config endpoint

---

## Backlog

### Secrets Providers

- [ ] GCP Secret Manager provider (`secrets-gcp` feature)
- [ ] Azure Key Vault provider (`secrets-azure` feature)

### Other

- [ ] PII anonymiser (evaluate Rust libraries)
- [ ] Python bindings for ClickHouse client (PyO3)

---

## Notes

- BatchEngine spec: `docs/superpowers/specs/2026-04-02-batch-engine-design.md`
- TopicResolver spec: `docs/superpowers/specs/2026-04-02-topic-resolver-design.md`
- DFE remediation plan (Phase 1): `docs/superpowers/plans/2026-04-02-dfe-full-remediation.md`
- DFE Phase 2 plan (amended): `docs/superpowers/plans/2026-04-03-dfe-phase2-deep-integration.md`
- Phase 2A evolves the engine API first (async sink, ticker, transport adapter)
- Phase 2B applies engine per-app: loader → VRL → archiver → receiver → fetcher → vector
- Common patterns in rustlib first — if all 6 DFE projects use the same pattern, implement in rustlib
