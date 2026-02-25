# ClickHouse Python Bindings Discussion

**Date:** 2025-01-19
**Status:** Proposal / Discussion

---

## Overview

This document outlines the approach for exposing hyperi-rustlib's ClickHouse client to Python via PyO3, enabling hyperi-pylib to leverage the Rust implementation's performance and type safety.

---

## Why Python Bindings?

### Benefits

1. **Performance**: Native Arrow protocol is faster than HTTP-based Python clients
2. **Type Safety**: Runtime type parsing from ClickHouse schema (SSOT principle)
3. **Consistency**: Same client behaviour across Rust and Python codebases
4. **Arrow Native**: Zero-copy data exchange via PyArrow

### Use Cases

- hyperi-pylib applications needing high-performance ClickHouse access
- Data pipelines mixing Python and Rust components
- Gradual migration from Python to Rust services

---

## Types to Expose

### Primary API (Must Have)

| Rust Type | Python Class | Purpose |
|-----------|--------------|---------|
| `ArrowClickHouseClient` | `ClickHouseClient` | Main client interface |
| `ClickHouseConfig` | `ClickHouseConfig` | Connection configuration |
| `TableSchema` | `TableSchema` | Table metadata |
| `ColumnInfo` | `ColumnInfo` | Column metadata |
| `ParsedType` | `ParsedType` | Type introspection |

### Error Handling

| Rust Type | Python Exception |
|-----------|------------------|
| `ClickHouseError::Connection` | `ClickHouseConnectionError` |
| `ClickHouseError::Query` | `ClickHouseQueryError` |
| `ClickHouseError::Insert` | `ClickHouseInsertError` |
| `ClickHouseError::Schema` | `ClickHouseSchemaError` |
| `ClickHouseError::Arrow` | `ClickHouseArrowError` |

---

## Proposed Python API

### Configuration

```python
from hyperi_rustlib import ClickHouseConfig, ClickHouseClient

# Basic configuration
config = ClickHouseConfig(
    host="localhost:9000",
    database="default"
)

# With authentication
config = ClickHouseConfig(
    host="clickhouse.example.com:9000",
    database="analytics",
    username="user",
    password="secret",
    connect_timeout_ms=5000,
    request_timeout_ms=30000,
)

# Multiple hosts (load balancing)
config = ClickHouseConfig(
    hosts=["ch1:9000", "ch2:9000", "ch3:9000"],
    database="default"
)
```

### Client Usage

```python
import pyarrow as pa
from hyperi_rustlib import ClickHouseClient, ClickHouseConfig

# Create client (async context manager)
async with ClickHouseClient(config) as client:
    # Health check
    await client.health_check()

    # Execute query (returns PyArrow Table)
    table = await client.select("SELECT * FROM events LIMIT 100")
    df = table.to_pandas()  # Convert to pandas if needed

    # Insert data (accepts PyArrow Table or RecordBatch)
    data = pa.table({
        "id": [1, 2, 3],
        "name": ["a", "b", "c"],
        "timestamp": pa.array([...], type=pa.timestamp("us"))
    })
    rows_inserted = await client.insert("events", data)

    # Schema introspection
    schema = await client.fetch_table_schema("events")
    for col in schema.columns:
        print(f"{col.name}: {col.type_name} (nullable={col.is_nullable})")
```

### Synchronous Wrapper

```python
# For sync codebases
from hyperi_rustlib import ClickHouseClient, ClickHouseConfig

client = ClickHouseClient.connect_sync(config)
table = client.select_sync("SELECT * FROM events LIMIT 100")
client.close()
```

### Type Introspection

```python
from hyperi_rustlib import ParsedType

# Parse a ClickHouse type string
parsed = ParsedType.parse("Nullable(Array(String))")
print(parsed.base)           # "Array"
print(parsed.nullable)       # True
print(parsed.array_element)  # ParsedType for String
print(parsed.is_string())    # False (it's an array)
```

---

## Implementation Approach

### Option 1: PyO3 Extension Module (Recommended)

Create a separate crate `hyperi-rustlib-python` that wraps the Rust types:

```text
hyperi-rustlib/
├── Cargo.toml
├── src/           # Core Rust library
└── python/
    ├── Cargo.toml # PyO3 extension
    ├── src/
    │   └── lib.rs # Python bindings
    └── pyproject.toml
```

**Pros:**

- Clean separation of concerns
- Can version Python bindings independently
- Easier to maintain

**Cons:**

- Separate build process
- Need to keep in sync with core library

### Option 2: Feature-Gated Bindings

Add Python bindings directly to hyperi-rustlib behind a feature flag:

```toml
[features]
python = ["dep:pyo3"]
```

**Pros:**

- Single codebase
- Always in sync

**Cons:**

