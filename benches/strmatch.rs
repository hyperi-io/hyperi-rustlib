// Project:   hyperi-rustlib
// File:      benches/strmatch.rs
// Purpose:   Criterion benchmarks proving the strmatch cost budgets
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Cost-budget verification for the four strmatch tiers.
//!
//! Targets (per `StrMatcher::is_match` call on a ~200-byte haystack):
//!
//! - **Byte** (memchr / single-byte starts/ends/eq): ≤ 30 ns
//! - **Literal** (memmem / multi-byte starts/ends/eq): ≤ 200 ns
//! - **LiteralSet** (AhoCorasick over many literals): ≤ 500 ns
//! - **Regex** (regex-automata fallback): bounded by the engine itself
//!
//! Run with `cargo bench --bench strmatch --features strmatch`.

use criterion::{Criterion, Throughput, criterion_group, criterion_main};

use hyperi_rustlib::strmatch::{MatcherTier, StrMatcher, StrMatcherSet};

const HAYSTACK_SHORT: &[u8] =
    b"2026-05-13T11:00:00.123+11:00 INFO request_id=req_abc123 user=alice action=login src=10.0.0.1";

const HAYSTACK_LONG: &[u8] = include_bytes!("../tests/fixtures/log_line.txt.fixture");

fn bench_byte_tier(c: &mut Criterion) {
    let mut g = c.benchmark_group("strmatch_byte");
    g.throughput(Throughput::Bytes(HAYSTACK_SHORT.len() as u64));

    let m = StrMatcher::new("x").unwrap();
    assert_eq!(m.tier(), MatcherTier::Byte);
    g.bench_function("memchr_single_byte", |b| {
        b.iter(|| std::hint::black_box(m.is_match(std::hint::black_box(HAYSTACK_SHORT))));
    });

    let m = StrMatcher::new("[,\t|]").unwrap();
    assert_eq!(m.tier(), MatcherTier::Byte);
    g.bench_function("memchr3_byte_set", |b| {
        b.iter(|| std::hint::black_box(m.is_match(std::hint::black_box(HAYSTACK_SHORT))));
    });

    let m = StrMatcher::new("^/").unwrap();
    assert_eq!(m.tier(), MatcherTier::Byte);
    g.bench_function("anchored_single_byte_start", |b| {
        b.iter(|| std::hint::black_box(m.is_match(std::hint::black_box(HAYSTACK_SHORT))));
    });

    g.finish();
}

fn bench_literal_tier(c: &mut Criterion) {
    let mut g = c.benchmark_group("strmatch_literal");
    g.throughput(Throughput::Bytes(HAYSTACK_SHORT.len() as u64));

    let m = StrMatcher::new(r"^2026-").unwrap();
    assert_eq!(m.tier(), MatcherTier::Literal);
    g.bench_function("multi_byte_starts_with", |b| {
        b.iter(|| std::hint::black_box(m.is_match(std::hint::black_box(HAYSTACK_SHORT))));
    });

    let m = StrMatcher::new("request_id").unwrap();
    assert_eq!(m.tier(), MatcherTier::Literal);
    g.bench_function("multi_byte_memmem", |b| {
        b.iter(|| std::hint::black_box(m.is_match(std::hint::black_box(HAYSTACK_SHORT))));
    });

    g.finish();
}

fn bench_literal_set_tier(c: &mut Criterion) {
    let mut g = c.benchmark_group("strmatch_literal_set");
    g.throughput(Throughput::Bytes(HAYSTACK_SHORT.len() as u64));

    let m = StrMatcher::new("INFO|WARN|ERROR").unwrap();
    assert_eq!(m.tier(), MatcherTier::LiteralSet);
    g.bench_function("alternation_3", |b| {
        b.iter(|| std::hint::black_box(m.is_match(std::hint::black_box(HAYSTACK_SHORT))));
    });

    let patterns: Vec<String> = (0..100).map(|i| format!("pattern_{i:03}")).collect();
    let joined = patterns.join("|");
    let m = StrMatcher::new(&joined).unwrap();
    assert_eq!(m.tier(), MatcherTier::LiteralSet);
    g.bench_function("alternation_100_no_match", |b| {
        b.iter(|| std::hint::black_box(m.is_match(std::hint::black_box(HAYSTACK_SHORT))));
    });

    g.finish();
}

fn bench_regex_tier(c: &mut Criterion) {
    let mut g = c.benchmark_group("strmatch_regex");
    g.throughput(Throughput::Bytes(HAYSTACK_SHORT.len() as u64));

    let m = StrMatcher::new(r"\w+@\w+").unwrap();
    assert_eq!(m.tier(), MatcherTier::Regex);
    g.bench_function("word_at_word_short", |b| {
        b.iter(|| std::hint::black_box(m.is_match(std::hint::black_box(HAYSTACK_SHORT))));
    });

    g.finish();
}

fn bench_set_construction(c: &mut Criterion) {
    let mut g = c.benchmark_group("strmatch_set");

    let patterns: Vec<String> = (0..100).map(|i| format!("token_{i:04}")).collect();
    let set = StrMatcherSet::new(&patterns).unwrap();
    let [byte_n, lit_n, lit_set_n, regex_n] = set.tier_counts();
    // Each pattern is multi-byte "token_XXXX" → Literal tier
    assert_eq!(
        (byte_n, lit_n, lit_set_n, regex_n),
        (0, 100, 0, 0),
        "expected 100 Literal-tier patterns",
    );

    g.bench_function("set_100_is_match_no_hit", |b| {
        b.iter(|| std::hint::black_box(set.is_match(std::hint::black_box(HAYSTACK_SHORT))));
    });

    g.bench_function("set_100_earliest_match_no_hit", |b| {
        b.iter(|| std::hint::black_box(set.earliest_match(std::hint::black_box(HAYSTACK_SHORT))));
    });

    g.finish();
}

fn bench_long_haystack(c: &mut Criterion) {
    let mut g = c.benchmark_group("strmatch_long_haystack");
    g.throughput(Throughput::Bytes(HAYSTACK_LONG.len() as u64));

    let m = StrMatcher::new("AKIA|ghp_|sk_live_").unwrap();
    g.bench_function("literal_set_3_long", |b| {
        b.iter(|| std::hint::black_box(m.is_match(std::hint::black_box(HAYSTACK_LONG))));
    });

    g.finish();
}

criterion_group!(
    benches,
    bench_byte_tier,
    bench_literal_tier,
    bench_literal_set_tier,
    bench_regex_tier,
    bench_set_construction,
    bench_long_haystack
);
criterion_main!(benches);
