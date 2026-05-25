# Strmatch

> **Status: Planned — not in v2.7.4.** The module is designed and
> largely written but has not landed on `main` or `crates.io`. This
> page is the API reference the module *will* expose when it ships —
> treat it as a design preview, not as functionality you can
> `cargo add` today.

---

`strmatch` is the official answer to "don't use regex on the hot
path". Operators write a regex (the pattern language everyone knows);
the matcher classifies it into one of four tiers and dispatches via
the cheapest engine that's correct. Most hot-path patterns —
field-presence checks, prefix tests, alternation over a fixed token
list — never invoke the regex engine at all.

When shipped, replaces casual `Regex::new(...)` calls in routers,
scrubbers, classifiers, and field filters across the DFE stack.

---

## Four tiers

| Tier | Engine | Typical budget¹ | What classifies here |
|------|--------|-----------------|----------------------|
| `Byte` | `memchr` / `memchr2` / `memchr3` / single-byte `starts_with` / `ends_with` / `==` | ≤ 30 ns | `/x/`, `/[xy]/`, `/[xyz]/`, `/^/`, `/$/` on single byte |
| `Literal` | `memmem::Finder` / multi-byte `starts_with` / `ends_with` / `==` | ≤ 200 ns | `/AKIA/`, `/^https:/`, `/error$/`, `/^GET /` |
| `LiteralSet` | `aho-corasick` over ≥ 2 literals (one linear scan) | ≤ 500 ns | `/AKIA|ghp_|sk_live_/`, anchored alternation, extractor-derived literal sets |
| `Regex` | `regex_automata::meta::Regex` (full engine, with its own prefilter pipeline) | engine-bounded | Word boundaries, multi-line anchors, unbounded quantifiers, large unicode classes, everything else |

¹Budgets are typical for a modern x86 server on a ~200-byte haystack.
See `benches/strmatch.rs`.

`tier_for_shape` distinguishes single-byte anchored literals (Byte)
from multi-byte ones (Literal), since multi-byte `starts_with` /
`ends_with` / `==` are still slightly more expensive than a `memchr`
call.

---

## Classification (HIR walk, not regex on regex)

`classify::classify` parses the pattern via `regex-syntax` and walks
the HIR. Three outcomes:

1. **Shape** — single byte / byte-set / anchored or unanchored
   literal. Direct byte-op dispatch.
2. **LiteralOnly** — alternation of exact literals with no
   lookaround. Single `aho-corasick` automaton; regex engine never
   invoked at match time.
3. **Meta** — anything that contains a feature we can't safely
   reduce (word boundaries, multi-line anchors, lookarounds,
   unbounded quantifiers, non-ASCII unicode classes past a small
   byte-cap).

The classifier never reports "this regex is invalid" — that's the
parser's job. It returns a `Plan` (the dispatch shape) and a
`Descriptor` explaining which tier was chosen and (if Meta) why.

A separate extractor fallback handles patterns that are
literal-shaped after `regex-syntax::hir::literal::Extractor` strips
prefixes/suffixes. So `Regex::new(r"^/api/(orders|users|carts)$")`
ends up on `LiteralSet` despite the surface anchors.

---

## Quality gates

`StrMatcherBuilder::min_tier` rejects (or loudly warns about) patterns
that classify below an operator-chosen tier. Useful for hot-path
configs where regex fall-through is unacceptable:

```rust
use hyperi_rustlib::strmatch::{MatcherTier, OnBelowMin, StrMatcher};

let scrubber = StrMatcher::builder()
    .min_tier(MatcherTier::LiteralSet)
    .on_below_min(OnBelowMin::Reject)
    .build(pattern)?;        // fails at config-load if pattern is Meta-tier
```

`OnBelowMin` choices:

| Policy | Effect |
|--------|--------|
| `Allow` (default) | Build succeeds; anti-spam protocol still emits up to 10 WARNs per process |
| `Warn` | Build succeeds; always WARN (bypasses anti-spam cap) |
| `Reject` | Build fails with `BuildError::TierTooLow { pattern, wanted, got, reason, hint }` |

