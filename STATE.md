# Project State

**Project:** hs-rustlib
**Purpose:** Shared Rust utility library for HyperSec applications (port of hs-lib/hs-golib)
**Status:** v1.0.8 published, license module added

---

## Current Session (2026-01-19)

### In Progress

None - license module implementation complete, ready for commit.

### Accomplished

- **License Module Implementation** - Full encrypted license system with anti-tampering:
  - `src/license/mod.rs` - Main module with file/URL/default loading, global singleton API
  - `src/license/crypto.rs` - AES-256-GCM encryption/decryption with SHA-256 key derivation
  - `src/license/defaults.rs` - Obfuscated compile-time defaults using `obfstr`
  - `src/license/types.rs` - `LicenseSettings` struct with feature flags, expiration, limits
  - `src/license/error.rs` - Error types for license operations
  - `src/license/integrity.rs` - Ed25519 signature verification, hash integrity checks, debugger detection

- **Features Added**:
  - `license` - Core license functionality (aes-gcm, obfstr, ed25519-dalek, sha2, base64, rand)
  - `license-http` - HTTP license fetching (adds reqwest blocking)

- **Earlier in session**:
  - Published v1.0.8 to Artifactory (fixed package excludes)
  - Created release workflow for cargo publish
  - Transferred repo from catinspace-au to hypersec-io

### Key Files Modified

- `Cargo.toml` - Added license feature and dependencies (aes-gcm, obfstr, ed25519-dalek, sha2, base64, rand)
- `src/lib.rs` - Added license module export and re-exports
- `src/license/` - NEW: Complete license module (6 files)

### Decisions Made

- **AES-256-GCM for encryption**: Audited crate, authenticated encryption
- **obfstr for string obfuscation**: Compile-time XOR obfuscation, no proc-macro complexity
- **Ed25519 for signatures**: Fast, secure, ed25519-dalek is well-maintained
- **SHA-256 for key derivation**: Simple, deterministic key from secret
- **Blocking reqwest for HTTP**: License loading happens at startup, simpler than async
- **Fallback to compiled defaults**: Always have a working license, even if file missing

### Next Steps

1. Commit license module changes
2. Bump version to 1.1.0 (new feature)
3. Push and verify CI passes
4. Update clickhouse-arrow dependency when new version published

### Blockers/Issues

- clickhouse-arrow fork has new commits (Variant/Dynamic/Nested/BFloat16 support) but not yet published to Artifactory

### Dead Ends & Hypotheses

- obfstr! macro returns a temporary - must convert to String immediately before using
- Ed25519 SPKI format has 12-byte header before 32-byte key

### Git State

- **Branch:** main
- **Upstream:** up to date with origin/main
- **Uncommitted:** Cargo.toml, src/lib.rs modified
- **Untracked:** src/license/ (new directory)
- **Staged:** none

### Session Context Summary

Implemented comprehensive license module with AES-256-GCM encryption, Ed25519 signature verification, obfuscated compile-time defaults, and anti-tampering measures. The module supports loading encrypted license files from local paths, environment variables, standard locations, or HTTPS URLs, with automatic fallback to compiled defaults. 38 unit tests passing, clippy clean.

---

## Previous Session (2025-01-19)

### Accomplished

- Transferred repo from `catinspace-au/hs-rustlib` to `hypersec-io/hs-rustlib`
- Created `.github/workflows/release.yml` for Artifactory cargo publish
- Fixed multiple CI issues (registry auth, doc tests, clippy, package excludes)
- Published v1.0.7 and v1.0.8 to Artifactory
- Comprehensive dependency audit and migration (serde_yml → serde-yaml-ng, queue-file → yaque, once_cell → LazyLock)

---

## Project Overview

### Architecture

Modular library with feature-gated components. Each module can be enabled/disabled independently via Cargo features.

### Key Components

1. **env** - Environment detection (K8s, Docker, Container, BareMetal)
2. **runtime** - Runtime paths with XDG/container awareness
3. **config** - 7-layer configuration cascade
4. **logger** - Structured logging with JSON/text formats
5. **metrics** - Prometheus metrics with process/container awareness
6. **spool** - Disk-backed async FIFO queue (yaque)
7. **tiered-sink** - Resilient message delivery with disk spillover
8. **transport** - Kafka/Zenoh/Memory transport abstraction
9. **clickhouse-arrow** - ClickHouse client with Arrow protocol
10. **license** - Encrypted license files with anti-tampering (NEW)

### Tech Stack

- **Language:** Rust 1.80+ (MSRV)
- **Config:** figment (0.10)
- **Logging:** tracing + tracing-subscriber (0.3)
- **Metrics:** metrics + metrics-exporter-prometheus
- **Async:** tokio (1.0)
- **Disk Queue:** yaque (0.6)
- **YAML:** serde-yaml-ng (0.10)
- **License Crypto:** aes-gcm, ed25519-dalek, obfstr

---

## Build Configuration

**IMPORTANT:** Use `CARGO_BUILD_JOBS=2` for all cargo commands:

```bash
CARGO_BUILD_JOBS=2 cargo build
CARGO_BUILD_JOBS=2 cargo test
CARGO_BUILD_JOBS=2 cargo clippy
```

---

## Resources

**Documentation:**

- [WBS.md](WBS.md) - Work breakdown structure
- [DESIGN.md](DESIGN.md) - Architecture and API design
- [TODO.md](TODO.md) - Task tracking

**External Resources:**

- [figment docs](https://docs.rs/figment)
- [tracing docs](https://docs.rs/tracing)
- [metrics docs](https://docs.rs/metrics)
- [yaque docs](https://docs.rs/yaque)
- [aes-gcm docs](https://docs.rs/aes-gcm)
- [obfstr docs](https://docs.rs/obfstr)

---

**Last Updated:** 2026-01-19
**Version:** 1.0.8 (published), 1.1.0 (pending with license module)
**Status:** License module complete, ready for release

---

## Notes for AI Assistants

This file maintains session state across conversations. Update this file when:

- Completing significant milestones
- Making architectural decisions
- Identifying blockers or issues
- Planning next steps

Keep this file concise and focused on current project state.

### License Module Production Notes

Before deploying to production:

1. Change the obfuscated key in `src/license/defaults.rs` (`get_decryption_key()`)
2. Replace the Ed25519 public key in `src/license/integrity.rs` (`get_public_key_bytes()`)
3. Generate license files externally using `encrypt_license()` with your secret key
