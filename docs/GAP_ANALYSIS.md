# Gap Analysis: hyperi-rustlib vs hyperi-pylib

**Date:** 2025-01-19
**hyperi-rustlib Version:** 0.3.0
**hyperi-pylib Reference:** Latest main branch

---

## Summary

| Metric | hyperi-pylib | hyperi-rustlib |
|--------|----------|-----------|
| Total LOC | ~15,400 | ~6,163 |
| Major Modules | 11 | 7 |
| Status | Production-ready, enterprise features | MVP complete, core modules |

---

## Module Comparison

| Module | hyperi-pylib | hyperi-rustlib | Gap Notes |
|--------|----------|-----------|-----------|
| **env** | ✅ K8s/Docker/Container/BareMetal detection | ✅ Same detection methods | ✓ Parity |
| **runtime** | ✅ XDG + container-aware paths | ✅ XDG + container-aware paths | ✓ Parity |
| **config** | ✅ 7-layer cascade (Dynaconf) + PostgreSQL loader | ✅ 7-layer cascade (Figment) | ✓ Core parity; pylib has PostgreSQL config layer |
| **logger** | ✅ Loguru + masking + rate limiting | ✅ Tracing + masking | ✓ Core parity; pylib has rate limiting, emoji support |
| **metrics** | ✅ Prometheus + OpenTelemetry + FastAPI middleware | ✅ Prometheus only | ✓ Core parity; pylib has OpenTelemetry, middleware |
| **transport** | ❌ Not implemented | ✅ Kafka/Zenoh/Memory abstraction | ⚡ **rustlib advantage** |
| **clickhouse** | ❌ Not implemented | ✅ Arrow protocol client | ⚡ **rustlib advantage** |
| **database** | ✅ URL builders (Postgres/MySQL/MongoDB/Redis) | ❌ Not implemented | ⚠️ Gap |
| **http** | ✅ Sync/async with retries (Stamina) | ❌ Feature planned | ⚠️ Gap |
| **cache** | ✅ Disk + PostgreSQL backends | ❌ Feature planned | ⚠️ Gap |
| **kafka** | ✅ Full ecosystem (~3,500 LOC) | ❌ Not standalone | ⚠️ Major gap |
| **cli** | ✅ Typer framework + helpers | ❌ Not implemented | ⚠️ Gap |
| **anonymizer** | ✅ PII detection (Presidio) | ❌ Not implemented | ⚠️ Gap |
| **harness** | ✅ Timeout monitors, registry utils | ❌ Not implemented | ⚠️ Gap |

---

## Features hyperi-rustlib Has That hyperi-pylib Doesn't

### 1. Transport Abstraction Layer (~1,000 LOC)

Multi-transport abstraction supporting Kafka, Zenoh, and in-memory transports with:

- Stateful format detection with auto-locking
- Payload format handling (JSON/MsgPack)
- Generic message interface with commit tokens
- Lower-level control for event streaming pipelines

### 2. ClickHouse Client (~500 LOC)

Native Arrow protocol client with:

- Schema introspection and type parsing
- Connection pooling
- Type-safe queries
- Arrow-native for high performance

---

## Features hyperi-pylib Has That hyperi-rustlib Doesn't

### P0 - Critical for Enterprise Use

#### Kafka Client (~3,500 LOC)

Full Kafka ecosystem that would require significant effort to port:

- Sync/async producer, consumer, admin clients
- Health monitoring with consumer group lag tracking
- Schema analysis for JSON messages
- Sampling utilities (reservoir, time-bounded, partition)
- Metrics collection with callback integration

**Note:** hyperi-rustlib has Kafka via transport abstraction, but lacks standalone client.

### P1 - High Utility

#### Database Module (~200 LOC)

Connection URL builders:

- PostgreSQL: `POSTGRES_HOST`, `POSTGRES_PORT`, `POSTGRES_USER`, `POSTGRES_PASSWORD`, `POSTGRES_DB`
- MySQL: Similar ENV var patterns
- MongoDB: Connection string builder
- Redis: `REDIS_HOST`, `REDIS_PORT`, `REDIS_PASSWORD`

#### HTTP Client (~200 LOC)

Production-ready HTTP client:

- Sync and async variants
- Automatic retries with exponential backoff (Stamina)
- Configurable timeouts (30s default)
- Metrics auto-detection

### P2 - Supporting Features

#### Cache Module (~300 LOC)

Multi-backend caching:

- Disk cache (Cashews/SQLite)
- PostgreSQL cache for distributed deployments
- `@cached` decorator with TTL support
- Async-first design

#### CLI Framework (~300 LOC)

Typer-based CLI utilities:

- Reusable options (VERBOSE_OPTION, DRY_RUN_OPTION)
- Version handling
- Output formatters (tables, progress bars)
- CliRunner for testing

### P3 - Nice to Have

#### Anonymizer/PII Detection (~500 LOC)

Microsoft Presidio integration:

- Multiple presets (minimal, standard, compliance)
- Strategies: mask, redact, hash, encrypt
- Text/JSON/dict support
- Streaming for large datasets

#### Harness/Testing Utilities (~300 LOC)

CI/CD helpers:

- Smart timeout monitoring for functions
- Container registry login utilities
- Docker Hub rate limit checking

---

## Implementation Priority for hyperi-rustlib

### Phase 1 - Core Enterprise (Recommended)

1. **Database module** (~200 LOC)
   - URL builders for PostgreSQL, Redis
   - ENV var parsing with standard prefixes
   - Minimal dependencies

2. **HTTP client** (~300 LOC)
   - Wrap reqwest with retry middleware
   - Already in Cargo.toml as optional feature
   - Foundation for many applications

### Phase 2 - Enhanced Features

3. **Cache module** (~300 LOC)
   - Disk-based cache with SQLite
   - Optional Redis support

4. **CLI framework** (~200 LOC)
   - Wrap Clap with reusable helpers
   - Output formatters

### Phase 3 - Advanced (As Needed)

5. **Kafka standalone client** - Only if transport layer insufficient
6. **Anonymizer** - Depends on external library availability
7. **Harness utilities** - Lower priority

---

## Architectural Differences

| Aspect | hyperi-pylib | hyperi-rustlib |
|--------|----------|-----------|
| Philosophy | "Zero-config batteries included" | "Minimal core + extensibility" |
| Async | Full async/await throughout | Async-ready, primarily sync core |
| Dependencies | ~50+ transitive | ~15-20 focused |
| Type Safety | Runtime | Compile-time |
| Memory | Garbage collected | Zero-cost abstractions |

---

## Conclusion

**hyperi-rustlib** is well-positioned as a core infrastructure library with type safety and performance advantages. The transport and ClickHouse modules are unique capabilities.

**For full enterprise parity**, prioritise:

1. Database URL builders (high utility, low effort)
2. HTTP client with retries (already partially scaffolded)
3. Evaluate need for standalone Kafka vs using transport layer

**Keep unique in hyperi-rustlib**:

- Transport abstraction (better in Rust)
- ClickHouse Arrow client (Rust-specific advantage)
