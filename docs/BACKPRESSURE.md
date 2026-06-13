# Backpressure

The one doctrine: **gate the SOURCE, never the SINK.**

When a pipeline falls behind, pause the inbound source so the in-flight buffer
drains. Do NOT slow the outbound drain. Gating the drain DEADLOCKS: the sink
stops accepting, the buffer fills, the source keeps pulling (nothing told it to
stop), the pipeline wedges. Backpressure propagates UPSTREAM to the intake.

So the inbound gate (`src/governor/gate.rs`) is INBOUND-only; `send` is never
involved. See [SELF-REGULATION.md](SELF-REGULATION.md) for the governor that
decides WHEN to gate, [KAFKA-PATH.md](KAFKA-PATH.md) for the Kafka brake.

---

## InboundGate -- one primitive, reused everywhere

`InboundGate` is the single public backpressure primitive: a shared
`UnifiedPressure` latch + an actuator, driven on pause/resume EDGES. `pause()`
fires once on the rising (false->true) edge, `resume()` once on the falling
edge. While the latch stays held, repeated `evaluate()` calls return
`Admit::Hold` without re-calling `pause()`.

The same gate and the same `Admit::{Yes, Hold}` decision drive every
transport's intake. Only the actuator differs:

| Stage | Brake mechanism | Commit / ack token | Lossless? |
|---|---|---|---|
| Loader / transform (Kafka in) | Pause ASSIGNED partitions (member stays in group, no rebalance) | Kafka offset, committed after send | Yes -- offsets not advanced, re-delivered |
| Receiver (HTTP / gRPC in) | Return 503 / `UNAVAILABLE` to the caller | HTTP responder / gRPC status | Only if the upstream RETRIES the rejected request |
| Fetcher (poll a source) | Pause-fetch (stop the poll loop) | Fetch cursor | Yes -- cursor not advanced, re-fetched |

The hysteresis band (`pause_above` / `resume_below`) stops flapping: once
paused the gate stays paused until pressure drops well below the pause
threshold, not the instant it dips under it.

### The lossless caveat for the receiver

Kafka and fetcher sources are pull-based: pausing the pull leaves the data at
rest in the broker / source, re-read once the gate reopens. Nothing is lost.

The receiver is push-based. Its 503 / `UNAVAILABLE` is only lossless if the
UPSTREAM caller retries. A well-behaved DFE sender does; a fire-and-forget
client does not, and its rejected payload is gone. The gate cannot make a push
source lossless on its own -- that contract lives with the caller. Deliberate
limit, not a bug.

---

## The brake / commit-token table

The brake is one half of the at-least-once contract; the commit token is the
other. The driver commits source acks ONLY after the whole out-batch is sent
(`src/worker/engine/driver.rs`). A brake without matching commit discipline
would lose or double-count data. The two are designed together:

- **Brake** decides whether to pull the next unit of work.
- **Commit token** decides when the source ack fires -- always after a
  successful send, never before. A send failure skips the commit, so the
  block is re-delivered (at-least-once: duplicates, never loss).

Commit tokens live on the `WorkBatch`, not on the record, and their count is
decoupled from the record count. A transform that fans `N` records out to
`2N` does not multiply the source acks -- the driver commits EXACTLY the `N`
input tokens after the `2N`-record block is sent. See
[MIGRATIONS.md](MIGRATIONS.md) for the `WorkBatch` / `Record` / tokens-on-batch
contract.

### Filter-dropped records still commit

A record an inbound filter DROPS or routes to DLQ produces no passing record,
but it WAS handled -- so its commit token must still fire. The filter carries
those tokens (`FilteredBatch.filtered_tokens`, flowed into
`WorkBatch.commit_tokens`) so the block commit advances the source past them.
Drop the token instead and an all-filtered stretch FREEZES the Kafka offset
(replay storm + phantom KEDA lag on restart) and LEAKS the Redis
consumer-group PEL forever. Handled == committed, even when nothing passed.

---

## Streaming -- bounding peak memory

The governed driver need not hold a whole received block in memory at once.
`BatchEngine::run_workbatch_streaming` (and `run_governed` when the byte budget
is wired) processes each block in consecutive byte-budget-sized SUB-BLOCKS:

```text
  recv(block of N records)
    -> split into sub-blocks of ~budget bytes (floor: one record)
    -> for each sub-block:
         lease its bytes -> process -> send -> RELEASE lease
    -> commit ALL the block's source acks ONCE, after the final sub-block
```

Each sub-block's ingress lease is released BEFORE the next is leased, so peak
in-flight ingress memory is bounded to ONE sub-block (`~sub_block_bytes`), not
the whole block. A record larger than the budget is still its own single-record
sub-block, so the loop never stalls.

Commit discipline holds across the split:

- Each sub-block view carries EMPTY commit tokens, so a fan-out within a
  sub-block never multiplies the source acks.
- The whole block's source acks commit EXACTLY ONCE, after the FINAL
  sub-block's sink returns `Ok` (under `CommitMode::Auto`).
- A sink error on ANY sub-block stops the block and skips the commit, so the
  WHOLE block is re-delivered. At-least-once holds even mid-stream.

Under low pressure the budget is big: the whole block is a single sub-block, no
per-record overhead -- the streaming path collapses to the whole-batch path.
The byte budget shrinks peak memory only when pressure demands it.

---

## Pressure -> lag -> KEDA

Pausing intake is the LOCAL response on one pod. It buys time for the buffer to
drain, but sustained pressure -- the pod just cannot keep up -- needs more pods,
not a permanently-paused intake.

A paused source grows consumer lag (Kafka) or queue depth (other sources). KEDA
reads that lag via `ScalingPressure`'s external-scaler signal and adds replicas.
The two levers compose:

- **InboundGate** is the fast, local, per-pod brake -- milliseconds, no new
  capacity.
- **KEDA scaling** is the slow, global, capacity response -- seconds to a new
  pod, driven by the lag the brake helps surface.

See [SELF-REGULATION.md](SELF-REGULATION.md) (the three brains) and
[pipeline/SCALING.md](pipeline/SCALING.md) (the KEDA signal).
