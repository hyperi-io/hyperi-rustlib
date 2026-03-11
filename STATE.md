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

**IMPORTANT:** Use `CARGO_BUILD_JOBS=2` for all cargo commands:

```bash
CARGO_BUILD_JOBS=2 cargo build
CARGO_BUILD_JOBS=2 cargo test
CARGO_BUILD_JOBS=2 cargo clippy
```

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
| dfe-loader | `>=1.2.2` | transport-kafka |
| dfe-archiver | `>=1.3` | config, logger, metrics, transport-kafka, spool, tiered-sink |
| dfe-receiver | `1.3` | config, logger, metrics, http-server, transport-kafka, spool, tiered-sink, runtime, secrets |

---

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
