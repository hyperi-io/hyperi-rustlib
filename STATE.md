## CI

CI is live via `hyperi-ci`. Quality, Test, Build, Release, and Publish stages
run on push to `main`. Use `hyperi-ci check` locally before pushing.

**This project is the first Rust project on the new CI.** It is a transitive
dependency of every downstream HyperI Rust service (dfe-loader, dfe-archiver,
dfe-receiver, etc.).

---

# Project State

**Project:** hyperi-rustlib
**Purpose:** Shared Rust utility library for HyperI applications (port of hyperi-pylib/hyperi-golib)

---

## Architecture

Modular library with feature-gated components. Each module can be enabled/disabled independently via Cargo features.

### Key Components

1. **env** - Environment detection (K8s, Docker, Container, BareMetal)
2. **runtime** - Runtime paths with XDG/container awareness
3. **config** - 8-layer configuration cascade (figment)
4. **logger** - Structured logging with JSON/text formats (tracing)
5. **metrics** - Prometheus metrics with process/container awareness
6. **otel-metrics** - OpenTelemetry metrics export (OTLP)
7. **directory-config** - YAML directory-backed config store with optional git2 integration
8. **spool** - Disk-backed async FIFO queue (yaque)
9. **tiered-sink** - Resilient message delivery with disk spillover
10. **transport** - Kafka/gRPC/Memory transport abstraction
11. **http-server** - Axum-based HTTP server with health endpoints
12. **secrets** - Secrets management (OpenBao/Vault, AWS Secrets Manager)
13. **worker** - AdaptiveWorkerPool (rayon-backed parallel processing with pressure-based scaling)
14. **batch-engine** - BatchEngine: SIMD parse (sonic-rs), pre-route filtering, field interning, chunked rayon. APIs: process_mid_tier(), process_raw(), run_async(), run_raw_async()
15. **memory** - MemoryGuard: cgroup-aware memory backpressure with auto-detection
16. **scaling** - ScalingPressure: KEDA autoscaling signal calculation
17. **cli** - DfeApp trait, ServiceRuntime (pre-wired metrics + worker pool + batch engine + memory guard + shutdown)
18. **transport-filter** - TransportFilterEngine: CEL-syntax message filtering embedded in every transport. Tier 1 SIMD field ops (~50-100ns), Tier 2 compiled CEL (opt-in), Tier 3 complex CEL with regex/iteration (opt-in). Inbound/outbound, drop/dlq, first-match-wins.

### Tech Stack

- **Language:** Rust (edition 2024, pinned to latest stable, currently 1.94)
- **Config:** figment (0.10)
- **Logging:** tracing + tracing-subscriber (0.3)
- **Metrics:** metrics + metrics-exporter-prometheus, OpenTelemetry
- **Async:** tokio (1.50+)
- **Kafka:** rdkafka (0.39, dynamic-linking against system librdkafka)
- **gRPC:** tonic + prost (0.14)
- **Disk Queue:** yaque (0.6)
- **YAML:** serde-yaml-ng (0.10)
- **HTTP Server:** axum (0.8)
- **HTTP Client:** reqwest (0.12/0.13) + reqwest-middleware
- **Secrets:** vaultrs, aws-sdk-secretsmanager
- **Git:** git2 (0.20, links system libgit2 when available)

---

## Native Dependencies (Dynamic Linking)

This crate dynamically links against system C libraries instead of compiling
them from source. This drops build times significantly (rdkafka alone was
30 minutes of C++ compilation).

**Build host** needs `-dev` packages; **deployment host** needs runtime libs.
See [README.md](README.md) for full package tables and Docker examples.

| Feature | Crate | Build Package | Runtime Package |
|---------|-------|--------------|-----------------|
| `transport-kafka` | `rdkafka-sys` | `librdkafka-dev` (>= 2.12.1, Confluent repo) | `librdkafka1` |
| `directory-config-git` | `libgit2-sys` | `libgit2-dev` | `libgit2-1.7` |
| `spool` / `tiered-sink` | `zstd-sys` | `libzstd-dev` | `libzstd1` |
| (transitive) | `openssl-sys` | `libssl-dev` | `libssl3` |
| (transitive) | `libz-sys` | `zlib1g-dev` | `zlib1g` |
| `secrets-aws` | `aws-lc-sys` | — (compiled from source, ~20-30s, sccache-cached) | — (statically linked) |

`hyperi-ci` auto-detects which `-sys` crates are in `Cargo.lock` and installs
matching packages. The Confluent APT repo is added automatically when
`rdkafka-sys` is detected and the installed version is below the minimum.

---

## Build Configuration

**NEVER kill cargo processes.** Cargo holds a file lock on the build directory. Killing a process leaves the lock held, causing subsequent `cargo` invocations to block indefinitely on "Blocking waiting for file lock on build directory". If a build seems stuck, wait for it to finish or check for orphaned processes first.

