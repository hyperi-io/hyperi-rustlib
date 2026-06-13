# Cache

A moka TinyLFU async cache, source-keyed and JSON-serialised. Use it
for values that are expensive to compute but cheap to re-derive -- DNS
resolution results, parsed expressions, compiled regexes, fetched
secrets, schema lookups. Don't use it as session state, response
cache, or shared-cluster cache.

TinyLFU's eviction outperforms LRU on the access patterns these
workloads produce (long-tail of warm keys plus a hot working set).
moka handles concurrency lock-free on the hot path.

---

## Usage

```rust
use hyperi_rustlib::cache::{Cache, CacheConfig};

let cache = Cache::from_cascade();

// Source identifies the namespace; key is opaque within that namespace.
let user: Option<User> = cache.get("user-profile", "alice").await;

cache.set("user-profile", "alice", user_struct).await;

cache.invalidate("user-profile", "alice").await;
cache.invalidate_source("user-profile").await;   // drop everything in the source
```

Source-keyed lookups let you invalidate a whole subsystem's cache in
one call (e.g. clear all secrets on rotation, or all schema entries on
schema change).

Values are JSON-serialised on insert and deserialised on read. The
penalty is one `serde` round-trip per access -- fine for the workloads
above. For raw `Vec<u8>` access, use `moka` directly.

---

## What it isn't

| Use case | Use instead |
|----------|-------------|
| HTTP response cache | A real cache (CDN, proxy) -- those handle revalidation, vary headers, etc. |
| Distributed cache across pods | Redis (see [../transport/BACKENDS.md](../transport/BACKENDS.md)) |
| Session store | Postgres or Redis |
| Read-through DB cache | Build it explicitly with `get_or_insert_with` semantics; the basic API here is just get/set |
| Cross-process state | This is in-process only |

---

## Config shape

```yaml
cache:
  max_capacity: 10000          # max entry count
  time_to_live: 300s           # expire-after-write
  time_to_idle: 60s            # expire-after-last-access (optional)
```

Choose `max_capacity` based on expected working set, not theoretical
maximum -- TinyLFU is good but not magic, and going past the working
set just wastes memory.

---

## API surface

| Item | Purpose |
|------|---------|
| `Cache::new(config)` | Build from explicit config |
| `Cache::from_cascade()` | Build from the `cache` config section |
| `.get::<T>(source, key) -> Option<T>` | Fetch and deserialise |
| `.set::<T>(source, key, value)` | Serialise and insert |
| `.invalidate(source, key)` | Drop one entry |
| `.invalidate_source(source)` | Drop every entry in a source namespace |
| `.entry_count() -> u64` | Current size (approximate under concurrency) |
| `.config() -> &CacheConfig` | Read back the effective config |

---

## When TinyLFU loses

TinyLFU beats LRU on most workloads but loses on:

- Strictly sequential scans (LRU degrades gracefully, TinyLFU has to
  re-warm)
- Workloads with no temporal locality (cache is the wrong tool --
  consider a bloom filter or precomputed table)

For those cases, the workload shouldn't be in a cache at all.

---

## Related

- [SECRETS.md](SECRETS.md) -- secrets manager has its own cache with TTL
- [../core-pillars/CONFIG.md](../core-pillars/CONFIG.md) -- cascade
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) -- `cache`
- Source: [../../src/cache/](../../src/cache/)
