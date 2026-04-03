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
>
> **Trust but verify:** Plan assessments are hypotheses. Read actual code before
> starting each app. Update the plan with what you find. If code contradicts
> the plan, trust the code.
>
> **Release discipline:** CI pass ≠ done. EVERY project (rustlib + all dfe-*)
> must be PUBLISHED (rustlib → crates.io, dfe-* → JFrog) before marking
> complete. Push + CI + semantic-release tag + `hyperi-ci release <tag>` +
> verify in registry. Can continue other work while waiting, but must track
> and complete releases.

Phase 1 done (all 6 apps on rustlib >=2.4.3, ServiceRuntime, released).
Phase 2 plan: `docs/superpowers/plans/2026-04-03-dfe-phase2-deep-integration.md`

**Phase 2A: Evolve BatchEngine API (rustlib)** — DONE
- [x] run_async() + run_raw_async(): async sink, commit tokens, optional ticker
- [x] Doc-test fixes (No SEP: flat_env unsafe set_var, registry MyConfig)
- [x] rustlib v2.4.4 published to crates.io

**Phase 2B: Per-App Integration** — ALL CODE DONE, RELEASING `[IN PROGRESS]`
- [x] dfe-loader: BatchEngine pre-route SIMD filtering + SOC2 audit
  - 592 tests pass (all against real ClickHouse). Publish pipeline running.
- [x] dfe-receiver: process_batch() for HEC/OTLP/fluent/prometheus_rw
  - 408 tests pass. CI re-triggered, awaiting semantic-release tag.
- [x] dfe-archiver: concurrent batch writes + parallel routing + SOC2
  - 80 tests pass. Publish pipeline running.
- [x] dfe-fetcher: concurrent within-source service fetching (AWS/Azure/M365/GCP)
  - 164 tests pass. Clippy fix pushed, CI re-running.
- [x] dfe-transform-vrl: sonic-rs SIMD JSON parse (drop-in serde_json replacement)
  - 295 tests pass. CI running.
- [x] dfe-transform-vector: no changes needed (subprocess wrapper)
- [ ] ALL apps: verify released (rustlib → crates.io, dfe-* → JFrog)
  - rustlib v2.4.4: RELEASED to crates.io
  - dfe-loader v1.16.5: publish dispatched, pipeline running
  - dfe-archiver v1.6.2: publish dispatched, pipeline running
  - dfe-receiver: awaiting CI pass → semantic-release → publish
  - dfe-fetcher: awaiting CI pass → semantic-release → publish
  - dfe-transform-vrl: awaiting CI pass → semantic-release → publish

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

### Completed This Session (Phase 2 Deep Integration)

- [x] **BatchEngine API evolution** — run_async(), run_raw_async(), ticker, doc-test fixes
- [x] **Rustlib v2.4.4** published to crates.io
- [x] **dfe-loader** — SIMD pre-route filtering, engine pool, SOC2 audit, clippy fixes
- [x] **dfe-receiver** — process_batch() for HEC/OTLP/fluent/prometheus_rw batching
- [x] **dfe-archiver** — concurrent batch writes via join_all, parallel routing, SOC2
- [x] **dfe-fetcher** — concurrent within-source service fetching (AWS/Azure/M365/GCP)
- [x] **dfe-transform-vrl** — sonic-rs SIMD JSON parse (drop-in serde_json replacement)
- [x] **Rust standards** — added useless .into() AI pitfall to RUST.md
- [x] **No SEP fixes** — clippy warnings in loader tests, doc-test failures in rustlib,
  unused imports, missing semicolons, formatting across all repos

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
