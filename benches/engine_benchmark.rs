// Project:   hyperi-rustlib
// File:      benches/engine_benchmark.rs
// Purpose:   Criterion benchmarks for BatchEngine throughput and overhead
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use std::sync::Arc;

use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

use hyperi_rustlib::worker::engine::types::{MessageMetadata, PayloadFormat, RawMessage};
use hyperi_rustlib::worker::engine::{BatchEngine, BatchProcessingConfig};

fn make_messages(n: usize) -> Vec<RawMessage> {
    (0..n)
        .map(|i| RawMessage {
            payload: Bytes::from(format!(
                r#"{{"_table":"events","host":"web-{i}","source_type":"syslog","id":{i},"timestamp":"2026-04-02T12:00:00Z","message":"Event number {i} with some padding data to make it realistic"}}"#
            )),
            key: Some(Arc::from("partition-key")),
            headers: vec![],
            metadata: MessageMetadata {
                timestamp_ms: Some(1_743_580_800_000),
                format: PayloadFormat::Json,
                commit_token: None,
            },
        })
        .collect()
}

fn bench_engine(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_engine");

    let messages_10k = make_messages(10_000);

    // Benchmark 1: engine mid-tier vs manual serde_json parse
    group.bench_function("engine_mid_tier_10k", |b| {
        let engine = BatchEngine::new(BatchProcessingConfig::default());
        b.iter(|| {
            let results: Vec<Result<(), String>> = engine.process_mid_tier(&messages_10k, |msg| {
                let _ = msg.field("_table");
                Ok(())
            });
            assert!(!results.is_empty());
        });
    });

    group.bench_function("manual_serde_json_10k", |b| {
        b.iter(|| {
            let results: Vec<Result<(), String>> = messages_10k
                .iter()
                .map(|msg| {
                    let v: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
                    let _ = v["_table"].as_str();
                    Ok(())
                })
                .collect();
            assert!(!results.is_empty());
        });
    });

    // Benchmark 2: with vs without pre-route
    group.bench_function("engine_with_pre_route_10k", |b| {
        let config = BatchProcessingConfig {
            routing_field: Some("_table".into()),
            ..BatchProcessingConfig::default()
        };
        let engine = BatchEngine::new(config);
        b.iter(|| {
            let results: Vec<Result<(), String>> =
                engine.process_mid_tier(&messages_10k, |_| Ok(()));
            assert!(!results.is_empty());
        });
    });

    // Benchmark 3: throughput scaling
    for size in [1_000, 5_000, 10_000, 20_000] {
        let msgs = make_messages(size);
        group.bench_with_input(
            BenchmarkId::new("engine_throughput", size),
            &msgs,
            |b, msgs| {
                let engine = BatchEngine::new(BatchProcessingConfig::default());
                b.iter(|| {
                    let results: Vec<Result<(), String>> = engine.process_mid_tier(msgs, |msg| {
                        let _ = msg.field("_table");
                        let _ = msg.field("host");
                        Ok(())
                    });
                    assert!(!results.is_empty());
                });
            },
        );
    }

    // Benchmark 4: process_raw (zero-copy passthrough)
    group.bench_function("engine_raw_10k", |b| {
        let engine = BatchEngine::new(BatchProcessingConfig::default());
        b.iter(|| {
            let results: Vec<Result<usize, String>> =
                engine.process_raw(&messages_10k, |msg| Ok(msg.payload.len()));
            assert!(!results.is_empty());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_engine);
criterion_main!(benches);
