# ClickHouse Integration Architecture

**Status:** Archived
**Date:** 2026-01-20
**Reason:** Moved from hyperi-rustlib to application layer

---

## Overview

This document captures the ClickHouse client wrapper and type parsing system that was originally implemented in `hyperi-rustlib`. After architectural review, this code was determined to be application-level logic rather than infrastructure, and should live in applications that perform data ingestion (e.g., logjam, pipeline-agent).

---

## Architecture Principles

### ClickHouse as Single Source of Truth (SSOT)

The core principle: **ClickHouse schema is the source of truth for types, not compiled code.**

- Types are queried from `system.columns` at runtime
- Type mappings are extensible without code changes
- New ClickHouse types require config changes, not code changes
- Forward compatibility: unknown types fall back to String

### Why Runtime Type Parsing?

Traditional approaches compile type mappings into the code. This creates problems:

1. **Version coupling**: Adding new ClickHouse types requires code releases
2. **Schema drift**: Compiled types can disagree with actual schema
3. **Maintenance burden**: Every ClickHouse upgrade potentially needs code changes

Runtime parsing solves these:

1. **Dynamic adaptation**: Read schema, adapt coercion
2. **Forward compatible**: Unknown types → String (safe fallback)
3. **Config-driven**: Type behavior can be tuned without code changes

---

## Type Parsing System

### ParsedType Structure

The `ParsedType` struct represents a parsed ClickHouse type:

```rust
pub struct ParsedType {
    /// Original type string from ClickHouse (e.g., "LowCardinality(Nullable(String))")
    pub raw: String,

    /// Base type name (e.g., "String", "Int64", "DateTime64")
    pub base: String,

    /// Whether wrapped in Nullable()
    pub nullable: bool,

    /// Whether wrapped in LowCardinality()
    pub low_cardinality: bool,

    /// For Array types, the element type
    pub array_element: Option<Box<ParsedType>>,

    /// For Map types, (key_type, value_type)
    pub map_types: Option<(Box<ParsedType>, Box<ParsedType>>),

    /// Precision for DateTime64, Decimal, etc.
    pub precision: Option<u8>,

    /// Scale for Decimal types
    pub scale: Option<u8>,

    /// Timezone for DateTime64
    pub timezone: Option<String>,

    /// Size for FixedString
    pub fixed_size: Option<usize>,
}
```

### Parsing Algorithm

The parser handles nested wrappers recursively:

1. **Unwrap wrappers** in a loop:
   - `Nullable(X)` → set nullable=true, continue with X
   - `LowCardinality(X)` → set low_cardinality=true, continue with X

2. **Handle complex types**:
   - `Array(X)` → parse element type recursively
   - `Map(K, V)` → parse key and value types (handles nested parens)
   - `DateTime64(precision, 'timezone')` → extract precision and timezone
   - `FixedString(N)` → extract size
   - `Decimal(P, S)` or `Decimal64(S)` → extract precision and scale
   - `Enum8(...)` / `Enum16(...)` → capture base, ignore values

3. **Simple types**: Everything else is just the base name

### Coercer Categories

Types are mapped to categories for uniform coercion:

| Category | ClickHouse Types |
|----------|------------------|
| String | String, FixedString |
| Int | Int8, Int16, Int32, Int64, Int128, Int256 |
| UInt | UInt8, UInt16, UInt32, UInt64, UInt128, UInt256 |
| Float | Float32, Float64 |
| Decimal | Decimal, Decimal32, Decimal64, Decimal128, Decimal256 |
| Bool | Bool |
| Date | Date, Date32 |
| DateTime | DateTime |
| DateTime64 | DateTime64 |
| UUID | UUID |
| IPv4 | IPv4 |
| IPv6 | IPv6 |
| Array | Array |
| Map | Map |
| Tuple | Tuple |
| JSON | JSON, Object |
| Variant | Variant (ClickHouse 24.1+) |
| Dynamic | Dynamic (ClickHouse 25.3+) |
| Enum | Enum8, Enum16 |
| Geo | Point, Ring, Polygon, MultiPolygon, LineString, MultiLineString |

Unknown types default to "String" (safe fallback).

---

## Null Handling

### Null String Recognition

Common representations of null in string data:

```rust
pub const NULL_STRINGS: &[&str] = &[
    "null", "NULL", "Null", "None", "nil", "undefined",
    "\\N", "<null>", "NA", "N/A", "n/a", "NaN", ""
];
```

### Default Values by Category

When replacing nulls (ClickHouse best practice to avoid Nullable overhead):

| Category | Default Value |
|----------|---------------|
| String | `""` |
| Int, UInt | `"0"` |
| Float, Decimal | `"0.0"` |
| Bool | `"false"` |
| Date | `"1970-01-01"` |
| DateTime, DateTime64 | `"1970-01-01T00:00:00Z"` |
| UUID | `"00000000-0000-0000-0000-000000000000"` |
| IPv4 | `"0.0.0.0"` |
| IPv6 | `"::"` |
| Array | `"[]"` |
| Map, JSON | `"{}"` |
| Enum | `""` |
| Geo | `"(0, 0)"` |