- Increases core library complexity
- PyO3 dependencies even when not needed

### Recommendation

**Option 1** - Separate crate in `python/` subdirectory, published as `hyperi-rustlib-python` to PyPI.

---

## Async Runtime Handling

The ClickHouse client is async-first. Options for Python:

### Option A: pyo3-asyncio (Recommended)

```rust
use pyo3_asyncio::tokio::future_into_py;

#[pymethods]
impl PyClickHouseClient {
    fn select<'py>(&self, py: Python<'py>, sql: &str) -> PyResult<&'py PyAny> {
        let client = self.inner.clone();
        let sql = sql.to_string();
        future_into_py(py, async move {
            let batches = client.select(&sql).await?;
            // Convert to PyArrow
            Ok(batches_to_pyarrow(batches)?)
        })
    }
}
```

**Pros:**

- Native async/await in Python
- Non-blocking

**Cons:**

- Requires Python 3.7+
- More complex error handling

### Option B: Sync Wrappers

```rust
#[pymethods]
impl PyClickHouseClient {
    fn select_sync(&self, sql: &str) -> PyResult<PyObject> {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(async {
            let batches = self.inner.select(sql).await?;
            Ok(batches_to_pyarrow(batches)?)
        })
    }
}
```

**Pros:**

- Simpler to use
- Works with sync Python code

**Cons:**

- Blocks the thread
- Can't be used in async Python context

### Recommendation

Provide **both**: async methods as default, with `_sync` suffix variants for convenience.

---

## Arrow Interoperability

### PyArrow Integration

```rust
use arrow::pyarrow::ToPyArrow;
use pyo3::prelude::*;

fn batches_to_pyarrow(py: Python, batches: Vec<RecordBatch>) -> PyResult<PyObject> {
    // Convert RecordBatches to PyArrow Table
    let schema = batches[0].schema();
    let table = arrow::compute::concat_batches(&schema, &batches)?;
    table.to_pyarrow(py)
}

fn pyarrow_to_batch(table: &PyAny) -> PyResult<RecordBatch> {
    // Convert PyArrow Table to RecordBatch
    RecordBatch::from_pyarrow(table)
}
```

### Zero-Copy Data Transfer

Arrow's columnar format enables zero-copy data sharing between Rust and Python when memory is properly aligned. This is a key performance benefit.

---

## Dependencies

### Rust Side

```toml
[dependencies]
pyo3 = { version = "0.22", features = ["extension-module"] }
pyo3-asyncio = { version = "0.21", features = ["tokio-runtime"] }
arrow = { version = "53", features = ["pyarrow"] }
```

### Python Side

```toml
[project]
dependencies = [
    "pyarrow>=14.0",
]
```

---

## Build and Distribution

### Build Process

```bash
# Development
cd python/
maturin develop

# Release
maturin build --release
```

### Distribution

1. **PyPI**: Publish wheels for Linux/macOS/Windows
2. **Artifactory**: Internal distribution alongside hyperi-pylib

### Platform Support

- Linux x86_64 (primary)
- macOS arm64 (development)
- Windows x86_64 (if needed)

---

## Integration with hyperi-pylib

### Option A: Separate Package

```python
# hyperi-pylib uses hyperi-rustlib-python as optional dependency
# pyproject.toml
[project.optional-dependencies]
clickhouse = ["hyperi-rustlib-python>=0.1"]
```

### Option B: Vendored in hyperi-pylib

Include pre-built wheels in hyperi-pylib's distribution.

### Recommendation

**Option A** - Separate package, optional dependency. Allows independent versioning and reduces hyperi-pylib's complexity.

---

## Open Questions

1. **Naming**: `hyperi-rustlib-python` vs `hs-clickhouse` vs `clickhouse-arrow-py`?
2. **Scope**: Just ClickHouse, or expose other hyperi-rustlib modules (config, metrics)?
3. **Async**: Should async be the default, or provide sync-first API?
4. **Error Messages**: How much detail to expose in Python exceptions?

---

## Next Steps

1. [ ] Create `python/` subdirectory with PyO3 skeleton
2. [ ] Implement basic `ClickHouseConfig` and `ClickHouseClient` wrappers
3. [ ] Add PyArrow integration for data transfer
4. [ ] Write Python tests
5. [ ] Set up maturin build pipeline
6. [ ] Publish to internal Artifactory
7. [ ] Integrate with hyperi-pylib as optional dependency

---

## References

- [PyO3 User Guide](https://pyo3.rs/)
- [pyo3-asyncio](https://github.com/awestlake87/pyo3-asyncio)
- [Arrow PyArrow Integration](https://docs.rs/arrow/latest/arrow/pyarrow/index.html)
- [Maturin](https://www.maturin.rs/)