---

## Registry

- **Package name:** `hyperi-rustlib` (renamed from `hs-rustlib` in v1.4.3)
- **Registry:** `hyperi` (JFrog Artifactory at `hypersec.jfrog.io`)
- **Virtual repo:** `hyperi-cargo-virtual`
- **Local repo:** `hyperi-cargo-local`

### Downstream consumers

| Project | Dep version | Features |
|---------|------------|----------|
| dfe-loader | `>=2.4.3` | transport-kafka, transport-grpc, dlq-kafka, config, config-reload, deployment, version-check, scaling, cli, top, logger, metrics, metrics-dfe, expression, memory, worker |
| dfe-archiver | `>=2.4.3` | config, config-reload, logger, metrics, metrics-dfe, transport-kafka, http-server, tiered-sink, spool, scaling, deployment, cli, memory, health, shutdown, dlq, worker |
| dfe-receiver | `>=2.4.3` | config, config-reload, logger, metrics, metrics-dfe, transport-kafka, http-server, tiered-sink, spool, scaling, deployment, cli, memory, health, shutdown, dlq, worker |
| dfe-fetcher | `>=2.0.0` | config, config-reload, logger, metrics, metrics-dfe, transport-kafka, transport-grpc, http-server, scaling, deployment, cli, memory, health, shutdown, dlq, expression, worker |
| dfe-transform-vrl | `>=2.4.3` | cli, config, config-reload, deployment, logger, metrics, metrics-dfe, scaling, memory, worker, version-check |
| dfe-transform-vector | `>=2.0.0` | cli, config-reload, deployment, http-server, logger, metrics, metrics-dfe, version-check |

---

## Core Pillars (Non-Negotiable Design Decision)

Every module in hyperi-rustlib MUST auto-integrate with the core infrastructure
pillars using the **global singleton pattern**. Services get observability for
free — no handles passed, no opt-in, no extra code in downstream apps.

| Pillar | Singleton | Module | Pattern |
|--------|-----------|--------|---------|
| Config | `OnceLock<Config>` | `config` | `T::from_cascade()` reads from global figment |
| Logging | Global `tracing` subscriber | `logger` | `tracing::info!()` macros — always available |
| Metrics | Global `metrics` recorder | `metrics` | `metrics::counter!()` macros — no-op if no recorder |
| Tracing | Global OTel subscriber | `otel` | W3C traceparent auto-propagated in gRPC/Kafka/HTTP |
| Health | Global `HealthRegistry` | `health` | Modules auto-register, `/readyz` aggregates |
| Shutdown | `CancellationToken` | `shutdown` | SIGTERM/SIGINT → all modules drain gracefully |

**Rule:** When adding ANY new module or feature to rustlib:
1. If it has configurable behaviour → load from cascade via `from_cascade()`
2. If it does I/O or processing → add `#[cfg(feature = "metrics")]` counters/gauges/histograms
3. If it can fail or has interesting state → add `tracing::` log calls
4. If it affects service health → report into unified `HealthState`

**The goal:** A DFE app that does `MetricsManager::new("dfe_loader")` +
`logger::setup_default()` + `config::setup()` at startup gets full
observability across every rustlib feature it uses — transport, tiered-sink,
spool, cache, secrets, HTTP client, DLQ — with zero additional wiring.

---

## Pre-GA Status

**hyperi-rustlib has NOT reached GA.** Until Derek explicitly says "it's GA",
every commit is `fix:` (PATCH) or `feat:` (MINOR) — never write
`BREAKING CHANGE:` in any commit footer, even when removing/renaming public
APIs or trimming default features. The 6 downstream DFE projects (loader,
receiver, fetcher, archiver, transform-vrl, transform-vector) migrate in
lockstep with rustlib; there are no external GA consumers to protect.

## Decisions