---

## Anti-spam discipline

When a pattern compiles to the `Regex` tier, `strmatch` emits **one**
WARN per distinct pattern per process, capped at 10 distinct WARNs
total. Past the cap, further patterns log at DEBUG plus one INFO
summary. The counter `hyperi_strmatch_regex_fallback_total` is
incremented regardless of log level — operators can scrape that
without touching log volume.

---

## Sets

`StrMatcherSet` compiles a vector of patterns at once. Each pattern
gets its own `StrMatcher` (and so its own tier). Useful for scrubbers
with a fixed pattern library:

```rust
let set = StrMatcherSet::builder()
    .min_tier(MatcherTier::Literal)
    .on_below_min(OnBelowMin::Reject)
    .build_set([
        "AKIA",                              // Literal
        "ghp_|sk_live_|gho_",                // LiteralSet
        r"^/api/(orders|users)$",            // LiteralSet via extractor
    ])?;

assert_eq!(set.tier_counts(), [0, 1, 2, 0]); // Byte / Literal / LiteralSet / Regex
```

`earliest_match` returns the leftmost match across all patterns,
breaking ties by input order. `find_iter` collects every
non-overlapping match. `tier_counts` is the dashboard input for
operator quality checks.

---

## Case-insensitivity

`StrMatcherBuilder::ascii_case_insensitive(true)` propagates to the
AC builder. The Shape tier becomes unavailable (memchr/memmem can't
fold case), so single literals route to a one-element AC. Pattern is
**not** wrapped in `(?i:...)` before parsing — that would expand
literals into per-byte case classes and defeat simple-shape
detection.

---

## API surface

| Item | Purpose |
|------|---------|
| `StrMatcher::new(pattern)` | Compile with defaults |
| `StrMatcher::builder() -> StrMatcherBuilder` | Custom build with `min_tier` / `on_below_min` / case-folding |
| `StrMatcher::is_match(hay) -> bool` | Hot path — single match arm, 1–2 instructions for Byte / Literal |
| `StrMatcher::find(hay) -> Option<Match>` | First match offsets |
| `StrMatcher::find_iter(hay) -> impl Iterator<Item = Match>` | All non-overlapping matches |
| `StrMatcher::tier() / pattern() / reason()` | Introspection for telemetry, tests, dashboards |
| `StrMatcherSet::new(patterns)` | Compile a set |
| `StrMatcherSet::is_match / earliest_match / find_iter / tier_counts / len` | Set-level dispatch |
| `MatcherTier::rank() / typical_budget_ns()` | Tier comparisons and ns budgets |
| `OnBelowMin::Allow / Warn / Reject` | Quality-gate policy |
| `BuildError::Empty / Syntax / TierTooLow` | Structured construction errors with rustc-style hints |

`Match { start, end }` and `SetMatch { start, end, pattern_idx }` are
byte-offset structs with end-exclusive ranges (`&hay[start..end]`).

---

## Source and benchmarks

- [`../../src/strmatch/mod.rs`](../../src/strmatch/mod.rs) — public API
- [`../../src/strmatch/classify.rs`](../../src/strmatch/classify.rs) — HIR walk + tier selection
- [`../../src/strmatch/plan.rs`](../../src/strmatch/plan.rs) — `Plan` enum and match-time dispatch
- [`../../benches/strmatch.rs`](../../benches/strmatch.rs) — criterion benchmarks per tier

---

## Related

- [BATCH-ENGINE.md](BATCH-ENGINE.md) — pre-route filters use the same byte-op primitives
- [../transport/FILTER-ENGINE.md](../transport/FILTER-ENGINE.md) — Tier 1 wire-filter uses `memmem::Finder` for the same reason
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) — `strmatch`
- [../ARCHITECTURE.md](../ARCHITECTURE.md)
