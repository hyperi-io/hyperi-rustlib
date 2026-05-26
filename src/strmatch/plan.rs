// Project:   hyperi-rustlib
// File:      src/strmatch/plan.rs
// Purpose:   Plan enum + match-time dispatch for strmatch
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! The `Plan` chosen by the classifier and the single-match dispatch
//! that fires it on a haystack.
//!
//! Three variants — `LiteralOnly` / `Shape` / `Meta` — map directly to
//! the public [`super::MatcherTier`]. Match-time dispatch is a single
//! `match` per call; each arm is one or two instructions for the
//! literal and shape tiers.

use aho_corasick::AhoCorasick;
use regex_automata::meta;

use super::Match;

/// Where in the haystack a match must occur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Anchor {
    /// `/foo/` — anywhere in the haystack.
    Anywhere,
    /// `/^foo/` — match must start at byte 0.
    AtStart,
    /// `/foo$/` — match must end at `hay.len()`.
    AtEnd,
    /// `/^foo$/` — match must cover the whole haystack.
    Exact,
}

/// Up-to-3-byte set, used for `[xy]` and `[xyz]` shape patterns. The
/// `n` field carries the live count; bytes beyond `n` are
/// uninitialised semantically (and zero-filled in practice).
#[derive(Debug, Clone, Copy)]
pub(crate) struct SmallByteSet {
    bytes: [u8; 3],
    n: u8,
}

impl SmallByteSet {
    pub(crate) fn new(src: &[u8]) -> Self {
        assert!(
            !src.is_empty() && src.len() <= 3,
            "SmallByteSet expects 1..=3 bytes, got {}",
            src.len()
        );
        let mut bytes = [0_u8; 3];
        bytes[..src.len()].copy_from_slice(src);
        Self {
            bytes,
            // SAFETY-equivalent: bounds enforced by the assert above.
            n: u8::try_from(src.len()).expect("1..=3 bounds checked above"),
        }
    }