- **Dynamic linking for C deps** — rdkafka, libgit2, zstd, zlib, openssl all link against system libs via pkg-config. Eliminates ~30min C++ build for rdkafka. aws-lc-sys is the one exception (AWS SDK hardcodes it, no opt-out).
- **sqlx uses ring crypto** — `tls-rustls-ring-webpki` feature avoids cmake-based aws-lc-sys for sqlx's TLS
- **AES-256-GCM** for license encryption (audited crate, authenticated encryption)
- **obfstr** for string obfuscation (compile-time XOR, no proc-macro complexity)
- **Ed25519** for signatures (ed25519-dalek, well-maintained)
- **serde-yaml-ng** replaced serde_yml (security fix)
- **yaque** replaced queue-file (async-native, maintained)
- **std::sync::LazyLock** replaced once_cell
- **fs4** replaced fs2 (unmaintained, fs4 is the maintained pure-Rust successor)
- **git2 kept over gix** — gix (pure Rust) lacks high-level write ops (add, commit, checkout). Revisit when gix matures.
- **Package rename** from `hs-rustlib` to `hyperi-rustlib` to match org rebrand
- **Config cascade unified spec** — rustlib and pylib must be identical. Both search `./`, `./config/`, `/config/`, `~/.config/{app_name}/`. Home `.env` opt-in. PG layer is built-for-not-with (YAML gitops already centralised). See [CONFIG-CASCADE.md](docs/CONFIG-CASCADE.md)
- **Common patterns in rustlib first** — if all 6 DFE projects will use the same pattern, implement it in rustlib first, publish, then consume. Never duplicate common logic across downstream projects. This applies to: worker pool, batch pipeline, pipeline stats, DLQ routing, metrics groups, config cascade, CLI framework.
- **DFE parallelisation pattern** — split sequential hot loops into parallel (pure `&self` computation via rayon) and sequential (mutable state: buffer push, mark_pending, stats, DLQ) phases. The `BatchProcessor` trait + `BatchPipeline` struct in rustlib provide the common framework. Each DFE app implements `BatchProcessor` for its domain. See `src/pipeline/` module.
- **ServiceRuntime** — pre-built infrastructure for DFE service apps. Created by `run_app()` before `run_service()`. Contains MetricsManager, DfeMetrics, MemoryGuard (optional), shutdown token (with K8s pre-stop delay), worker pool (optional), batch engine (optional), scaling pressure (optional), RuntimeContext. Apps receive it fully wired. See `src/cli/runtime.rs`.
- **BatchEngine** — SIMD-optimised batch processing for DFE pipelines. Two modes: `process_mid_tier()` (parse JSON via sonic-rs + parallel transform via rayon) and `process_raw()` (skip parsing, parallel transform on raw bytes). Transport-wired: `run_async()` / `run_raw_async()` with async sink, sink-managed commit tokens, and optional ticker callback. See `src/worker/engine/`.
- **TransportFilterEngine** — CEL-syntax message filtering embedded in every transport (Kafka, gRPC, Memory, File, Pipe, HTTP, Redis). Three performance tiers: Tier 1 (SIMD field ops via sonic_rs::get_from_slice, ~50-200ns/msg, always enabled), Tier 2 (compiled CEL with extracted fields, requires `expression.allow_cel_filters_in/out`), Tier 3 (CEL with regex/iteration/time, requires `expression.allow_complex_filters_in/out`). Operators write CEL syntax — engine classifies via text pattern matching and bypasses CEL engine entirely for Tier 1. First-match-wins, drop/dlq actions, fail-fast at startup. Zero downstream code changes — config-only activation. See `src/transport/filter/`.
- **RuntimeContext** — rich runtime metadata detected once at startup (pod_name, namespace, node_name, container_id, memory_limit_bytes, cpu_quota_cores). Global singleton via OnceLock. All modules read from this instead of doing their own env var lookups. No-ops on bare metal. See `src/env.rs`.
- **K8s pre-stop compliance** — shutdown handler sleeps `PRESTOP_DELAY_SECS` (default 5 in K8s, 0 elsewhere) before cancelling the token. Prevents traffic routing to a draining pod.
- **Deployment contract CI bridge** — `container-manifest.json` (minimal CI subset), `Dockerfile.runtime` (runtime stage fragment for CI composition), OCI labels (static from contract, dynamic injected by CI), `from_cargo_toml()` for auto-detecting native deps, `schema_version` field.
- **Commit type discipline** — `fix:` for most changes (PATCH bump). `feat:` only for genuinely new user-facing features. `BREAKING CHANGE:` NEVER written without explicit user approval. See `/projects/hyperi-ai/docs/superpowers/plans/2026-04-01-commit-type-enforcement.md`.

---

## Resources

- [README.md](README.md) - Quick start, native deps, feature list
- [DESIGN.md](docs/DESIGN.md) - Architecture and API design
- [CONFIG-CASCADE.md](docs/CONFIG-CASCADE.md) - Configuration cascade reference
- [TODO.md](TODO.md) - Task tracking
- [GAP_ANALYSIS.md](docs/GAP_ANALYSIS.md) - Comparison with hyperi-pylib

---

## Notes for AI Assistants

This file contains static project context only. For tasks and progress, see TODO.md.
For version, use `git describe --tags`. For history, use `git log`.

### License Module Production Notes

Before deploying to production:

1. Change the obfuscated key in `src/license/defaults.rs` (`get_decryption_key()`)
2. Replace the Ed25519 public key in `src/license/integrity.rs` (`get_public_key_bytes()`)
3. Generate license files externally using `encrypt_license()` with your secret key
