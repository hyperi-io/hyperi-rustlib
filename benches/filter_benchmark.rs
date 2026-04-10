// Project:   hyperi-rustlib
// File:      benches/filter_benchmark.rs
// Purpose:   Criterion benchmarks for transport filter engine performance
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Benchmarks for the transport filter engine.
//!
//! Validates the design assumption: Tier 1 filters are ~50-100ns/msg via SIMD,
//! and the no-filter overhead is negligible.

use criterion::{Criterion, Throughput, criterion_group, criterion_main};

use hyperi_rustlib::transport::filter::{
    FilterAction, FilterDisposition, FilterRule, TransportFilterEngine, TransportFilterTierConfig,
};

const SAMPLE_PAYLOAD: &[u8] = br#"{"_table":"events","host":"prod-web01","source_type":"syslog","severity":3,"id":12345,"timestamp":"2026-04-10T12:00:00Z","message":"Sample log event with some realistic padding for benchmarking"}"#;

const POISON_PAYLOAD: &[u8] = br#"{"_table":"events","status":"poison","data":"x"}"#;

fn bench_no_filters_baseline(c: &mut Criterion) {
    let engine = TransportFilterEngine::empty();

    let mut group = c.benchmark_group("filter_no_filters");
    group.throughput(Throughput::Elements(1));
    group.bench_function("apply_inbound_no_filters", |b| {
        b.iter(|| std::hint::black_box(engine.apply_inbound(SAMPLE_PAYLOAD)));
    });
    group.finish();
}

fn bench_tier1_field_exists(c: &mut Criterion) {
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: "has(_table)".into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    let mut group = c.benchmark_group("filter_tier1_field_exists");
    group.throughput(Throughput::Elements(1));
    group.bench_function("match", |b| {
        b.iter(|| std::hint::black_box(engine.apply_inbound(SAMPLE_PAYLOAD)));
    });
    group.finish();
}

fn bench_tier1_field_equals(c: &mut Criterion) {
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: r#"status == "poison""#.into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    let mut group = c.benchmark_group("filter_tier1_field_equals");
    group.throughput(Throughput::Elements(1));
    group.bench_function("no_match_pass", |b| {
        b.iter(|| {
            let result = engine.apply_inbound(SAMPLE_PAYLOAD);
            assert_eq!(result, FilterDisposition::Pass);
            std::hint::black_box(result)
        });
    });
    group.bench_function("match_drop", |b| {
        b.iter(|| {
            let result = engine.apply_inbound(POISON_PAYLOAD);
            assert_eq!(result, FilterDisposition::Drop);
            std::hint::black_box(result)
        });
    });
    group.finish();
}

fn bench_tier1_starts_with(c: &mut Criterion) {
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: r#"host.startsWith("prod-")"#.into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    let mut group = c.benchmark_group("filter_tier1_starts_with");
    group.throughput(Throughput::Elements(1));
    group.bench_function("match", |b| {
        b.iter(|| std::hint::black_box(engine.apply_inbound(SAMPLE_PAYLOAD)));
    });
    group.finish();
}

fn bench_tier1_dotted_path(c: &mut Criterion) {
    let nested_payload =
        br#"{"metadata":{"source":"aws","region":"ap-southeast-2"},"event":"login"}"#;
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: r#"metadata.source == "aws""#.into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    let mut group = c.benchmark_group("filter_tier1_dotted_path");
    group.throughput(Throughput::Elements(1));
    group.bench_function("nested_match", |b| {
        b.iter(|| std::hint::black_box(engine.apply_inbound(nested_payload)));
    });
    group.finish();
}

fn bench_tier1_first_match_wins(c: &mut Criterion) {
    // 5 filters, message matches the third one
    let rules = vec![
        FilterRule {
            expression: "has(no_match_1)".into(),
            action: FilterAction::Drop,
        },
        FilterRule {
            expression: "has(no_match_2)".into(),
            action: FilterAction::Drop,
        },
        FilterRule {
            expression: "has(_table)".into(),
            action: FilterAction::Drop,
        },
        FilterRule {
            expression: "has(no_match_3)".into(),
            action: FilterAction::Drop,
        },
        FilterRule {
            expression: "has(no_match_4)".into(),
            action: FilterAction::Drop,
        },
    ];
    let engine =
        TransportFilterEngine::new(&rules, &[], &TransportFilterTierConfig::default()).unwrap();

    let mut group = c.benchmark_group("filter_tier1_first_match_wins");
    group.throughput(Throughput::Elements(1));
    group.bench_function("match_at_position_3", |b| {
        b.iter(|| std::hint::black_box(engine.apply_inbound(SAMPLE_PAYLOAD)));
    });
    group.finish();
}

#[cfg(feature = "expression")]
fn bench_tier2_compound_cel(c: &mut Criterion) {
    let tier_config = TransportFilterTierConfig {
        allow_cel_filters_in: true,
        ..Default::default()
    };
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: r#"severity > 3 && source != "internal""#.into(),
            action: FilterAction::Drop,
        }],
        &[],
        &tier_config,
    )
    .unwrap();

    let payload = br#"{"severity":5,"source":"external","data":"x"}"#;

    let mut group = c.benchmark_group("filter_tier2_compound_cel");
    group.throughput(Throughput::Elements(1));
    group.bench_function("compound_cel_match", |b| {
        b.iter(|| std::hint::black_box(engine.apply_inbound(payload)));
    });
    group.finish();
}

#[cfg(feature = "expression")]
fn bench_tier3_regex_cel(c: &mut Criterion) {
    let tier_config = TransportFilterTierConfig {
        allow_complex_filters_in: true,
        ..Default::default()
    };
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: r#"host.matches("^prod-.*$")"#.into(),
            action: FilterAction::Drop,
        }],
        &[],
        &tier_config,
    )
    .unwrap();

    let mut group = c.benchmark_group("filter_tier3_regex_cel");
    group.throughput(Throughput::Elements(1));
    group.bench_function("regex_match", |b| {
        b.iter(|| std::hint::black_box(engine.apply_inbound(SAMPLE_PAYLOAD)));
    });
    group.finish();
}

#[cfg(feature = "expression")]
criterion_group!(
    benches,
    bench_no_filters_baseline,
    bench_tier1_field_exists,
    bench_tier1_field_equals,
    bench_tier1_starts_with,
    bench_tier1_dotted_path,
    bench_tier1_first_match_wins,
    bench_tier2_compound_cel,
    bench_tier3_regex_cel,
);

#[cfg(not(feature = "expression"))]
criterion_group!(
    benches,
    bench_no_filters_baseline,
    bench_tier1_field_exists,
    bench_tier1_field_equals,
    bench_tier1_starts_with,
    bench_tier1_dotted_path,
    bench_tier1_first_match_wins,
);

criterion_main!(benches);
