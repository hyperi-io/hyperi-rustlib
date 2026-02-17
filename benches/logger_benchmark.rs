// Project:   hyperi-rustlib
// File:      benches/logger_benchmark.rs
// Purpose:   Logger benchmarks
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;

fn logger_benchmark(c: &mut Criterion) {
    c.bench_function("masking_should_mask", |b| {
        let layer = hyperi_rustlib::logger::MaskingLayer::new();
        b.iter(|| {
            black_box(layer.should_mask("password"));
            black_box(layer.should_mask("username"));
            black_box(layer.should_mask("api_key"));
        });
    });
}

criterion_group!(benches, logger_benchmark);
criterion_main!(benches);
