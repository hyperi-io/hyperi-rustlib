// Project:   hyperi-rustlib
// File:      benches/config_benchmark.rs
// Purpose:   Configuration loading benchmarks
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use criterion::{criterion_group, criterion_main, Criterion};
use std::hint::black_box;

fn config_loading_benchmark(c: &mut Criterion) {
    c.bench_function("config_new_default", |b| {
        b.iter(|| {
            // Benchmark creating a new config with defaults
            let opts = hyperi_rustlib::config::ConfigOptions::default();
            black_box(opts)
        });
    });
}

criterion_group!(benches, config_loading_benchmark);
criterion_main!(benches);
