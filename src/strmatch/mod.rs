// Project:   hyperi-rustlib
// File:      src/strmatch/mod.rs
// Purpose:   Public API for the strmatch regex→fast-path matcher
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Regex-shaped patterns, fast-path dispatch.
//!
//! Operators write a regex (the most familiar pattern language).
//! `strmatch` classifies it into one of four tiers and dispatches at
//! match time via the cheapest engine that's correct:
//!
//! - **Byte** (≤ 30 ns) -- direct byte ops: `memchr` / `memchr2` /
//!   `memchr3` / single-byte `starts_with` / `ends_with` / `==`.
//! - **Literal** (≤ 200 ns) -- single multi-byte literal:
//!   `memmem::Finder` / multi-byte `starts_with` / `ends_with` / `==`.
//! - **LiteralSet** (≤ 500 ns) -- `aho-corasick` over ≥ 2 literals
//!   (optional uniform anchor checked after the AC scan). Regex engine
//!   is never invoked.
//! - **Regex** (engine-bounded) -- fall through to
//!   `regex-automata::meta::Regex`. Has its own internal prefilter
//!   pipeline; cost depends on pattern and haystack.
//!
//! Budgets are typical for a modern x86 server on a ~200-byte
//! haystack; see [`MatcherTier::typical_budget_ns`] and
//! `benches/strmatch.rs`.
//!
//! ## Anti-spam discipline
//!
//! When a pattern compiles to [`MatcherTier::Regex`] (the engine
//! fall-back), `strmatch` emits **one** WARN per distinct pattern per
//! process, capped at 10 distinct WARNs total. After the cap, further
//! fall-through patterns log at DEBUG. A counter
//! `hyperi_strmatch_regex_fallback_total` increments on every
//! fall-through regardless of log level -- operators can scrape that
//! rather than rely on logs.
//!
//! ## Quality gates
//!
//! Use [`StrMatcher::builder`] with [`StrMatcherBuilder::min_tier`] to
//! reject (or loudly warn about) patterns that fall below an
//! operator-chosen tier. Useful for hot-path configs where regex
//! fall-through is unacceptable.
//!
//! ## Example
//!
//! ```
//! use hyperi_rustlib::strmatch::{MatcherTier, OnBelowMin, StrMatcher};
//!
//! // Byte tier -- anchored single byte, dispatches to hay.first() == Some(b)
//! let m = StrMatcher::new(r"^/")?;
//! assert_eq!(m.tier(), MatcherTier::Byte);
//! assert!(m.is_match(b"/api/v1/orders"));
//!
//! // Literal tier -- multi-byte literal, dispatches to memmem
//! let m = StrMatcher::new(r"AKIA")?;
//! assert_eq!(m.tier(), MatcherTier::Literal);
//! assert!(m.is_match(b"... AKIA1234 ..."));
//!
//! // LiteralSet tier -- alternation, dispatches to AhoCorasick
//! let m = StrMatcher::new(r"AKIA|ghp_|sk_live_")?;
//! assert_eq!(m.tier(), MatcherTier::LiteralSet);
//! assert!(m.is_match(b"github token: ghp_abcdef"));
//!
//! // Regex tier -- falls through to engine; refuse the build instead
//! let err = StrMatcher::builder()
//!     .min_tier(MatcherTier::LiteralSet)
//!     .on_below_min(OnBelowMin::Reject)
//!     .build(r"\w+@\w+")
//!     .unwrap_err();
//! assert!(err.to_string().contains("tier"));
//! # Ok::<(), hyperi_rustlib::strmatch::BuildError>(())
//! ```

mod classify;
mod plan;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

use thiserror::Error;

use classify::Classified;
use plan::Plan;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Which engine class a [`StrMatcher`] is dispatching to. Tiers are
/// ordered by cost -- `Byte > Literal > LiteralSet > Regex` (higher
/// means faster). Use [`Self::rank`] for `min_tier` comparisons.
///
/// Budgets below are typical for a modern x86 server on a ~200-byte
/// haystack. Hardware-dependent; cold caches and pathological inputs
/// move them around.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatcherTier {
    /// **≤ 30 ns** -- direct byte ops: `memchr` / `memchr2/3` / single-byte
    /// `starts_with` / `ends_with` / `==`.
    Byte,
    /// **≤ 200 ns** -- single multi-byte literal: `memmem::Finder` /
    /// multi-byte `starts_with` / `ends_with` / `==`.
    Literal,
    /// **≤ 500 ns** -- `aho-corasick` over ≥ 2 literals (optional
    /// uniform anchor checked after the AC scan).
    LiteralSet,
    /// **Bounded by the regex engine** -- `regex-automata::meta::Regex`
    /// fall-through. Cost depends on pattern and haystack.
    Regex,
}

