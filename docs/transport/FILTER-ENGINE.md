# Filter Engine

The transport filter engine drops or DLQs messages on the way in or
the way out of every transport — Kafka, gRPC, Memory, File, Pipe,
HTTP, Redis — before they reach app code. It's embedded in every
backend, zero-cost when no rules are configured, and tiered so the
common case (field-presence or equality on a top-level field) runs at
~50-100 ns per message without invoking the CEL engine at all.

This is the doc consumers were missing — earlier work tracked
post-spec follow-ups in `TRANSPORT-FILTER-FOLLOWUP.md`; the engine
itself is now production-shipped.

---

## Why the engine exists

Operators want to:

- **Drop noise** at the wire (debug events, internal heartbeats) so it
  doesn't burn budget downstream.
- **Quarantine poison messages** to a DLQ for review without bringing
  down the pipeline.
- **Express filters in operator-language** (CEL) rather than scattering
  if-statements through transport code.
- **Pay nothing** for the engine when no filters are configured.

The three-tier design makes that last point work: the fast tier runs
without CEL, and tier classification happens at config-load — startup
fails fast if a rule lands in a tier the operator hasn't allowed.

---

## Three tiers

```mermaid
flowchart LR
    R[Filter rule] --> C{Classify}
    C -->|"has(field) / field == 'x' / startsWith(...)"| T1["Tier 1<br/>SIMD field ops<br/>~50-100 ns"]
    C -->|compound CEL, &&, \|\|, numeric, size| T2["Tier 2<br/>compiled CEL<br/>~500 ns – 1 µs"]
    C -->|"matches() / filter() / map() / time fns"| T3["Tier 3<br/>complex CEL<br/>~5-50 µs"]
    T2 -.gate.-> G2["expression.allow_cel_filters_in/out"]
    T3 -.gate.-> G3["expression.allow_complex_filters_in/out"]
```

| Tier | Cost per message | Operations | Config gate |
|------|------------------|------------|-------------|
| **1** | ~50-100 ns | `has(field)`, `!has(field)`, `field == "literal"`, `field != "literal"`, `field.startsWith(...)`, `field.endsWith(...)`, `field.contains(...)` | Always on |
| **2** | ~500 ns – 1 µs | Compound CEL (`&&`, `||`), numeric comparison, multi-field access, `size()`, nested paths | `expression.allow_cel_filters_in` / `_out` |
| **3** | ~5-50 µs | `matches()` (regex), `exists()`, `filter()`, `map()`, `all()`, `exists_one()`, `timestamp()`, `duration()` | `expression.allow_complex_filters_in` / `_out` (implies tier-2) |

Tier 1 uses `sonic-rs::get_from_slice` for field extraction plus
single-segment field-name lookups via pre-compiled
`memchr::memmem::Finder` (~10-20 ns when the field name is the only
needle). It never invokes the CEL interpreter.

Tier 2 compiles the CEL expression once at startup, then evaluates
against fields extracted via SIMD.

Tier 3 enables CEL's regex / iteration / time profile (which adds
expensive operations and DoS surface — hence the separate gate).

---

## Classification (no AST walking)

`classify()` decides what tier an expression sits in by **text-pattern
matching**, not AST analysis. Lazy-static regex patterns for each
tier-1 operation are tried in order of expected frequency
(`has`, `!has`, `==`, `!=`, `startsWith`, `endsWith`, `contains`). If
none match, scan for restricted function names — match means tier 3.
Otherwise tier 2.

This is conservative on purpose: only expressions that obviously fit a
tier-1 pattern execute outside the CEL engine. Anything subtle drops
to tier 2 or 3 where the actual CEL interpreter validates the
expression.

See [src/transport/filter/classify.rs](../../src/transport/filter/classify.rs).

---

## Semantics

- **First-match wins.** Filters are evaluated in declared order; the
  first match returns its action and stops the loop. No match → message
  passes.
- **`drop` action** silently discards. Recorded via the metric
  `filter_{direction}_{action}_total{tier,rule_index}`.
- **`dlq` action** produces a `FilteredDlqEntry` returned **inline** in
  `recv()`'s `RecvBatch.dlq_entries` (alongside the passing
  `RecvBatch.messages`) — the transport does **not** route to a DLQ
  directly. The caller routes `batch.dlq_entries` to the DLQ of its
  choice; they cannot be silently lost. (When the `BatchEngine` run
  loops own the receive, a `FilterDlqPolicy` governs this — `Reject` by
  default, or `Route`/`DiscardWithMetric`.)
