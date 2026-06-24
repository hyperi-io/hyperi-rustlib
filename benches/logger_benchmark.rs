// Project:   scalo
// File:      benches/logger_benchmark.rs
// Purpose:   Logger benchmarks
// Language:  Rust
//
// License:   Apache-2.0
// Copyright: (c) 2026 HYPERI PTY LIMITED

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

fn logger_benchmark(c: &mut Criterion) {
    c.bench_function("masking_should_mask", |b| {
        let layer = scalo::logger::MaskingLayer::new();
        b.iter(|| {
            black_box(layer.should_mask("password"));
            black_box(layer.should_mask("username"));
            black_box(layer.should_mask("api_key"));
        });
    });
}

criterion_group!(benches, logger_benchmark);
criterion_main!(benches);
