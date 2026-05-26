// Project:   hyperi-rustlib
// File:      src/strmatch/classify.rs
// Purpose:   Classify a regex pattern into a strmatch tier
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! HIR-walking classifier that decides which [`super::plan::Plan`] best
//! handles a given regex pattern.
//!
//! Three outcomes:
//!
//! - **Shape**: the pattern reduces to a single byte / byte-set /
//!   anchored or unanchored literal. Dispatched via direct `memchr` /
//!   `starts_with` / `ends_with` / `memmem` calls at match time.
//! - **LiteralOnly**: an alternation (or extracted prefix set) of
//!   *exact* literals with no look-around assertions. Dispatched via a
//!   single `aho_corasick::AhoCorasick` automaton. The regex engine is
//!   never invoked at match time.
//! - **Meta**: anything else. Dispatched via
//!   `regex_automata::meta::Regex`, which has its own internal
//!   prefilter pipeline (memchr → Teddy → AC → NFA/DFA).
//!
//! The classifier never returns "this regex is invalid" — that's the
//! parser's job. It returns a [`Plan`] and a [`Descriptor`] explaining
//! which tier was chosen and (if Meta) why.

use aho_corasick::{AhoCorasick, MatchKind};
use regex_syntax::hir::literal::{ExtractKind, Extractor};
use regex_syntax::hir::{Class, Hir, HirKind, Literal, Look};

use super::plan::{Anchor, Plan, ShapeOp, SmallByteSet};
use super::{BuildError, MatcherTier};

/// Soft-cap on the size of a character class we'll attempt to reduce to
/// a small byte set. `\w` is ~80 codepoints; `\p{L}` is tens of
/// thousands. Past this limit we fall through to Meta rather than
/// produce a giant byte set that scans no faster than a regex.
const CLASS_BYTE_CAP: usize = 16;

/// Soft-cap on the number of literals we'll lift into an Aho-Corasick
/// automaton. Beyond this the AC build cost dominates and the meta
/// engine's compiled NFA is usually competitive.
const LITERAL_SET_CAP: usize = 2048;

/// What the classifier picked, plus the human-readable trail.
pub struct Classified {
    pub plan: Plan,
    pub descriptor: Descriptor,
}

/// Why the planner chose what it chose.
#[derive(Debug, Clone)]
pub struct Descriptor {
    pub tier: MatcherTier,
    /// Short machine-readable reason. For LiteralOnly / Shape this is
    /// `"shape:{name}"` / `"literal-only:{n}"`. For Meta this names the
    /// disqualifying feature.
    pub reason: &'static str,
    /// Human-readable hint pointing at a fix (Rustc-style).
    /// Empty `""` when no hint applies (Shape / LiteralOnly success
    /// paths don't need one).
    pub hint: &'static str,
}

