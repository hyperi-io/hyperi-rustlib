// Project:   hs-rustlib
// File:      benches/logger_benchmark.rs
// Purpose:   Logger benchmarks
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn logger_benchmark(c: &mut Criterion) {
    c.bench_function("masking_should_mask", |b| {
        let layer = hs_rustlib::logger::MaskingLayer::new();
        b.iter(|| {
            black_box(layer.should_mask("password"));
            black_box(layer.should_mask("username"));
            black_box(layer.should_mask("api_key"));
        });
    });
}

criterion_group!(benches, logger_benchmark);
criterion_main!(benches);