- **Inbound and outbound are independent.** Each transport has
  `filters_in` and `filters_out`, evaluated separately.
- **Startup fails fast** on rules above the allowed tier, on invalid
  CEL syntax, on empty expressions, or on `dlq` actions when no DLQ is
  configured.

---

## Config

Two distinct cascade keys gate filter behaviour. Operators have to set
both correctly because they apply at different layers.

### `transport.<backend>.filters_{in,out}` — the rules themselves

Per-transport rule lists. Each rule has an `expression` (CEL text) and
an `action` (`drop` or `dlq`). Rules apply at the named transport, in
the named direction.

```yaml
transport:
  kafka:
    brokers: ["kafka.devex.hyperi.io:9092"]
    topics: ["events"]
    filters_in:
      - expression: 'has(_internal)'
        action: drop
      - expression: 'status == "poison"'
        action: dlq
      - expression: 'severity > 3 && source != "internal"'
        action: dlq
    filters_out:
      - expression: 'has(debug)'
        action: drop
```

### `transport.filter_tiers.*` — the tier gates

Top-level gate controlling which tiers any transport is allowed to
compile. Defaults to all Tier 2/3 gates closed — first-time deployments
get Tier 1 only.

```yaml
transport:
  filter_tiers:
    allow_cel_filters_in: false       # Tier 2 (compiled CEL) inbound
    allow_cel_filters_out: false      # Tier 2 outbound
    allow_complex_filters_in: false   # Tier 3 (regex/iteration/time) inbound
    allow_complex_filters_out: false  # Tier 3 outbound
```

If a configured filter rule classifies above the allowed tier, the
transport's constructor returns `TransportError::FilterCompile(...)`
and the transport fails to start. **Fail-loud, not fail-silently** —
the previous behaviour of substituting an empty filter engine has
been removed (a misconfigured drop/dlq rule would otherwise let every
message through).

### `expression.*` — the CEL function gates (layered with the tier gates)

Top-level gate controlling which CEL functions are usable anywhere in
the app (filters, transforms, validators). Tier 2 and Tier 3 filter
compilation honour this on top of the tier-gate above.

```yaml
expression:
  allow_regex: false        # regex matches() function
  allow_iteration: false    # filter/map/all/exists/exists_one
  allow_time: false         # time-related functions
```

### Both layers must allow

A filter rule like `'tag.matches("^prod-")'` compiles successfully
only when **both**:

1. `transport.filter_tiers.allow_complex_filters_in` is `true` (the
   tier gate — "this transport accepts Tier 3 inbound").