/// Classify `pattern`. Returns `Err(BuildError::Syntax)` only if the
/// pattern fails to parse as regex.
///
/// `case_insensitive` is honoured at *match time* via
/// `aho_corasick::AhoCorasickBuilder::ascii_case_insensitive`, not via
/// HIR rewriting. We deliberately do NOT wrap the pattern in `(?i:…)`
/// before parsing — that would expand literals into per-byte case
/// classes and defeat simple-shape detection.
///
/// When `case_insensitive` is set:
/// - The Shape tier is unavailable (`memchr` does not fold case).
/// - Single-literal patterns are routed to a one-element AC with the
///   case-insensitive flag set.
/// - Alternation, anchored alternation, and extractor-fallback all
///   propagate the flag to the AC builder.
pub fn classify(pattern: &str, case_insensitive: bool) -> Result<Classified, BuildError> {
    if pattern.is_empty() {
        return Err(BuildError::Empty);
    }

    let hir = regex_syntax::parse(pattern).map_err(|e| BuildError::Syntax {
        pattern: pattern.to_string(),
        source: Box::new(e),
        hint: SYNTAX_HINT,
    })?;

    // 1. Hard rejection: any feature we can't safely reduce.
    if let Some((reason, hint)) = hard_reject(&hir) {
        return Ok(meta_plan(pattern, reason, hint));
    }

    // 2. Single-pattern simple shape (skipped when case-insensitive —
    //    memchr/memmem can't fold case; we route those through a
    //    one-element AC below instead).
    if !case_insensitive && let Some(shape) = try_simple_shape(&hir) {
        let descriptor = Descriptor {
            tier: tier_for_shape(&shape),
            reason: shape_reason(&shape),
            hint: "",
        };
        return Ok(Classified {
            plan: Plan::Shape(shape),
            descriptor,
        });
    }

    // 3. Top-level alternation of literals (and Concat-wrapped
    //    anchored alternation: `^(?:foo|bar)$`).
    if let Some((ac, anchor, literals)) = try_literal_alternation(&hir, case_insensitive) {
        let descriptor = Descriptor {
            tier: MatcherTier::LiteralSet,
            reason: "literal-only:alternation",
            hint: "",
        };
        return Ok(Classified {
            plan: Plan::LiteralOnly {
                ac: Box::new(ac),
                anchor,
                literals: literals_to_boxed(literals),
                case_insensitive,
            },
            descriptor,
        });
    }

    // 4. Case-insensitive single-literal fast path. We've already
    //    refused the Shape tier above, but the pattern may still
    //    reduce to a single literal byte sequence — route it through
    //    a one-element AC so the case-folding happens at match time.
    if case_insensitive
        && let Some((ac, anchor, literals)) = try_single_literal_for_case_insensitive(&hir)
    {
        let descriptor = Descriptor {
            tier: MatcherTier::LiteralSet,
            reason: "literal-only:case-insensitive",
            hint: "",
        };
        return Ok(Classified {
            plan: Plan::LiteralOnly {
                ac: Box::new(ac),
                anchor,
                literals: literals_to_boxed(literals),
                case_insensitive,
            },
            descriptor,
        });
    }

    // 5. Extractor fallback: literals are exact and finite.
    if let Some((ac, anchor, literals)) = try_extractor_fallback(&hir, case_insensitive) {
        let descriptor = Descriptor {
            tier: MatcherTier::LiteralSet,
            reason: "literal-only:extracted",
            hint: "",
        };
        return Ok(Classified {
            plan: Plan::LiteralOnly {
                ac: Box::new(ac),
                anchor,
                literals: literals_to_boxed(literals),
                case_insensitive,
            },
            descriptor,
        });
    }

    // 6. Fall through to the meta engine.
    Ok(meta_plan(
        pattern,
        "complex-shape",
        "the pattern combines anchors, classes, and quantifiers in a way that \
         prevents safe literal reduction; consider splitting into smaller \
         patterns or accepting Meta-tier cost",
    ))
}

// ---------------------------------------------------------------------------
// Hard-reject features
// ---------------------------------------------------------------------------

const SYNTAX_HINT: &str = "check the regex syntax and ensure it parses under \
                           the regex-syntax crate's default flags";