impl std::fmt::Display for MatcherTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Byte => f.write_str("Byte"),
            Self::Literal => f.write_str("Literal"),
            Self::LiteralSet => f.write_str("LiteralSet"),
            Self::Regex => f.write_str("Regex"),
        }
    }
}

impl MatcherTier {
    /// Cost rank -- higher means faster. Useful for `min_tier`
    /// comparisons without depending on the specific ns numbers.
    ///
    /// Ordering: `Byte (4) > Literal (3) > LiteralSet (2) > Regex (1)`.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Byte => 4,
            Self::Literal => 3,
            Self::LiteralSet => 2,
            Self::Regex => 1,
        }
    }

    /// Indicative ns budget per `is_match` call on a ~200-byte
    /// haystack (modern x86 server). `None` for [`Self::Regex`] since
    /// the engine is unbounded.
    ///
    /// Numbers are estimates from benchmarks in `benches/strmatch.rs`;
    /// real-world cost varies with haystack length, cache state, and
    /// pattern complexity.
    #[must_use]
    pub const fn typical_budget_ns(self) -> Option<u64> {
        match self {
            Self::Byte => Some(30),
            Self::Literal => Some(200),
            Self::LiteralSet => Some(500),
            Self::Regex => None,
        }
    }
}

/// Byte offsets of a match. End is exclusive: `&hay[start..end]` is the
/// matched slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Match {
    pub start: usize,
    pub end: usize,
}

/// Like [`Match`] but also identifies which input pattern matched in a
/// [`StrMatcherSet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SetMatch {
    pub start: usize,
    pub end: usize,
    pub pattern_idx: usize,
}

/// Failure modes during construction.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BuildError {
    /// The pattern is empty.
    #[error(
        "strmatch: empty pattern\n  \
         hint: an empty pattern matches every position; pass a non-empty \
         literal or wrap the matcher in Option for an absent-matcher slot"
    )]
    Empty,

    /// The regex parser rejected the pattern.
    #[error(
        "strmatch: regex syntax error in pattern {pattern:?}\n  \
         reason: {source}\n  \
         hint: {hint}"
    )]
    Syntax {
        pattern: String,
        #[source]
        source: Box<regex_syntax::Error>,
        hint: &'static str,
    },

    /// The pattern compiled to a tier below the builder's minimum.
    #[error(
        "strmatch: pattern {pattern:?} compiles to tier {got}, but builder \
         requires at least {wanted}\n  \
         reason: {reason}\n  \
         hint: {hint}"
    )]
    TierTooLow {
        pattern: String,
        wanted: MatcherTier,
        got: MatcherTier,
        reason: &'static str,
        hint: &'static str,
    },
}

/// What to do when a pattern's classification falls below the
/// builder's `min_tier`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnBelowMin {
    /// Default -- proceed quietly. The anti-spam protocol still emits
    /// up to ten WARNs per process for fall-through patterns.
    #[default]
    Allow,
    /// Proceed, but force an immediate WARN that bypasses the anti-spam
    /// cap. Use when the caller explicitly wants every fall-through
    /// logged regardless of process-wide dedup state.
    Warn,
    /// Refuse to build. Returns [`BuildError::TierTooLow`].
    Reject,
}

// ---------------------------------------------------------------------------
// StrMatcher
// ---------------------------------------------------------------------------

/// Compiled pattern with tier-aware dispatch.
pub struct StrMatcher {
    plan: Plan,
    tier: MatcherTier,
    pattern: String,
    /// Short machine-readable reason (e.g. `"shape:starts-with"`,
    /// `"literal-only:alternation"`, `"unbounded-quantifier"`).
    reason: &'static str,
}

impl std::fmt::Debug for StrMatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StrMatcher")
            .field("tier", &self.tier)
            .field("pattern", &self.pattern)
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

