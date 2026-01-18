# Project State

**Project:** hs-rustlib
**Purpose:** Shared Rust utility library for HyperSec applications (port of hs-lib/hs-golib)
**Status:** MVP Complete - v0.4.0

---

## Current Session (2025-01-19)

### In Progress

None - dependency audit and migration complete.

### Accomplished

- Comprehensive dependency audit and migration:
  - **serde_yml → serde-yaml-ng**: Security fix (serde_yml has segfault issues, archived)
  - **queue-file → yaque**: Async-native, actively maintained disk queue
  - **once_cell → std::sync::LazyLock**: Using stdlib (MSRV 1.80)
- Updated spool module to use yaque's async API
- Updated tiered_sink module to use yaque with proper borrow handling
- Added new feature flags:
  - `otel`, `otel-metrics`, `otel-tracing` - OpenTelemetry support
  - `resilience` - tower-resilience for circuit breakers
- Bumped version to 0.4.0, MSRV to 1.80
- All tests passing (78 tests total)
- Clippy clean

### Key Files Modified

- `Cargo.toml` - Dependency updates, new features, MSRV bump
- `src/spool/queue.rs` - Migrated from queue-file to yaque (async API)
- `src/spool/error.rs` - Removed queue_file error type
- `src/spool/mod.rs` - Updated documentation
- `src/tiered_sink/tiered.rs` - Migrated to yaque with Arc<Mutex<Receiver>>
- `src/tiered_sink/drainer.rs` - Updated drain loop for yaque's RecvGuard semantics
- `tests/metrics_integration.rs` - Replaced once_cell with LazyLock

### Decisions Made

- **yaque over queue-file**: queue-file unmaintained since March 2023; yaque is async-native
- **serde-yaml-ng over serde_yml**: serde_yml has security issues and is archived
- **std::sync::LazyLock over once_cell**: Stdlib solution available in Rust 1.80+
- **Keep async-trait for Sink trait**: Public API stability, native async traits can wait

### Next Steps

1. Consider adding tower-resilience based circuit breaker as alternative to custom
2. Add OpenTelemetry integration tests
3. Document new features in README

### Blockers/Issues

None.

### Dead Ends & Hypotheses

- yaque's RecvGuard borrows the Receiver, so all operations (decompress, send, commit) must happen within the lock scope
- yaque doesn't have a built-in `try_clear`, so clear() is implemented by consuming all items

### Git State

- **Branch:** main
- **Uncommitted:** Multiple files (dependency migration)
- **Staged:** none

### Session Context Summary

Performed comprehensive dependency audit and migrated from deprecated/unmaintained libraries to modern alternatives. Migrated spool and tiered_sink modules from synchronous queue-file to async-native yaque. Replaced once_cell with std::sync::LazyLock. Added OpenTelemetry and tower-resilience as optional features. All 78 tests passing, clippy clean.

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

### Tech Stack

- **Language:** Rust 1.80+ (MSRV)
- **Config:** figment (0.10)
- **Logging:** tracing + tracing-subscriber (0.3)
- **Metrics:** metrics + metrics-exporter-prometheus
- **Async:** tokio (1.0)
- **Disk Queue:** yaque (0.6)
- **YAML:** serde-yaml-ng (0.10)

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

---

**Last Updated:** 2025-01-19
**Version:** 0.4.0
**Status:** MVP Complete

---

## Notes for AI Assistants

This file maintains session state across conversations. Update this file when:

- Completing significant milestones
- Making architectural decisions
- Identifying blockers or issues
- Planning next steps

Keep this file concise and focused on current project state.
