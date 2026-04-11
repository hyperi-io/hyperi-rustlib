## Transport Filter Engine — Follow-ups

Tracked items uncovered during the v2.4.7 review of `feat/transport-filter-engine`.
None of these block the merge — they are improvements/hardening to schedule
when transport filters see broader production use.

### #7 — Constant-time string comparison for sensitive fields

**Context.** `CompiledFilter::FieldEquals` / `FieldNotEquals` use plain `==`
for the field-value comparison. If a downstream operator writes a filter
on a high-entropy secret-like field (e.g. an API token, an opaque session
ID, an HMAC), the early-exit characteristic of byte-for-byte equality
leaks ~1 byte of comparison position per nanosecond of timing observation.

**Risk.** Low for the production use case (transport filters key off
routing fields, not secrets), but the door is open. An attacker who can
inject crafted payloads and observe per-message latency could in theory
brute-force a guarded field value.

**Proposed fix.** When the filter expression compares against a value
flagged as "sensitive" (heuristic match: > 16 bytes high entropy, contains
`/[A-Za-z0-9+/=]{20,}/`, or explicit opt-in via filter metadata), use
`subtle::ConstantTimeEq` for the comparison. Add a benchmark to measure
the cost — should be negligible compared to the surrounding `sonic-rs`
field extraction.

**Where.** `src/transport/filter/compiled.rs::evaluate()` and a new
`is_sensitive_pattern()` helper.

---

### #8 — Log masking for filter expression content

**Context.** Filter compilation, ordering warnings, and DLQ routing all
emit `tracing` events that include the raw expression text (e.g.
`"filter compiled: tier=tier2 expr=field == \"prod-secret\""`). If an
operator embeds a literal secret in a filter (bad practice but possible),
that secret ends up in logs.

**Risk.** Low — the engine's existing `tracing-throttle` integration
already rate-limits these events, and rustlib's `logger` module masks
known sensitive field names. But the expression text itself is not run
through the masker.

**Proposed fix.** Add an `expression_redacted()` helper that:
- Replaces string literals (anything between `"…"`) with `"<redacted>"`
- Keeps the structural form (`field == <redacted>`) for debugging
- Is the only form ever written to logs

Plumb it through every `tracing::warn!` / `tracing::info!` call site in
`compiled.rs`, `mod.rs`, and `metrics.rs`.

---

### #9 — Pre-quoted bytes for `field == "value"` fast path

**Context.** `FieldEquals` currently extracts the field value via
`sonic-rs` (which produces a `LazyValue`), runs `extract_string_value`
to unescape it into a `Cow<str>`, and then string-compares against the
expected value. For the very common case of a single-segment field
compared against an ASCII string with no escapes, we could pre-bake the
bytes `"…":"value"` once at compile time and use a single `memmem::find`
— same trick as `FieldExists`.

**Estimated win.** ~30-50% on the `field == "value"` path (currently
~250-400 ns, target ~150-200 ns).

**Caveats.**
- Only safe when both the field name AND the value contain no JSON-escape
  characters (`\`, `"`, control bytes). Otherwise the literal pattern
  doesn't match the encoded form.
- Inherits the same nested-field false-positive limitation as
  `FieldExists` (documented in the integration tests).
- Whitespace in the JSON (`{"field" : "value"}`) breaks the literal match.

**Proposed fix.** At compile time, classify the value as "fast-path
eligible" or not. Build a `memchr::memmem::Finder<'static>` for the
eligible cases and short-circuit in `evaluate()`. Fall back to the
sonic-rs extraction otherwise.

**Where.** `src/transport/filter/compiled.rs::FieldEquals` and
`evaluate()`.

---

### #10 — MsgPack edge cases bypass filtering

**Context.** `apply_inbound`/`apply_outbound` detect the payload format
via `PayloadFormat::detect()` (cheap heuristic on the first byte). For
detected MsgPack payloads we currently skip the JSON-oriented filter
engine entirely — the integration test
`adversarial_msgpack_bypasses_filters` documents this.

**Risk.** Filter rules silently do nothing on MsgPack payloads. An
operator who configures `has(_drop_me)` on a Kafka topic carrying
MsgPack messages will see no filtering, no warning, no metric.

**Proposed fix (cheap).** Emit a one-shot `tracing::warn!` per
filter-engine instance the first time a MsgPack payload is seen with
filters configured. Add a `dfe_transport_filter_bypassed_total{reason="msgpack"}`
counter so dashboards can flag the bypass.

**Proposed fix (expensive).** Compile a parallel MsgPack evaluator
(rmp-serde or rmpv) for each `CompiledFilter` variant. Tier 1 ops are
field extractions, which work the same way on MsgPack — the only thing
that changes is the lookup engine. Defer until there is a real customer
asking for MsgPack filtering.

---

### #11 — Preserve original `expression_text` through reload cycles

**Context.** `CompiledFilter::expression_text` stores the as-typed
expression string for use in error messages and ordering-warning logs.
On hot-reload (`ConfigReloader`), the engine is rebuilt from the new
config, but if the new config is structurally identical to the old one,
the same compiled filter could be reused without re-allocating the text
buffer. Currently every reload allocates fresh `String`s for every rule.

**Estimated win.** Negligible per reload, but reloads on a busy system
with thousands of routing rules add up. Mostly an allocator-pressure
hygiene item.

**Proposed fix.** Hash the rule list during compile and cache compiled
filters keyed by hash. On reload, look up by hash before recompiling.

**Where.** `src/transport/filter/mod.rs::TransportFilterEngine::new`.