    #[inline]
    pub(crate) fn contains_any_in(self, hay: &[u8]) -> Option<usize> {
        match self.n {
            1 => memchr::memchr(self.bytes[0], hay),
            2 => memchr::memchr2(self.bytes[0], self.bytes[1], hay),
            3 => memchr::memchr3(self.bytes[0], self.bytes[1], self.bytes[2], hay),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn n(self) -> u8 {
        self.n
    }

    #[inline]
    pub(crate) fn byte(self, i: usize) -> u8 {
        self.bytes[i]
    }
}

/// One-of-N byte-level shape that we can dispatch without invoking the
/// regex engine.
///
/// `Contains(Finder)` is boxed because the `memmem::Finder` carries
/// pre-computed search tables that can be hundreds of bytes; the other
/// variants are small. Boxing keeps `ShapeOp` itself one cache line so
/// dispatch doesn't pay for cold variant data.
#[derive(Debug, Clone)]
pub(crate) enum ShapeOp {
    /// `/x/` — single byte, anywhere.
    ContainsByte(u8),
    /// `/[xy]/` or `/[xyz]/`.
    ByteSet(SmallByteSet),
    /// `/foo/` — multi-byte literal, anywhere. Pre-compiled needle.
    Contains(Box<memchr::memmem::Finder<'static>>),
    /// `/^foo/` — anchored at start.
    StartsWith(Box<[u8]>),
    /// `/foo$/` — anchored at end.
    EndsWith(Box<[u8]>),
    /// `/^foo$/` — must cover the whole haystack.
    ExactMatch(Box<[u8]>),
}

/// The dispatch plan picked by the classifier.
///
/// `AhoCorasick` and `meta::Regex` are large structures (each carries
/// internal heap allocations + several enums in the hundreds of bytes).
/// Boxing them keeps `Plan` itself one cache line — the Shape variant
/// stays inline because it's used directly on the hottest path.
pub(crate) enum Plan {
    /// `aho-corasick` over a finite, exact literal set with optional
    /// uniform anchor. Regex engine is never invoked at match time.
    LiteralOnly {
        ac: Box<AhoCorasick>,
        anchor: Anchor,
    },

    /// Shape tier: single-pattern direct byte dispatch.
    Shape(ShapeOp),

    /// Fall-through: the regex-automata meta engine handles
    /// everything we couldn't reduce. Boxed for `Plan` size discipline.
    Meta(Box<meta::Regex>),
}

impl Plan {
    /// True if the haystack contains a match per this plan.
    #[inline]
    pub(crate) fn is_match(&self, hay: &[u8]) -> bool {
        match self {
            Self::LiteralOnly { ac, anchor } => match_set_is(ac, *anchor, hay),
            Self::Shape(op) => match_shape_is(op, hay),
            Self::Meta(r) => r.is_match(hay),
        }
    }

    /// Find the first match per this plan, returning byte offsets.
    #[inline]
    pub(crate) fn find(&self, hay: &[u8]) -> Option<Match> {
        match self {
            Self::LiteralOnly { ac, anchor } => match_set_find(ac, *anchor, hay),
            Self::Shape(op) => match_shape_find(op, hay),
            Self::Meta(r) => r.find(hay).map(|m| Match {
                start: m.start(),
                end: m.end(),
            }),
        }
    }

    /// Collect every non-overlapping match per this plan into `out`.
    /// Eager (vs lazy iterator) because the inner iterator types have
    /// incompatible lifetimes and dyn-dispatch would cost a vtable
    /// call per element; eager collection costs one allocation per
    /// `find_iter` call and amortises well over typical scrubber-style
    /// "redact all matches" workloads (0-3 matches per log line).
    pub(crate) fn collect_matches(&self, hay: &[u8], out: &mut Vec<Match>) {
        match self {
            Self::LiteralOnly { ac, anchor } => collect_set(ac, *anchor, hay, out),
            Self::Shape(op) => collect_shape(op, hay, out),
            Self::Meta(r) => {
                for m in r.find_iter(hay) {
                    out.push(Match {
                        start: m.start(),
                        end: m.end(),
                    });
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// LiteralOnly dispatch — one AC scan, then optional position check
// ---------------------------------------------------------------------------

#[inline]
fn match_set_is(ac: &AhoCorasick, anchor: Anchor, hay: &[u8]) -> bool {
    match anchor {
        Anchor::Anywhere => ac.find(hay).is_some(),
        // `ac.find` returns the LEFTMOST match. The smallest possible start
        // position is 0, so if any match is at position 0, the leftmost is
        // at position 0. Checking the leftmost is sufficient for `AtStart`.
        Anchor::AtStart => ac.find(hay).is_some_and(|m| m.start() == 0),
        // The leftmost match is not necessarily the rightmost — its end may
        // be well short of `hay.len()` even when a later match ends exactly
        // there. Walk every match.
        Anchor::AtEnd => ac.find_iter(hay).any(|m| m.end() == hay.len()),
        // Same problem as `AtEnd`: the leftmost match might satisfy only
        // the start constraint, while a different match (or none) satisfies
        // both. Walk every match.
        Anchor::Exact => ac
            .find_iter(hay)
            .any(|m| m.start() == 0 && m.end() == hay.len()),
    }
}

#[inline]
fn match_set_find(ac: &AhoCorasick, anchor: Anchor, hay: &[u8]) -> Option<Match> {
    match anchor {
        Anchor::Anywhere => ac.find(hay).map(|m| Match {
            start: m.start(),
            end: m.end(),
        }),
        Anchor::AtStart => ac.find(hay).filter(|m| m.start() == 0).map(|m| Match {
            start: m.start(),
            end: m.end(),
        }),
        // Find any match that ends at `hay.len()` — not just the leftmost.
        Anchor::AtEnd => ac
            .find_iter(hay)
            .find(|m| m.end() == hay.len())
            .map(|m| Match {
                start: m.start(),
                end: m.end(),
            }),
        // Find any match that satisfies both endpoints. Iterating is
        // bounded by the number of AC hits in the haystack and short-
        // circuits on the first satisfying match.
        Anchor::Exact => ac
            .find_iter(hay)
            .find(|m| m.start() == 0 && m.end() == hay.len())
            .map(|m| Match {
                start: m.start(),
                end: m.end(),
            }),
    }
}

// ---------------------------------------------------------------------------
// Shape dispatch — single match arm per shape, 1–3 instructions per
// ---------------------------------------------------------------------------

#[inline]
fn match_shape_is(op: &ShapeOp, hay: &[u8]) -> bool {
    match op {
        ShapeOp::ContainsByte(b) => memchr::memchr(*b, hay).is_some(),
        ShapeOp::ByteSet(s) => s.contains_any_in(hay).is_some(),
        ShapeOp::Contains(f) => f.find(hay).is_some(),
        ShapeOp::StartsWith(lit) => hay.starts_with(lit),
        ShapeOp::EndsWith(lit) => hay.ends_with(lit),
        ShapeOp::ExactMatch(lit) => hay == lit.as_ref(),
    }
}

#[inline]
fn match_shape_find(op: &ShapeOp, hay: &[u8]) -> Option<Match> {
    match op {
        ShapeOp::ContainsByte(b) => memchr::memchr(*b, hay).map(|i| Match {
            start: i,
            end: i + 1,
        }),
        ShapeOp::ByteSet(s) => s.contains_any_in(hay).map(|i| Match {
            start: i,
            end: i + 1,
        }),
        ShapeOp::Contains(f) => f.find(hay).map(|i| Match {
            start: i,
            end: i + f.needle().len(),
        }),
        ShapeOp::StartsWith(lit) => hay.starts_with(lit).then_some(Match {
            start: 0,
            end: lit.len(),
        }),
        // `then_some` eagerly evaluates the Match struct, so the subtraction
        // `hay.len() - lit.len()` underflows (panics in debug, wraps in release)
        // when the haystack is shorter than the literal — even though
        // `ends_with` returns false. `then(closure)` defers evaluation.
        ShapeOp::EndsWith(lit) => hay.ends_with(lit).then(|| Match {
            start: hay.len() - lit.len(),
            end: hay.len(),
        }),
        ShapeOp::ExactMatch(lit) => (hay == lit.as_ref()).then_some(Match {
            start: 0,
            end: hay.len(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Collect-iter dispatch — non-overlapping matches into a caller-owned Vec
// ---------------------------------------------------------------------------

fn collect_set(ac: &AhoCorasick, anchor: Anchor, hay: &[u8], out: &mut Vec<Match>) {
    // AhoCorasick::find_iter yields non-overlapping leftmost-first
    // matches. For non-`Anywhere` anchors, filter by the position
    // constraint; in practice an anchored set produces at most one
    // valid match (since the position is fixed), but we let the
    // iterator yield naturally and filter.
    for m in ac.find_iter(hay) {
        let keep = match anchor {
            Anchor::Anywhere => true,
            Anchor::AtStart => m.start() == 0,
            Anchor::AtEnd => m.end() == hay.len(),
            Anchor::Exact => m.start() == 0 && m.end() == hay.len(),
        };
        if keep {
            out.push(Match {
                start: m.start(),
                end: m.end(),
            });
            // For anchored sets there can be at most one match.
            if !matches!(anchor, Anchor::Anywhere) {
                break;
            }
        }
    }
}

fn collect_shape(op: &ShapeOp, hay: &[u8], out: &mut Vec<Match>) {
    match op {
        ShapeOp::ContainsByte(b) => {
            for i in memchr::memchr_iter(*b, hay) {
                out.push(Match {
                    start: i,
                    end: i + 1,
                });
            }
        }
        ShapeOp::ByteSet(s) => match s.n() {
            1 => {
                for i in memchr::memchr_iter(s.byte(0), hay) {
                    out.push(Match {
                        start: i,
                        end: i + 1,
                    });
                }
            }
            2 => {
                for i in memchr::memchr2_iter(s.byte(0), s.byte(1), hay) {
                    out.push(Match {
                        start: i,
                        end: i + 1,
                    });
                }
            }
            3 => {
                for i in memchr::memchr3_iter(s.byte(0), s.byte(1), s.byte(2), hay) {
                    out.push(Match {
                        start: i,
                        end: i + 1,
                    });
                }
            }
            _ => {}
        },
        ShapeOp::Contains(f) => {
            for i in f.find_iter(hay) {
                out.push(Match {
                    start: i,
                    end: i + f.needle().len(),
                });
            }
        }
        // Anchored shapes have at most one match; reuse the single-find logic.
        ShapeOp::StartsWith(_) | ShapeOp::EndsWith(_) | ShapeOp::ExactMatch(_) => {
            if let Some(m) = match_shape_find(op, hay) {
                out.push(m);
            }
        }
    }
}