2. `expression.allow_regex` is `true` (the function gate — "regex is
   allowed in CEL anywhere in this app").

Either gate set to `false` rejects the filter at config-load. This
gives operators two independent levers — they can turn off all regex
filters across the app with one flag, or turn off Tier 3 for a single
transport while leaving regex enabled elsewhere.

### Reload semantics

The tier-gate config (`transport.filter_tiers.*`) is read **at
transport construction time** via
`TransportFilterTierConfig::from_cascade()`. A `ConfigReloader` update
to those keys does **not** propagate to an already-running transport
— the old gates remain in effect until the transport is reconstructed.

This is intentional: a misconfigured reload that flips a gate would
otherwise tear down a working transport mid-stream. Operators wanting
the new gate config to take effect should restart the service (or, in
K8s, roll the pod). Per-rule reload (adding/removing rules while
keeping the same tier policy) is a separate workstream not yet
shipped.

A first-time deployment should start with all gates off — only Tier 1
filters work. Flip a gate on once Tier 2 or Tier 3 is genuinely needed
and the operator has reviewed the cost (Tier 3 is unbounded CPU; see
[Known limitations](#known-limitations)).

---

## API surface

```rust
use hyperi_rustlib::transport::filter::{
    TransportFilterEngine, FilterDisposition,
    FilteredBatch, FilteredDlqEntry,
    FilterAction, FilterDirection, FilterRule, FilterTier,
};

// Built once per transport from config:
let engine = TransportFilterEngine::new(
    filters_in,
    filters_out,
    tier_config,
)?;

// On the hot path:
match engine.apply_inbound(&payload, &key) {
    FilterDisposition::Pass => process(payload),
    FilterDisposition::Drop => continue,
    FilterDisposition::Dlq  => continue,   // returned in RecvBatch.dlq_entries
}

// DLQ entries come back inline from recv() -- route them per batch:
let batch = transport.recv(max).await?;
for entry in batch.dlq_entries {
    dlq_sender.send(entry).await?;
}

// Cheap fast-paths:
if !engine.has_inbound_filters() {
    /* short-circuit, skip evaluation */
}
```

`has_inbound_filters` and `has_outbound_filters` are marked
`#[inline]` so the no-filter branch becomes a single check on the hot
path.

`FilteredBatch::passthrough` constructs a batch when filters are off,
so transport code doesn't fork for the empty case.

---

## Where it's embedded

Every transport backend builds the engine at construction time from
its own config section's `filters_in` / `filters_out` plus the global
`expression.*` tier gates:

| Transport | Source |
|-----------|--------|
| Kafka | [src/transport/kafka/mod.rs](../../src/transport/kafka/mod.rs) |
| gRPC | [src/transport/grpc/mod.rs](../../src/transport/grpc/mod.rs) |
| Memory | [src/transport/memory/mod.rs](../../src/transport/memory/mod.rs) |
| File | [src/transport/file.rs](../../src/transport/file.rs) |
| Pipe | [src/transport/pipe.rs](../../src/transport/pipe.rs) |
| HTTP | [src/transport/http.rs](../../src/transport/http.rs) |
| Redis | [src/transport/redis_transport.rs](../../src/transport/redis_transport.rs) |

The engine is a no-op when both filter vectors are empty — there's no
per-message overhead beyond the inlined `has_*_filters` check.

---

## MsgPack and binary payloads

The engine targets JSON. When `apply_*` sees a payload that looks
like MsgPack (heuristic detection of MsgPack signature bytes), it
short-circuits to `Pass` without warning or metric.

This is a deliberate choice — running JSON-shaped filters against
binary payloads would either falsely match or always reject. Pipelines
that need to filter MsgPack should either upgrade their filters once a
MsgPack evaluator ships, or convert to JSON upstream.

---

## Known limitations

The post-spec follow-up items from earlier work, with current status:

| # | Item | Status | Notes |
|---|------|--------|-------|
| 7 | Constant-time string comparison for sensitive fields | Pending | Low risk; door open for timing attacks on high-entropy field values |
| 8 | Log masking for filter expression content | Pending | Expression text logged as-is at startup; expression authors should treat expressions as non-secret |
| 9 | Pre-quoted bytes fast path for `field == "value"` | Partial | `FieldExists` / `FieldNotExists` already use pre-compiled `memmem::Finder`; `FieldEquals` still uses SIMD extract + string compare |
| 10 | MsgPack payloads silently pass | Acknowledged | Design choice; cheap fix would be a one-shot warn + metric on first MsgPack seen |
| 11 | Preserve original `expression_text` through reload cycles | Pending | Current code re-allocates on reload; allocator-hygiene item, no functional impact |
| 12 | Tier 3 CEL has no evaluation budget | Open — design spec'd | `program.execute(&ctx)` runs with no time/recursion/payload cap. A bad filter can wedge the ingest thread. Static AST budget + payload size cap + degraded-mode fallback design spec'd in the project working area; must land before Phase 3 of the log-scrub landing plan (gitleaks rules are regex-heavy by nature) |

The items aren't blockers. Operators should know about #10 if their
pipeline mixes JSON and MsgPack.

---

## Tests and benchmarks

- Unit tests: 32 across the module (mod, classify, config, compiled, metrics).
- Integration tests: 54 in `tests/transport_filter.rs` — round-trip,
  adversarial inputs, Unicode, 1000-rule lists, MsgPack heuristic.
- Benchmarks: `benches/filter_benchmark.rs` — 8 criterion groups
  covering no-filter baseline, each tier-1 op, first-match-at-N, tier-2
  compound, tier-3 regex.

Tier-1 latency confirmed at ~50-100 ns/message on the bench machine.

---

## Related

- [transport/OVERVIEW.md](OVERVIEW.md) — trait architecture, factory, `AnySender`
- [transport/BACKENDS.md](BACKENDS.md) — per-backend wiring
- [pipeline/DLQ.md](../pipeline/DLQ.md) — DLQ sink backends
- [core-pillars/CONFIG.md](../core-pillars/CONFIG.md) — cascade
- [FEATURE-FLAGS.md](../FEATURE-FLAGS.md) — `transport`, `expression`
- Spec history:
  [docs/superpowers/specs/2026-04-10-transport-filter-engine-design.md](../superpowers/specs/2026-04-10-transport-filter-engine-design.md)
