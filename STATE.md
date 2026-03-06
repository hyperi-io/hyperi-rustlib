## CI UNAVAILABLE — Commit with [skip ci]

The CI system is being completely rewritten. Until migration instructions are provided:

- **All commits MUST include `[skip ci]` in the commit message**
- Do not trigger CI runs or rely on CI for validation
- Run `./ci/local-build.sh` for local validation if it exists

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
3. **config** - 7-layer configuration cascade (figment)
4. **logger** - Structured logging with JSON/text formats (tracing)
5. **metrics** - Prometheus metrics with process/container awareness
6. **otel-metrics** - OpenTelemetry metrics export (OTLP)
7. **directory-config** - YAML directory-backed config store with optional git2 integration
8. **spool** - Disk-backed async FIFO queue (yaque)
9. **tiered-sink** - Resilient message delivery with disk spillover
10. **transport** - Kafka/Zenoh/Memory transport abstraction
11. **http-server** - Axum-based HTTP server with health endpoints
12. **secrets** - Secrets management (OpenBao/Vault, AWS Secrets Manager)

### Tech Stack

- **Language:** Rust 1.80+ (MSRV)
- **Config:** figment (0.10)
- **Logging:** tracing + tracing-subscriber (0.3)
- **Metrics:** metrics + metrics-exporter-prometheus, OpenTelemetry
- **Async:** tokio (1.0)
- **Disk Queue:** yaque (0.6)
- **YAML:** serde-yaml-ng (0.10)
- **HTTP Server:** axum (0.8)
- **Secrets:** vaultrs, aws-sdk-secretsmanager

---

## Build Configuration

**IMPORTANT:** Use `CARGO_BUILD_JOBS=2` for all cargo commands:

```bash
CARGO_BUILD_JOBS=2 cargo build
CARGO_BUILD_JOBS=2 cargo test
CARGO_BUILD_JOBS=2 cargo clippy
```

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

- **AES-256-GCM** for license encryption (audited crate, authenticated encryption)
- **obfstr** for string obfuscation (compile-time XOR, no proc-macro complexity)
- **Ed25519** for signatures (ed25519-dalek, well-maintained)
- **serde-yaml-ng** replaced serde_yml (security fix)
- **yaque** replaced queue-file (async-native, maintained)
- **std::sync::LazyLock** replaced once_cell (MSRV 1.80)
- **Package rename** from `hs-rustlib` to `hyperi-rustlib` to match org rebrand
- **Config cascade unified spec** — rustlib and pylib must be identical. Both search `./`, `./config/`, `/config/`, `~/.config/{app_name}/`. Home `.env` opt-in. PG layer is built-for-not-with (YAML gitops already centralised). See [CONFIG-CASCADE.md](docs/CONFIG-CASCADE.md)

---

## Resources

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
