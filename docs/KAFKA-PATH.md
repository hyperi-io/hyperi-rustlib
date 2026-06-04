# Kafka path

The Kafka data path is sized around ONE idea: a byte-budget envelope with a
time bound, so a single config takes a stage from "SME on a laptop" to
hyperscale without re-tuning. This doc covers the three batch sizes, the
sizing profile, the librdkafka property names the code actually uses (several
differ from the Java client), the AIMD loop, wire compression, the raw escape
hatch, and the partition-limited diagnostic.

Code: `src/transport/kafka/config.rs` (sizing surface) and
`src/transport/kafka/mod.rs` (transport + diagnostic). See
[SELF-REGULATION.md](SELF-REGULATION.md) and
[BACKPRESSURE.md](BACKPRESSURE.md) for the governor and brake that sit over
this path.

---

## The three batch sizes

There are THREE distinct batch sizes on the Kafka path, and conflating them
is the usual tuning mistake. Each governs a different hop.

| # | Batch | Governs | Sized by |
|---|---|---|---|
| 1 | **GET** (consumer fetch) | How many bytes the broker hands the consumer per Fetch | `fetch.min.bytes` + `fetch.wait.max.ms` + `fetch.max.bytes` + `max.partition.fetch.bytes` |
| 2 | **PROCESS** (WorkBatch) | How much in-flight data the stage holds while processing | The AIMD byte budget (`src/governor/budget.rs`) -- the self-regulation lever |
| 3 | **SEND** (producer) | How many bytes the producer accumulates per MessageSet | `batch.size` + `linger.ms` |

- **GET** is broker-side accumulation: the broker waits up to
  `fetch.wait.max.ms` to fill `fetch.min.bytes` before responding. Bigger
  GET batches mean fewer round-trips and higher throughput, at the cost of
  latency when traffic is thin.
- **PROCESS** is the only one that self-regulates. It is the byte budget the
  governed driver uses to size sub-blocks (see
  [BACKPRESSURE.md](BACKPRESSURE.md)). GET and SEND are static librdkafka
  knobs; PROCESS moves with pressure.
- **SEND** is producer-side accumulation, mirror of GET. `linger.ms` is the
  time bound; `batch.size` is the byte bound.

The poll-safety cap (`max_poll_records`) is a fourth, count-based limit. It
is NOT a librdkafka property -- there is no broker knob for it. It is a
client-side cap enforced by passing it as the `max` argument to
`recv()`, bounding how many records the WorkBatch layer receives per poll so
a tiny-record flood cannot blow the count even within the byte budget.

---

## Byte budget + time bound = one config, SME to hyperscale

The sizing knobs default to GENEROUS ceilings with a TIME bound. The
generous ceiling means a busy topic fills large, efficient batches; the time
bound (`fetch.wait.max.ms` on GET, `linger.ms` on SEND) means a quiet topic
does NOT stall waiting to fill that ceiling -- it returns whatever it has when
the timer fires. The same `profile: throughput` config that batches a PB/day
firehose into 1 MiB fetches also serves a trickle of events at low latency,
because the time bound caps the wait either way. One config, no re-tuning as
volume grows.

On a small or memory-tight pod the generous `throughput` start budget can
overshoot in the cold-start window (the first block before the governor's
AIMD loop / memory-hard override reacts). Set a lower `self_regulation`
start budget or use the `balanced` / `low_latency` profile -- see the
small-pod guidance in [SELF-REGULATION.md](SELF-REGULATION.md).

---

## Profile + GET/SEND tuning table

`SelfRegulationProfile` (in `src/transport/kafka/config.rs`) sets opinionated
defaults for the byte envelope and latency. An explicit per-knob value wins
over the profile default; the raw librdkafka escape hatch wins over
everything.

```yaml
kafka:
  sizing:
    profile: throughput      # throughput (default) | balanced | low_latency
    consumer:
      fetch_min_bytes: 2097152      # 2 MiB, overrides the profile default
    producer:
      compression_type: zstd        # opt into zstd for storage-bound topics
    consumer_librdkafka:
      fetch.wait.max.ms: "75"       # raw override -- wins over everything
    producer_librdkafka:
      linger.ms: "50"
```

The profile defaults, with the ACTUAL librdkafka property each maps to:

| Profile | GET `fetch.min.bytes` | GET `fetch.wait.max.ms` | GET `max.partition.fetch.bytes` | GET `fetch.max.bytes` | poll cap | SEND `batch.size` | SEND `linger.ms` | SEND codec | SEND `queue.buffering.max.kbytes` |
|---|---|---|---|---|---|---|---|---|---|
| `throughput` (default) | 1 MiB | 50 ms | 10 MiB | 100 MiB | 2000 | 128 KiB | 20 ms | lz4 | 64 MiB |
| `balanced` | 256 KiB | 25 ms | 5 MiB | 50 MiB | 1000 | 64 KiB | 5 ms | lz4 | 32 MiB |
| `low_latency` | 1 byte | 5 ms | 1 MiB | 10 MiB | 500 | 16 KiB | 0 ms | lz4 | 16 MiB |

### librdkafka property names -- mind the differences

librdkafka is NOT the Java Kafka client. Several property names differ, and
the code uses the librdkafka ones. Getting these wrong silently no-ops
(librdkafka ignores unknown keys).

| Intent | librdkafka (used here) | NOT (Java client) |
|---|---|---|
| Max broker wait to fill a fetch | `fetch.wait.max.ms` | `fetch.max.wait.ms` |
| Uniform sticky for null-key messages (KIP-794) | `sticky.partitioning.linger.ms` | `partitioner.ignore.keys` |
| Total producer queue byte budget | `queue.buffering.max.kbytes` (in KiB) | `buffer.memory` (in bytes) |
| Accumulation delay before send | `linger.ms` (alias `queue.buffering.max.ms`) | -- |
| Per-MessageSet byte ceiling | `batch.size` (bytes -- same name/unit as Java) | -- |