impl StrMatcher {
    /// Compile `pattern` with default options (no minimum tier, no
    /// case-insensitivity).
    ///
    /// # Errors
    ///
    /// Returns `Err` if the pattern is empty or fails to parse. See
    /// [`BuildError`] for the structured variants.
    pub fn new(pattern: &str) -> Result<Self, BuildError> {
        Self::builder().build(pattern)
    }

    /// Start a builder.
    #[must_use]
    pub fn builder() -> StrMatcherBuilder {
        StrMatcherBuilder::new()
    }

    /// `true` if the haystack contains a match.
    #[inline]
    #[must_use]
    pub fn is_match(&self, hay: &[u8]) -> bool {
        self.plan.is_match(hay)
    }

    /// Find the first match, returning byte offsets.
    #[inline]
    #[must_use]
    pub fn find(&self, hay: &[u8]) -> Option<Match> {
        self.plan.find(hay)
    }

    /// Collect every non-overlapping match in iteration order.
    ///
    /// Returns an owned iterator over `Match`. The implementation
    /// eagerly populates a `Vec` and returns its `IntoIter`; this
    /// costs one allocation per call but amortises well over typical
    /// scrubber workloads where each haystack yields 0-3 matches.
    ///
    /// For the anchored shapes (`StartsWith`, `EndsWith`,
    /// `ExactMatch`) the iterator yields at most one match.
    #[must_use]
    pub fn find_iter(&self, hay: &[u8]) -> std::vec::IntoIter<Match> {
        let mut out = Vec::new();
        self.plan.collect_matches(hay, &mut out);
        out.into_iter()
    }

    /// The tier this matcher dispatches to. Useful for telemetry and
    /// assertions in tests.
    #[must_use]
    pub fn tier(&self) -> MatcherTier {
        self.tier
    }

    /// The original pattern string passed to [`Self::new`] (or the
    /// builder).
    #[must_use]
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// Short machine-readable reason for the tier choice. For Shape /
    /// Literal this is a "success" reason (e.g. `"shape:starts-with"`).
    /// For Meta this names the disqualifying feature
    /// (`"unbounded-quantifier"`, `"word-boundary"`, …).
    #[must_use]
    pub fn reason(&self) -> &'static str {
        self.reason
    }
}

// ---------------------------------------------------------------------------
// StrMatcherBuilder
// ---------------------------------------------------------------------------

/// Builder for [`StrMatcher`]. Carries minimum-tier policy and the
/// case-insensitivity flag.
#[derive(Debug, Clone, Default)]
pub struct StrMatcherBuilder {
    min_tier: Option<MatcherTier>,
    on_below_min: OnBelowMin,
    ascii_case_insensitive: bool,
}

impl StrMatcherBuilder {
    /// Create a builder with default options (no minimum tier, no
    /// case-insensitivity).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reject (or warn about) patterns that classify below this tier.
    ///
    /// Tier ordering (highest → lowest): `Shape > Literal > Meta`.
    /// Setting `min_tier(Literal)` allows Shape and Literal but
    /// triggers `on_below_min` for Meta.
    #[must_use]
    pub fn min_tier(mut self, tier: MatcherTier) -> Self {
        self.min_tier = Some(tier);
        self
    }

    /// What to do when a pattern falls below `min_tier`.
    #[must_use]
    pub fn on_below_min(mut self, policy: OnBelowMin) -> Self {
        self.on_below_min = policy;
        self
    }

    /// Build a case-insensitive matcher.
    ///
    /// The pattern is wrapped in `(?i:...)` before parsing. Both the
    /// literal and shape tiers honour the flag via
    /// `aho_corasick::AhoCorasickBuilder::ascii_case_insensitive`;
    /// shape-tier dispatch downgrades to the literal tier when this
    /// flag is set, since `memchr` does not fold case.
    #[must_use]
    pub fn ascii_case_insensitive(mut self, enabled: bool) -> Self {
        self.ascii_case_insensitive = enabled;
        self
    }

    /// Build the matcher.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the pattern fails to parse or violates the
    /// `min_tier` policy with `on_below_min = Reject`.
    pub fn build(&self, pattern: &str) -> Result<StrMatcher, BuildError> {
        let classified = classify::classify(pattern, self.ascii_case_insensitive)?;
        let tier = classified.descriptor.tier;

        if let Some(min) = self.min_tier
            && tier.rank() < min.rank()
        {
            return self.apply_below_min(pattern, min, classified);
        }

        if tier == MatcherTier::Regex {
            warn::on_regex_fallback(
                pattern,
                classified.descriptor.reason,
                classified.descriptor.hint,
                /*force=*/ false,
            );
            metrics_inc_fallback();
        }

        Ok(StrMatcher {
            plan: classified.plan,
            tier,
            pattern: pattern.to_string(),
            reason: classified.descriptor.reason,
        })
    }

