# Spool

Disk-backed async FIFO queue built on
[yaque](https://crates.io/crates/yaque). Crash-safe, persistent, with
optional zstd compression and bounded size. Used by `TieredSink` as
the disk-spillover tier; available as a standalone primitive for any
pipeline that needs durable store-and-forward.

For most workloads you want `TieredSink` (transport + spool + circuit
breaker + drain). Reach for `Spool` directly only when you're building
something `TieredSink` doesn't cover — e.g. an out-of-band replay
buffer, a checkpoint store, or a custom drainer.

---

## What it gives you

| Property | Notes |
|----------|-------|
| FIFO order | Strict — yaque is a single-producer / single-consumer log |
| Persistent | Survives restarts; segment files in the queue directory |
| Crash-safe | Receiver position persisted in `recv-metadata`; commit-then-advance semantics |
| Async-native | Built on yaque's async receiver — `recv()` awaits when empty |
| Optional compression | zstd at construction-time level (1–22, clamped) |
| Bounded | `max_items` (count) and `max_size_bytes` (directory size) |

---

## Storage layout

Inside the configured `path`:

```
spool.queue/
|-- 0.q                # segment file — [4-byte Hamming header][payload] ...
|-- 1.q
|-- ...
|-- recv-metadata      # 16 bytes: (segment u64 BE, position u64 BE)
`-- send-metadata
```

Segments roll over as they fill. The receiver position is two
big-endian u64s pointing into the segment file at the next byte to
read. On `recv` / `pop_front` the guard returned by yaque is
explicitly `commit()`'d — the position advances; on drop without
commit, the read rolls back and the item stays in the queue (this is
how `peek` works).

`Spool::open` rescans the directory on construction to recover the
item count after a restart — yaque doesn't expose a length API.

---

## Durability and recovery

- yaque writes durable per-message — a successful `push().await`
  means the bytes are in the segment file. (Whether the OS has
  fsync'd to disk depends on yaque's internal policy; for absolute
  durability the caller should `fsync` the directory out-of-band or
  use a journalled filesystem.)
- On restart, the receiver position is read from `recv-metadata` and
  scanning starts from there. Items consumed before the crash stay
  consumed; items not yet committed reappear.
- `clear()` walks the queue and commits every item — empties without
  touching the filesystem directly.

---

## Compression

When `compress = true`, every payload is zstd-compressed before
`sender.send` and decompressed inside `recv` / `pop_front` /
`pop_front_async`. Compression level is config-controlled (default 3
— fast). Use higher levels (10+) for archival queues; default for
hot-path spool.

The choice is a one-shot at construction — there's no per-message
override.

---

## Bounded size

| Limit | Behaviour on exceeded |
|-------|----------------------|
| `max_items: Some(n)` | `push` returns `Err(MaxItemsReached { max })` |
| `max_size_bytes: Some(b)` | `push` returns `Err(MaxSizeReached { max_bytes })` |

Both checks happen pre-write. The size check uses `file_size()` which
sums every regular file in the queue directory — exact for fresh
opens, slightly stale between segment rolls. Callers should treat
these as soft bounds; downstream pressure (DLQ, drop, throttle) is
the right response when they fire.

There's no built-in "drop oldest" mode — yaque is append-only and
removing oldest would require rewriting segment files. If you need
ring-buffer semantics, build it on top by combining `pop_front` (oldest)
with `push` (newest) under your own lock.

---

## When to use `TieredSink` instead

Use `TieredSink` if the answer to all of these is yes:

- The primary destination is a network sink (Kafka, gRPC, HTTP, S3).
- You want automatic spillover on transport failure.
- You want a circuit breaker and background drain back to primary.

Use `Spool` directly when:

- You're not retrying against an upstream sink — the spool **is** the
  destination (replay buffer, audit log, deferred-work queue).
- You need to peek or clear the queue, which `TieredSink` doesn't
  expose.
- You're building a custom drainer with non-standard semantics.

---

## Configuration

```yaml
spool:
  path: /var/spool/dfe/replay
  compress: true
  compression_level: 3
  max_items: 1000000
  max_size_bytes: 10737418240   # 10 GiB
```

Builder methods on `SpoolConfig` cover the common shapes —
`SpoolConfig::new(path)`, `SpoolConfig::with_compression(path)`,
`.compress(bool)`, `.compression_level(i)`, `.max_items(n)`,
`.max_size_bytes(b)`.

---

## Usage

```rust
use hyperi_rustlib::spool::{Spool, SpoolConfig};

let cfg = SpoolConfig::new("/var/spool/myapp")
    .compress(true)
    .max_items(1_000_000);

let mut spool = Spool::open(cfg).await?;

spool.push(b"event-1").await?;
spool.push(b"event-2").await?;

while let Some(data) = spool.pop_front().await? {
    process(&data).await?;
}
```

`recv()` is the async-await variant — it blocks when the queue is
empty (useful for a consumer task that should idle until work
arrives). `pop_front` is the try-style alternative that returns
`Ok(None)` instead of blocking.

---

## API surface

| Item | Purpose |
|------|---------|
| `Spool::open(config)` | Open or create the queue; recovers item count from disk |
| `Spool::create(path)` | Convenience for `open(SpoolConfig::new(path))` |
| `Spool::create_compressed(path)` | Convenience for `open(SpoolConfig::with_compression(path))` |
| `push(data).await` | Append; checks `max_items` / `max_size_bytes` |
| `recv().await` | Wait for next item; commits on success |
| `pop_front().await` | Non-blocking pop; `Ok(None)` if empty |
| `peek().await` | Read without removing (guard rollback) |
| `pop().await` | Remove front without returning value |
| `len() / is_empty()` | Item count (tracked internally) |
| `file_size()` | Directory size in bytes |
| `clear()` | Drain every item; resets count |
| `config() -> &SpoolConfig` | Active config |
| `SpoolError` | `Open / Queue / Compression / Decompression / Io / MaxItemsReached / MaxSizeReached` |

---

## Source

- [`../../src/spool/mod.rs`](../../src/spool/mod.rs)
- [`../../src/spool/queue.rs`](../../src/spool/queue.rs) — `Spool`, yaque wrapper, item count recovery
- [`../../src/spool/config.rs`](../../src/spool/config.rs)
- [`../../src/spool/error.rs`](../../src/spool/error.rs)

---

## Related

- [TIERED-SINK.md](TIERED-SINK.md) — the primary consumer; handles the retry / circuit / drain semantics on top
- [DLQ.md](DLQ.md) — where to send messages when spool is full
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) — `spool` (pulls `zstd`)
- [../ARCHITECTURE.md](../ARCHITECTURE.md)