KIP-794 detail: `partitioner.ignore.keys` is a Java-client-only property and
does NOT exist in librdkafka. The librdkafka equivalent for uniform sticky
null-key distribution is to keep the default `consistent_random` partitioner
and set `sticky.partitioning.linger.ms` equal to the linger window, so
null-key batches stick to one partition until the batch is full, then rotate.
The sizing surface sets this to `linger_ms` automatically. It does NOT set
`partitioner` (keyed `RoutedSender` paths set their own).

`queue.buffering.max.kbytes` is in KiB -- the config struct stores the
producer buffer in bytes and divides by 1024 when it applies the property.

---

## The rho ~ 0.7 loop

The PROCESS byte budget is driven by an AIMD loop (full description in
[SELF-REGULATION.md](SELF-REGULATION.md) and `src/governor/budget.rs`). In
Kafka terms:

- `rho = EMA(process_time) / EMA(ingest_interval)` -- how much of the gap
  between fetches the stage spends processing.
- `rho < 0.7` (slack) -> additive-increase the budget: pull bigger blocks.
- `rho > 0.7` (behind) -> multiplicative-decrease: pull smaller blocks.
- memory HARD pressure -> multiplicative-decrease IMMEDIATELY, regardless of
  rho. Memory never waits for the rho loop.

Target `0.7` keeps the consumer ~70% busy with 30% headroom for a fetch burst.

---

## Wire compression

All profiles default the producer to `lz4` -- the best throughput/ratio
tradeoff for the hot path. Match the codec to the topic:

- **`lz4`** (default) -- fast, good ratio. The right choice for most
  transform / forwarding topics.
- **`zstd`** -- better ratio at higher CPU cost. Opt in for storage-bound
  topics (archiver, long-retention land/load topics) that can absorb the CPU
  to save disk and network.

Set per stage via `kafka.sizing.producer.compression_type`. The consumer
decompresses transparently regardless of producer codec.

---

## KIP / Kafka 4.0 notes

- **KIP-429 (cooperative rebalancing)** -- the consumer profiles set
  `partition.assignment.strategy = cooperative-sticky` to avoid
  stop-the-world rebalances. Combined with partition-pause backpressure (see
  [BACKPRESSURE.md](BACKPRESSURE.md)), a paused consumer stays IN the group
  rather than triggering a rebalance.
- **KIP-794 (uniform sticky partitioner)** -- handled via
  `sticky.partitioning.linger.ms` as above, since librdkafka lacks the Java
  `partitioner.ignore.keys`.
- **KIP-848 (new consumer group protocol)** and **Kafka 4.0** -- the sizing
  surface is property-name based and forward-compatible: as librdkafka adds
  support, the raw escape hatch can set the new properties without a rustlib
  change. Share groups (below) are the 4.0 answer to partition-limited
  scaling.

---

## The raw escape hatch

The sizing surface is opinionated but never a cage. Two raw maps let an
operator set ANY librdkafka property:

```yaml
kafka:
  sizing:
    consumer_librdkafka:
      fetch.wait.max.ms: "75"
    producer_librdkafka:
      linger.ms: "50"
```

These are applied LAST and WIN over both the profile defaults and the named
knobs. Resolution precedence (lowest to highest):

1. `SelfRegulationProfile` defaults
2. Named knobs (`consumer.*` / `producer.*`)
3. Raw maps (`consumer_librdkafka` / `producer_librdkafka`)

When a raw override touches a property the sizing governor depends on (the
fetch byte sizes, `enable.auto.commit`, the producer batch/linger/compression
keys), the code logs ONE warning line per key so the operator knows the
governor's assumptions have changed. An invalid key silently no-ops in
librdkafka, so double-check spelling.

There is also a transport-level `kafka.librdkafka_overrides` map (separate
from the sizing surface) that overrides the profile baseline for the broader
client config.

---

## Partition-limited diagnostic

A Kafka consumer group cannot have more ACTIVELY-consuming members than the
topic has partitions -- extra members sit idle. So adding pods past the
partition count does nothing for throughput; the lag just keeps growing while
half the pods do nothing.

rustlib DETECTS this and tells you. It does NOT fix it by mutating topology.
`KafkaTransport::check_partition_limited` (governor feature,
`src/transport/kafka/mod.rs`) reads the group member count, the assigned
partition count, and the current lag, then evaluates the pure
`partition_limited(members, partitions, lag)` decision. When limited it:

- sets the `kafka_partition_limited` gauge to `1.0` (else `0.0`),
- registers a `Degraded` entry on the health registry, and
- emits ONE rate-limited warning per cooldown window (deduped so a
  persistently-limited consumer does not spam the log).

It NEVER calls `createPartitions` -- no topology mutation. This is a
metadata round-trip, run periodically (e.g. once per refresh tick via
`spawn_partition_limited_tick`), not on the recv hot path.

The warning:

```text
WARN  kafka consumer group is partition-limited: members >= partitions with
      persistent lag -- extra consumers sit idle; the topic needs more
      partitions (diagnostic only, no topology change made)
      members=8 partitions=4 lag=120000
```

Resolution is an operator / topology decision, not something rustlib should
do silently:

- **More partitions per pod** -- raise per-pod parallelism so each consumer
  drains its assigned partitions faster (the byte budget + worker pool
  already help here).
- **Share groups (KIP-932, Kafka 4.0)** -- let more consumers than partitions
  share a partition's records, breaking the one-member-per-partition ceiling.
- **Over-partition the topic** -- create the topic with more partitions than
  you expect to need, so scale-out has headroom. The cleanest fix, but a
  create-time decision.
