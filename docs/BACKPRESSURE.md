# Backpressure

The one doctrine: **gate the SOURCE, never the SINK.**

When a pipeline cannot keep up, the safe response is to stop pulling new work
in -- pause the inbound source so the in-flight buffer drains. The unsafe
response is to slow the outbound drain (the sink). Gating the drain
DEADLOCKS the pipeline: the sink stops accepting, the buffer fills, the
source keeps pulling because nothing told it to stop, and the whole thing
wedges. Backpressure must propagate UPSTREAM to the intake, not downstream
to the egress.

This is why the inbound gate (`src/governor/gate.rs`) is deliberately the
INBOUND side only, and `send` is never involved. See
[SELF-REGULATION.md](SELF-REGULATION.md) for the pressure governor that
decides WHEN to gate, and [KAFKA-PATH.md](KAFKA-PATH.md) for the
Kafka-specific brake.

---

## InboundGate -- one primitive, reused everywhere

`InboundGate` is the single public backpressure primitive. It wraps a shared
`UnifiedPressure` latch and an actuator, and drives the actuator on
pause/resume EDGES -- `pause()` exactly once on the false->true (rising)
edge, `resume()` exactly once on the true->false (falling) edge. While the
latch stays held, repeated `evaluate()` calls return `Admit::Hold` but do
NOT re-call `pause()`.

The same gate, the same `Admit::{Yes, Hold}` decision, drives every
transport's intake. Only the actuator differs per transport:

| Stage | Brake mechanism | Commit / ack token | Lossless? |
|---|---|---|---|
| Loader / transform (Kafka in) | Pause ASSIGNED partitions (member stays in group, no rebalance) | Kafka offset, committed after send | Yes -- offsets not advanced, re-delivered |
| Receiver (HTTP / gRPC in) | Return 503 / `UNAVAILABLE` to the caller | HTTP responder / gRPC status | Only if the upstream RETRIES the rejected request |
| Fetcher (poll a source) | Pause-fetch (stop the poll loop) | Fetch cursor | Yes -- cursor not advanced, re-fetched |

The hysteresis band (`pause_above` / `resume_below`) stops the gate from
flapping: once paused it stays paused until pressure drops well below the
pause threshold, not the instant it dips under it.

### The lossless caveat for the receiver

Kafka and fetcher sources are pull-based: pausing the pull simply leaves the
data at rest in the broker / source, to be re-read once the gate reopens.
Nothing is lost.

The receiver is push-based. When it returns 503 / `UNAVAILABLE`, the data is
only safe if the UPSTREAM caller retries. A well-behaved DFE sender retries on
503; a fire-and-forget client does not, and its rejected payload is gone. The
gate cannot make a push source lossless on its own -- that contract lives
with the caller. This is a deliberate, documented limit, not a bug.

---

## The brake / commit-token table

The brake is one half of the at-least-once contract; the commit token is the
other. The driver commits the input source acks ONLY after the whole
out-batch is sent (`src/worker/engine/driver.rs`). A brake that pauses intake
without the matching commit discipline would either lose data or double-count
it. The two are designed together:

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

---

## Streaming -- bounding peak memory

The governed driver does not have to hold a whole received block in memory at
once. `BatchEngine::run_workbatch_streaming` (and `run_governed` when the
byte budget is wired) processes each received block in consecutive
byte-budget-sized SUB-BLOCKS:

```text
  recv(block of N records)
    -> split into sub-blocks of ~budget bytes (floor: one record)
    -> for each sub-block:
         lease its bytes -> process -> send -> RELEASE lease
    -> commit ALL the block's source acks ONCE, after the final sub-block
```

The per-sub-block ingress lease is dropped (releasing those bytes) BEFORE the
next sub-block is leased. So peak in-flight ingress memory is bounded to ONE
sub-block (`~sub_block_bytes`), not the whole block. A record larger than the
budget is still its own single-record sub-block, so the loop never stalls.

The commit discipline is preserved across the streaming split:

- Each sub-block view carries EMPTY commit tokens, so a fan-out within a
  sub-block never multiplies the source acks.
- The whole block's source acks are committed EXACTLY ONCE, after the FINAL
  sub-block's sink returns `Ok` (under `CommitMode::Auto`).
- A sink error on ANY sub-block stops the block and skips the commit, so the
  WHOLE block is re-delivered. At-least-once holds even mid-stream.

Under low pressure the budget is big, so the whole block is a single
sub-block and there is no per-record overhead -- the streaming path collapses
to the whole-batch path. The byte budget is the lever that makes peak memory
shrink only when pressure demands it.

---

## Pressure -> lag -> KEDA

Pausing intake is the LOCAL response on one pod. It buys time for the
in-flight buffer to drain, but if the pressure is sustained -- the pod simply
cannot keep up -- the right answer is more pods, not a permanently-paused
intake.

A paused inbound source grows consumer lag (Kafka) or queue depth (other
sources). KEDA reads that lag via `ScalingPressure`'s external-scaler signal
and adds replicas. So the two levers compose:

- **InboundGate** is the fast, local, per-pod brake -- milliseconds, no new
  capacity.
- **KEDA scaling** is the slow, global, capacity response -- seconds to a new
  pod, driven by the lag the brake helps surface.

See [SELF-REGULATION.md](SELF-REGULATION.md) (the three brains) and
[pipeline/SCALING.md](pipeline/SCALING.md) (the KEDA signal).