---

## Schema Introspection

### ColumnInfo Structure

```rust
pub struct ColumnInfo {
    pub name: String,
    pub type_name: String,           // Raw type from ClickHouse
    pub parsed_type: ParsedType,     // Parsed representation
    pub position: u64,               // 1-based column position
    pub default_kind: String,        // DEFAULT, MATERIALIZED, ALIAS, EPHEMERAL
    pub default_expression: String,
    pub comment: String,             // May contain metadata directives
    pub is_in_primary_key: bool,
    pub is_in_sorting_key: bool,
}
```

### TableSchema Structure

```rust
pub struct TableSchema {
    pub database: String,
    pub table: String,
    pub columns: Vec<ColumnInfo>,
    pub comment: String,             // May contain directives like logjson=force
}
```

---

## Client Wrapper

### Configuration

```rust
pub struct ClickHouseConfig {
    /// List of host:port addresses (port defaults to 9000)
    pub hosts: Vec<String>,

    /// Database name
    pub database: String,

    /// Authentication
    pub username: String,
    pub password: String,

    /// Timeouts
    pub connect_timeout_ms: u64,     // Default: 5000
    pub request_timeout_ms: u64,     // Default: 30000
}
```

### Client API

```rust
impl ArrowClickHouseClient {
    /// Create client from config
    pub async fn new(config: &ClickHouseConfig) -> Result<Self>;

    /// Insert RecordBatch into table
    pub async fn insert(&self, table: &str, batch: RecordBatch) -> Result<usize>;

    /// Insert multiple RecordBatches
    pub async fn insert_many(&self, table: &str, batches: Vec<RecordBatch>) -> Result<usize>;

    /// Execute SELECT, return RecordBatches
    pub async fn select(&self, sql: &str) -> Result<Vec<RecordBatch>>;

    /// Execute DDL/schema queries
    pub async fn query(&self, sql: &str) -> Result<()>;

    /// Fetch Arrow schema for table
    pub async fn fetch_schema(&self, table: &str) -> Result<SchemaRef>;

    /// Fetch full TableSchema with parsed types
    pub async fn fetch_table_schema(&self, table: &str) -> Result<TableSchema>;

    /// Health check
    pub async fn health_check(&self) -> Result<()>;

    /// Check if table exists
    pub async fn table_exists(&self, table: &str) -> Result<bool>;

    /// List all tables in database
    pub async fn list_tables(&self) -> Result<Vec<String>>;
}
```

### Error Types

```rust
pub enum ClickHouseError {
    Connection(String),
    Query(String),
    Insert(String),
    Schema(String),
    Arrow(String),
}
```

---

## Arrow Type Mapping

Arrow DataType to ClickHouse type name (for schema introspection):

| Arrow Type | ClickHouse Type |
|------------|-----------------|
| Int8/16/32/64 | Int8/16/32/64 |
| UInt8/16/32/64 | UInt8/16/32/64 |
| Float32/64 | Float32/64 |
| Boolean | Bool |
| Utf8, LargeUtf8 | String |
| Binary, LargeBinary | String |
| Date32, Date64 | Date |
| FixedSizeBinary(16) | UUID |
| FixedSizeBinary(4) | IPv4 |
| FixedSizeBinary(n) | FixedString(n) |
| List(inner) | Array(inner) |
| Timestamp(_,_) | DateTime64(3) |
| Time32(_) | DateTime |
| Time64(_) | DateTime64(6) |

---

## Why This Was Removed from hyperi-rustlib

After architectural review, this code was identified as **application logic**, not **infrastructure**:

1. **Type coercion is business logic**: How to handle nulls, what defaults to use - these are application decisions, not library concerns.

2. **SSOT principle already exists**: The `clickhouse-arrow` crate provides schema via Arrow. The additional parsing is only needed for specific ingestion use cases.

3. **Not shared across all apps**: Only data ingestion applications (logjam, pipeline-agent) need this. Other apps just use `clickhouse-arrow` directly.

4. **Maintenance burden**: Keeping this in hyperi-rustlib creates a sync point that adds friction without broad benefit.

### Recommendation

For applications needing type parsing:

1. **Use `clickhouse-arrow` directly** for basic ClickHouse operations
2. **Copy the type parsing code** to your application if needed
3. **Keep coercion logic local** to the ingestion pipeline

The `clickhouse-arrow` crate (our fork at hyperi-io) handles:

- Native Arrow protocol
- Connection management
- Schema introspection (Arrow format)
- Efficient batch insert/query

Applications add their own:

- Type parsing for coercion
- Null handling policies
- Default value strategies

---

## References

- [clickhouse-arrow crate](https://github.com/hyperi-io/clickhouse-arrow)
- [ClickHouse Data Types](https://clickhouse.com/docs/en/sql-reference/data-types)
- [Arrow Rust Documentation](https://docs.rs/arrow)
- [CLICKHOUSE_PYTHON_BINDINGS.md](CLICKHOUSE_PYTHON_BINDINGS.md) - Python binding proposal (deprecated)