fn hard_reject(hir: &Hir) -> Option<(&'static str, &'static str)> {
    use HirKind as K;
    match hir.kind() {
        K::Look(l) => match l {
            // Start/End anchors are fine — we handle them in the
            // simple-shape detector. Everything else (word boundaries,
            // multi-line anchors, lookarounds) prevents safe reduction.
            Look::Start | Look::End => None,
            Look::WordAscii
            | Look::WordAsciiNegate
            | Look::WordUnicode
            | Look::WordUnicodeNegate
            | Look::WordStartAscii
            | Look::WordEndAscii
            | Look::WordStartUnicode
            | Look::WordEndUnicode
            | Look::WordStartHalfAscii
            | Look::WordEndHalfAscii
            | Look::WordStartHalfUnicode
            | Look::WordEndHalfUnicode => Some((
                "word-boundary",
                "remove the word-boundary assertion if the surrounding context \
                 is known, or accept Meta-tier cost",
            )),
            Look::StartLF | Look::EndLF | Look::StartCRLF | Look::EndCRLF => Some((
                "multiline-anchor",
                "split the haystack by line and use a non-multiline pattern, \
                 or accept Meta-tier cost",
            )),
        },
        K::Repetition(r) => {
            // Quantifiers with variable length (unbounded * + ? or
            // {n,} or {n,m} with n != m) prevent literal extraction
            // from yielding finite exact literals.
            if r.max.is_none_or(|max| max != r.min) {
                Some((
                    "unbounded-quantifier",
                    "bound the quantifier (e.g. {1,64}) so literal extraction \
                     can yield finite exact prefixes, or accept Meta-tier cost",
                ))
            } else {
                hard_reject(&r.sub)
            }
        }
        K::Capture(c) => {
            // Anonymous captures are fine if we walk into their body —
            // we just don't expose captures in our public API. But the
            // important case is back-references, which regex-syntax
            // forbids at parse time anyway (the default parser
            // rejects `(\1)`). So traverse the sub-Hir; captures
            // themselves aren't disqualifying.
            hard_reject(&c.sub)
        }
        K::Class(cls) => {
            if class_byte_count(cls).is_none() {
                Some((
                    "unicode-class",
                    "if ASCII-only is acceptable, use (?-u) to disable unicode \
                     classes, or accept Meta-tier cost",
                ))
            } else {
                None
            }
        }
        K::Concat(parts) | K::Alternation(parts) => parts.iter().find_map(hard_reject),
        K::Empty | K::Literal(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Simple-shape detection
// ---------------------------------------------------------------------------

fn try_simple_shape(hir: &Hir) -> Option<ShapeOp> {
    match hir.kind() {
        HirKind::Literal(lit) => simple_literal(&lit.0),
        HirKind::Class(cls) => simple_class(cls),
        HirKind::Concat(parts) => anchored_literal(parts),
        // Capture wrapping a simple shape: walk in.
        HirKind::Capture(c) => try_simple_shape(&c.sub),
        _ => None,
    }
}

fn simple_literal(bytes: &[u8]) -> Option<ShapeOp> {
    match bytes.len() {
        0 => None,
        1 => Some(ShapeOp::ContainsByte(bytes[0])),
        _ => Some(ShapeOp::Contains(Box::new(
            memchr::memmem::Finder::new(bytes).into_owned(),
        ))),
    }
}

fn simple_class(cls: &Class) -> Option<ShapeOp> {
    let bytes = collect_class_bytes(cls)?;
    match bytes.len() {
        0 => None,
        1 => Some(ShapeOp::ContainsByte(bytes[0])),
        n if n <= 3 => Some(ShapeOp::ByteSet(SmallByteSet::new(&bytes))),
        _ => None,
    }
}

fn anchored_literal(parts: &[Hir]) -> Option<ShapeOp> {
    let (anchor_start, body, anchor_end) = strip_anchors(parts)?;
    let body_bytes = collect_literal_bytes(body)?;
    if body_bytes.is_empty() {
        return None;
    }
    match (anchor_start, anchor_end) {
        (true, true) => Some(ShapeOp::ExactMatch(body_bytes.into_boxed_slice())),
        (true, false) => Some(ShapeOp::StartsWith(body_bytes.into_boxed_slice())),
        (false, true) => Some(ShapeOp::EndsWith(body_bytes.into_boxed_slice())),
        (false, false) => simple_literal(&body_bytes),
    }
}

/// Split a `Concat` into `(has_start_anchor, body, has_end_anchor)`.
///
/// Returns `None` if the structure doesn't fit the shape "optional ^ +
/// body + optional $" — e.g. multiple disjoint literals inside the
/// Concat that aren't combineable.
fn strip_anchors(parts: &[Hir]) -> Option<(bool, &[Hir], bool)> {
    let (mut lo, mut hi) = (0_usize, parts.len());
    let start = parts
        .first()
        .is_some_and(|p| matches!(p.kind(), HirKind::Look(Look::Start)));
    if start {
        lo += 1;
    }
    let end = parts
        .last()
        .is_some_and(|p| matches!(p.kind(), HirKind::Look(Look::End)));
    if end {
        hi -= 1;
    }
    if hi <= lo {
        return None;
    }
    Some((start, &parts[lo..hi], end))
}

/// Try to collect a contiguous run of literal-only `Hir` nodes into a
/// flat byte string. Returns `None` if any non-literal node is
/// encountered.
fn collect_literal_bytes(parts: &[Hir]) -> Option<Vec<u8>> {
    if parts.is_empty() {
        return None;
    }
    let mut out: Vec<u8> = Vec::new();
    for p in parts {
        match p.kind() {
            HirKind::Literal(Literal(bytes)) => out.extend_from_slice(bytes),
            HirKind::Capture(c) => {
                // Single inner literal inside a capture is still a
                // literal for our purposes.
                let inner = capture_literal_bytes(c)?;
                out.extend_from_slice(&inner);
            }
            _ => return None,
        }
    }
    Some(out)
}

fn capture_literal_bytes(c: &regex_syntax::hir::Capture) -> Option<Vec<u8>> {
    match c.sub.kind() {
        HirKind::Literal(Literal(bytes)) => Some(bytes.to_vec()),
        HirKind::Concat(parts) => collect_literal_bytes(parts),
        _ => None,
    }
}

/// Reduce a `Class` to its byte set, or `None` if it's not byte-shaped
/// or exceeds [`CLASS_BYTE_CAP`].
fn class_byte_count(cls: &Class) -> Option<usize> {
    let bytes = collect_class_bytes(cls)?;
    Some(bytes.len())
}

fn collect_class_bytes(cls: &Class) -> Option<Vec<u8>> {
    match cls {
        Class::Bytes(b) => {
            let mut out = Vec::new();
            for r in b.iter() {
                for byte in r.start()..=r.end() {
                    out.push(byte);
                    if out.len() > CLASS_BYTE_CAP {
                        return None;
                    }
                }
            }
            Some(out)
        }
        Class::Unicode(u) => {
            // Only succeed if every codepoint is ASCII (<= 0x7F).
            let mut out = Vec::new();
            for r in u.iter() {
                let start = r.start() as u32;
                let end = r.end() as u32;
                if end > 0x7F {
                    return None;
                }
                for cp in start..=end {
                    // `cp <= 0x7F` enforced just above, so the cast
                    // cannot truncate.
                    out.push(u8::try_from(cp).expect("ASCII range checked above"));
                    if out.len() > CLASS_BYTE_CAP {
                        return None;
                    }
                }
            }
            Some(out)
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level literal alternation
// ---------------------------------------------------------------------------

fn try_literal_alternation(
    hir: &Hir,
    case_insensitive: bool,
) -> Option<(AhoCorasick, Anchor, Vec<Vec<u8>>)> {
    // The pattern shape we accept here:
    //   Alternation([foo, bar])             → Anchor::Anywhere
    //   Concat([^, Alternation, $])         → Anchor inferred from outer ^/$
    //   Concat([^, Alternation])            → Anchor::AtStart
    //   Concat([Alternation, $])            → Anchor::AtEnd
    // We strip the outer anchors first, then require the inner node to
    // be an Alternation whose branches are all literals (with no
    // additional per-branch anchors, which would conflict with the
    // outer ones).
    let (outer_anchor, inner) = strip_outer_concat_anchors(hir)?;
    let HirKind::Alternation(branches) = inner.kind() else {
        return None;
    };
    if branches.is_empty() {
        return None;
    }

    let mut literals: Vec<Vec<u8>> = Vec::with_capacity(branches.len());
    for branch in branches {
        let body_bytes = match branch.kind() {
            HirKind::Literal(Literal(b)) => b.to_vec(),
            HirKind::Concat(parts) => collect_literal_bytes(parts)?,
            HirKind::Capture(c) => capture_literal_bytes(c)?,
            _ => return None,
        };
        if body_bytes.is_empty() {
            return None;
        }
        literals.push(body_bytes);
    }

    if literals.len() < 2 || literals.len() > LITERAL_SET_CAP {
        return None;
    }

    let ac = AhoCorasick::builder()
        .match_kind(MatchKind::LeftmostFirst)
        .ascii_case_insensitive(case_insensitive)
        .build(&literals)
        .ok()?;

    Some((ac, outer_anchor, literals))
}

/// Strip optional `^` start and `$` end anchors from a top-level
/// `Concat`. Returns `(anchor, inner_hir)`. If the HIR is not a Concat
/// (i.e. the user passed a bare alternation or literal), returns
/// `(Anywhere, hir)` unchanged.
fn strip_outer_concat_anchors(hir: &Hir) -> Option<(Anchor, &Hir)> {
    let HirKind::Concat(parts) = hir.kind() else {
        return Some((Anchor::Anywhere, hir));
    };
    let (start, body, end) = strip_anchors(parts)?;
    let anchor = match (start, end) {
        (true, true) => Anchor::Exact,
        (true, false) => Anchor::AtStart,
        (false, true) => Anchor::AtEnd,
        (false, false) => Anchor::Anywhere,
    };
    if body.len() == 1 {
        Some((anchor, &body[0]))
    } else {
        // Multi-element body — synthesise a Concat representation by
        // returning None. The caller can route through the extractor
        // fallback instead.
        None
    }
}

/// For the case-insensitive path: accept a top-level literal (possibly
/// anchored) and route it through a one-element AC so the case-fold
/// happens at match time.
fn try_single_literal_for_case_insensitive(
    hir: &Hir,
) -> Option<(AhoCorasick, Anchor, Vec<Vec<u8>>)> {
    let (anchor, inner) = strip_outer_concat_anchors(hir)?;
    let body_bytes = match inner.kind() {
        HirKind::Literal(Literal(b)) => b.to_vec(),
        HirKind::Concat(parts) => collect_literal_bytes(parts)?,
        HirKind::Capture(c) => capture_literal_bytes(c)?,
        _ => return None,
    };
    if body_bytes.is_empty() {
        return None;
    }
    let ac = AhoCorasick::builder()
        .match_kind(MatchKind::LeftmostFirst)
        .ascii_case_insensitive(true)
        .build([&body_bytes])
        .ok()?;
    Some((ac, anchor, vec![body_bytes]))
}

// ---------------------------------------------------------------------------
// Extractor fallback (last-chance literal-only path)
// ---------------------------------------------------------------------------

fn try_extractor_fallback(
    hir: &Hir,
    case_insensitive: bool,
) -> Option<(AhoCorasick, Anchor, Vec<Vec<u8>>)> {
    let mut ex = Extractor::new();
    ex.kind(ExtractKind::Prefix);
    ex.limit_class(CLASS_BYTE_CAP);
    ex.limit_total(LITERAL_SET_CAP);
    let seq = ex.extract(hir);

    // Require the seq to be exact (every literal terminates a match)
    // and finite. Inexact sequences would need engine re-verification,
    // so they don't qualify for the "skip engine" path.
    if !seq.is_exact() {
        return None;
    }
    let literals = seq.literals()?;
    if literals.len() < 2 || literals.len() > LITERAL_SET_CAP {
        return None;
    }
    let owned: Vec<Vec<u8>> = literals.iter().map(|l| l.as_bytes().to_vec()).collect();
    let bytes: Vec<&[u8]> = owned.iter().map(Vec::as_slice).collect();
    let ac = AhoCorasick::builder()
        .match_kind(MatchKind::LeftmostFirst)
        .ascii_case_insensitive(case_insensitive)
        .build(&bytes)
        .ok()?;
    Some((ac, Anchor::Anywhere, owned))
}

// ---------------------------------------------------------------------------
// Meta engine fall-through
// ---------------------------------------------------------------------------

fn meta_plan(pattern: &str, reason: &'static str, hint: &'static str) -> Classified {
    use regex_automata::meta;

    // We've already parsed the pattern with `regex_syntax::parse` in
    // the caller, so `meta::Regex::new` will accept the same input
    // (regex-automata wraps the same parser). The meta engine has its
    // own internal `(?i)` handling — no wrapper required here.
    let meta = meta::Regex::new(pattern)
        .expect("meta::Regex::new should not fail after regex_syntax::parse succeeded");
    Classified {
        plan: Plan::Meta(Box::new(meta)),
        descriptor: Descriptor {
            tier: MatcherTier::Regex,
            reason,
            hint,
        },
    }
}

// ---------------------------------------------------------------------------
// Reason strings for the shape tier (no hint needed — success path).
// ---------------------------------------------------------------------------

fn shape_reason(shape: &ShapeOp) -> &'static str {
    match shape {
        ShapeOp::ContainsByte(_) => "shape:contains-byte",
        ShapeOp::ByteSet(_) => "shape:byte-set",
        ShapeOp::Contains(_) => "shape:contains-literal",
        ShapeOp::StartsWith(_) => "shape:starts-with",
        ShapeOp::EndsWith(_) => "shape:ends-with",
        ShapeOp::ExactMatch(_) => "shape:exact-match",
    }
}

/// Map a `ShapeOp` to the public [`MatcherTier`]. Single-byte
/// dispatch (memchr, hay.first/last, hay == [b]) lands in
/// [`MatcherTier::Byte`]; multi-byte literals (memmem, multi-byte
/// starts_with/ends_with/eq) land in [`MatcherTier::Literal`].
fn tier_for_shape(shape: &ShapeOp) -> MatcherTier {
    match shape {
        ShapeOp::ContainsByte(_) | ShapeOp::ByteSet(_) => MatcherTier::Byte,
        ShapeOp::StartsWith(b) | ShapeOp::EndsWith(b) | ShapeOp::ExactMatch(b) if b.len() == 1 => {
            MatcherTier::Byte
        }
        ShapeOp::Contains(_)
        | ShapeOp::StartsWith(_)
        | ShapeOp::EndsWith(_)
        | ShapeOp::ExactMatch(_) => MatcherTier::Literal,
    }
}

/// Compact `Vec<Vec<u8>>` for storage on `Plan::LiteralOnly`.
fn literals_to_boxed(lits: Vec<Vec<u8>>) -> Box<[Box<[u8]>]> {
    lits.into_iter()
        .map(Vec::into_boxed_slice)
        .collect::<Vec<_>>()
        .into_boxed_slice()
}