    fn apply_below_min(
        &self,
        pattern: &str,
        wanted: MatcherTier,
        classified: Classified,
    ) -> Result<StrMatcher, BuildError> {
        let got = classified.descriptor.tier;
        let reason = classified.descriptor.reason;
        let hint = classified.descriptor.hint;

        match self.on_below_min {
            OnBelowMin::Reject => Err(BuildError::TierTooLow {
                pattern: pattern.to_string(),
                wanted,
                got,
                reason,
                hint,
            }),
            OnBelowMin::Warn => {
                warn::on_regex_fallback(pattern, reason, hint, /*force=*/ true);
                metrics_inc_fallback();
                Ok(StrMatcher {
                    plan: classified.plan,
                    tier: got,
                    pattern: pattern.to_string(),
                    reason,
                })
            }
            OnBelowMin::Allow => {
                if got == MatcherTier::Regex {
                    warn::on_regex_fallback(pattern, reason, hint, /*force=*/ false);
                    metrics_inc_fallback();
                }
                Ok(StrMatcher {
                    plan: classified.plan,
                    tier: got,
                    pattern: pattern.to_string(),
                    reason,
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// StrMatcherSet -- multi-pattern, single AC scan where possible
// ---------------------------------------------------------------------------

/// Multi-pattern matcher.
///
/// Build-time partition:
///
/// - Unanchored `Byte` / `Literal` / `LiteralSet` patterns fold into
///   one shared aho-corasick. One linear scan covers them all --
///   O(M + total_matches).
/// - Anchored patterns (`^foo`, `bar$`, `^baz$`) and `Regex` fall-
///   throughs stay as per-pattern matchers.
///
/// The merged AC runs `MatchKind::LeftmostLongest` -- overlapping
/// literals like `AKIA` vs `AKIA1234` resolve to the longer match.
/// Pattern indices survive the merge: `SetMatch.pattern_idx`
/// always matches the caller's input order.
pub struct StrMatcherSet {
    merged: Option<MergedAc>,
    /// Anchored / regex-tier patterns, paired with input index.
    individual: Vec<(usize, StrMatcher)>,
    /// Tier per input pattern, in input order. Used for `tier_counts`.
    tiers: Vec<MatcherTier>,
}

/// Cross-pattern AC. `pattern_indices[ac_id]` maps an AC pattern ID
/// back to the caller's input index -- one input pattern can
/// contribute multiple literals (alternation, byte-set), so indices
/// repeat.
struct MergedAc {
    ac: aho_corasick::AhoCorasick,
    pattern_indices: Vec<usize>,
}

impl StrMatcherSet {
    /// Compile every pattern. Patterns that violate the builder's
    /// `min_tier` policy with `OnBelowMin::Reject` fail the build for
    /// the entire set -- returning the first failing pattern's error.
    ///
    /// # Errors
    ///
    /// As [`StrMatcherBuilder::build`].
    pub fn new<I, S>(patterns: I) -> Result<Self, BuildError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self::builder().build_set(patterns)
    }

    /// Start a builder.
    #[must_use]
    pub fn builder() -> StrMatcherBuilder {
        StrMatcherBuilder::new()
    }

    /// `true` if any pattern matches anywhere in the haystack.
    #[must_use]
    pub fn is_match(&self, hay: &[u8]) -> bool {
        if let Some(m) = &self.merged
            && m.ac.find(hay).is_some()
        {
            return true;
        }
        self.individual.iter().any(|(_, m)| m.is_match(hay))
    }

    /// Find the earliest match across all patterns.
    ///
    /// Order:
    /// - Lower `start` wins.
    /// - At the same `start`, the merged AC applies
    ///   `MatchKind::LeftmostLongest` *within* its literal set, so
    ///   the longer literal wins among merged patterns (e.g.
    ///   `AKIA1234` over `AKIA`).
    /// - Across the merge boundary (merged-AC hit vs an individual
    ///   anchored / regex matcher at the same `start`), the
    ///   lower `pattern_idx` wins -- input-order preserved.
    #[must_use]
    pub fn earliest_match(&self, hay: &[u8]) -> Option<SetMatch> {
        let mut best: Option<SetMatch> = None;
        if let Some(m) = &self.merged
            && let Some(hit) = m.ac.find(hay)
        {
            best = Some(SetMatch {
                start: hit.start(),
                end: hit.end(),
                pattern_idx: m.pattern_indices[hit.pattern().as_usize()],
            });
        }
        for (idx, matcher) in &self.individual {
            if let Some(found) = matcher.find(hay) {
                let cand = SetMatch {
                    start: found.start,
                    end: found.end,
                    pattern_idx: *idx,
                };
                best = match best {
                    None => Some(cand),
                    Some(b) if cand.start < b.start => Some(cand),
                    Some(b) if cand.start == b.start && cand.pattern_idx < b.pattern_idx => {
                        Some(cand)
                    }
                    Some(b) => Some(b),
                };
            }
        }
        best
    }

    /// Collect every non-overlapping match across all patterns,
    /// sorted by `(start, pattern_idx)`.
    ///
    /// The merged AC runs `MatchKind::LeftmostLongest` internally,
    /// so when two literals overlap within the merged set, the
    /// longer one wins. Across the merge boundary the sort key
    /// gives input-order tie-breaking.
    ///
    /// When multiple patterns match at the same position, the
    /// lower-indexed pattern wins (consistent with the input order).
    /// Matches across different patterns may overlap.
    #[must_use]
    pub fn find_iter(&self, hay: &[u8]) -> std::vec::IntoIter<SetMatch> {
        let mut all: Vec<SetMatch> = Vec::new();
        if let Some(m) = &self.merged {
            for hit in m.ac.find_iter(hay) {
                all.push(SetMatch {
                    start: hit.start(),
                    end: hit.end(),
                    pattern_idx: m.pattern_indices[hit.pattern().as_usize()],
                });
            }
        }
        for (idx, matcher) in &self.individual {
            for hit in matcher.find_iter(hay) {
                all.push(SetMatch {
                    start: hit.start,
                    end: hit.end,
                    pattern_idx: *idx,
                });
            }
        }
        all.sort_by_key(|m| (m.start, m.pattern_idx));
        all.into_iter()
    }

    /// Per-tier population counts: `[Byte, Literal, LiteralSet, Regex]`.
    /// Useful for operator dashboards / quality gates.
    #[must_use]
    pub fn tier_counts(&self) -> [usize; 4] {
        let mut counts = [0_usize; 4];
        for t in &self.tiers {
            let idx = match t {
                MatcherTier::Byte => 0,
                MatcherTier::Literal => 1,
                MatcherTier::LiteralSet => 2,
                MatcherTier::Regex => 3,
            };
            counts[idx] += 1;
        }
        counts
    }

    /// Number of patterns in the set.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tiers.len()
    }

    /// `true` if the set has no patterns.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tiers.is_empty()
    }
}

impl StrMatcherBuilder {
    /// Build a [`StrMatcherSet`] from an iterator of patterns.
    ///
    /// `min_tier` + `on_below_min` apply per pattern. First failure
    /// short-circuits the build. Unanchored literals fold into a
    /// merged AC; anchored / regex patterns stay individual.
    ///
    /// # Errors
    ///
    /// As [`StrMatcherBuilder::build`].
    pub fn build_set<I, S>(&self, patterns: I) -> Result<StrMatcherSet, BuildError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut individual: Vec<(usize, StrMatcher)> = Vec::new();
        let mut merge_lits: Vec<Vec<u8>> = Vec::new();
        let mut pattern_indices: Vec<usize> = Vec::new();
        let mut tiers: Vec<MatcherTier> = Vec::new();

        for (idx, pat) in patterns.into_iter().enumerate() {
            let m = self.build(pat.as_ref())?;
            tiers.push(m.tier);
            match m.plan.merge_literals() {
                Some((lits, ci)) if ci == self.ascii_case_insensitive => {
                    for lit in lits {
                        pattern_indices.push(idx);
                        merge_lits.push(lit);
                    }
                }
                _ => individual.push((idx, m)),
            }
        }

        // AC build is infallible on a pre-validated literal set --
        // empty merge_lits short-circuits to None above, and the
        // classifier already enforces LITERAL_SET_CAP. A failure
        // here would mean a contract violation, so panic.
        let merged = (!merge_lits.is_empty()).then(|| {
            let ac = aho_corasick::AhoCorasickBuilder::new()
                .ascii_case_insensitive(self.ascii_case_insensitive)
                .match_kind(aho_corasick::MatchKind::LeftmostLongest)
                .build(&merge_lits)
                .expect("strmatch: merged AC build failed on a pre-validated literal set");
            MergedAc {
                ac,
                pattern_indices,
            }
        });

        Ok(StrMatcherSet {
            merged,
            individual,
            tiers,
        })
    }
}

// The inherent single-pattern build method already exists above; the
// trait-style impl below disambiguates against the iterator version.
// Rust resolves single-pattern `&str` to the inherent method because
// `IntoIterator<Item = S: AsRef<str>>` is not implemented for `&str`
// (a `&str` doesn't iter into `&str`s).

// ---------------------------------------------------------------------------
// Anti-spam log helper
// ---------------------------------------------------------------------------

mod warn {
    use super::{AtomicBool, AtomicUsize, HashSet, LazyLock, Mutex, Ordering};

    /// Soft cap on distinct WARN-level fall-back log lines per process.
    /// Past this we step down to DEBUG and emit one INFO summary.
    pub(super) const WARN_CAP: usize = 10;

    static WARNED_HASHES: LazyLock<Mutex<HashSet<u64>>> =
        LazyLock::new(|| Mutex::new(HashSet::new()));
    static DISTINCT: AtomicUsize = AtomicUsize::new(0);
    static SUMMARY_EMITTED: AtomicBool = AtomicBool::new(false);

    fn hash_pattern(pattern: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        pattern.hash(&mut h);
        h.finish()
    }

    /// Run the dedup + cap protocol. `force = true` always emits a
    /// WARN (used by `OnBelowMin::Warn` callers who explicitly want
    /// the log line).
    pub(super) fn on_regex_fallback(
        pattern: &str,
        reason: &'static str,
        hint: &'static str,
        force: bool,
    ) {
        let h = hash_pattern(pattern);
        let already_warned = {
            let mut set = WARNED_HASHES.lock().unwrap_or_else(|e| e.into_inner());
            !set.insert(h)
        };

        if already_warned && !force {
            // Already warned about this pattern; respect the dedup.
            return;
        }

        let n = if force {
            // Force path doesn't count toward the cap.
            DISTINCT.load(Ordering::Relaxed)
        } else {
            DISTINCT.fetch_add(1, Ordering::Relaxed) + 1
        };

        if force || n <= WARN_CAP {
            tracing::warn!(
                target: "hyperi_rustlib::strmatch",
                pattern,
                reason,
                hint,
                "pattern falls through to regex engine on hot path"
            );
        } else {
            tracing::debug!(
                target: "hyperi_rustlib::strmatch",
                pattern,
                reason,
                hint,
                "regex fallback (WARN suppressed past cap)"
            );
        }

        if !force && n == WARN_CAP + 1 && !SUMMARY_EMITTED.swap(true, Ordering::Relaxed) {
            tracing::info!(
                target: "hyperi_rustlib::strmatch",
                cap = WARN_CAP,
                "{}+ distinct patterns have fallen through to the regex engine; \
                 further fall-throughs log at DEBUG. Inspect StrMatcher::tier() / \
                 StrMatcherSet::tier_counts() at runtime, or scrape the \
                 hyperi_strmatch_regex_fallback_total metric.",
                WARN_CAP,
            );
        }
    }

    /// Reset state for tests. Symbol exists only under `cfg(test)` so
    /// production callers can't accidentally use it.
    #[cfg(test)]
    pub(super) fn reset_for_tests() {
        WARNED_HASHES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        DISTINCT.store(0, Ordering::Relaxed);
        SUMMARY_EMITTED.store(false, Ordering::Relaxed);
    }
}

#[inline]
fn metrics_inc_fallback() {
    #[cfg(feature = "metrics")]
    metrics::counter!("hyperi_strmatch_regex_fallback_total").increment(1);
}

// Re-export the warn-state reset for integration tests inside the
// crate. Hidden from doc and not part of the public API.
#[cfg(test)]
#[doc(hidden)]
pub fn reset_warn_state_for_tests() {
    warn::reset_for_tests();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
