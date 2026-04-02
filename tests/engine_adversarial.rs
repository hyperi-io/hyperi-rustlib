// Project:   hyperi-rustlib
// File:      tests/engine_adversarial.rs
// Purpose:   Adversarial tests for BatchEngine — edge cases, boundaries, stress
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

#![cfg(feature = "worker")]

use std::sync::Arc;

use bytes::Bytes;
use hyperi_rustlib::worker::engine::config::PreRouteFilterConfig;
use hyperi_rustlib::worker::engine::intern::FieldInterner;
use hyperi_rustlib::worker::engine::types::{MessageMetadata, PayloadFormat, RawMessage};
use hyperi_rustlib::worker::engine::{BatchEngine, BatchProcessingConfig};
use sonic_rs::JsonValueTrait as _;

// --- Helpers ---

fn make_raw(payload: &[u8]) -> RawMessage {
    RawMessage {
        payload: Bytes::copy_from_slice(payload),
        key: None,
        headers: vec![],
        metadata: MessageMetadata {
            timestamp_ms: None,
            format: PayloadFormat::Json,
            commit_token: None,
        },
    }
}

fn make_json_messages(n: usize) -> Vec<RawMessage> {
    (0..n)
        .map(|i| make_raw(format!(r#"{{"_table":"events","id":{i}}}"#).as_bytes()))
        .collect()
}

fn default_engine() -> BatchEngine {
    BatchEngine::new(BatchProcessingConfig::default())
}

// --- Tests ---

#[test]
fn empty_batch() {
    let engine = default_engine();
    let results: Vec<Result<(), String>> = engine.process_mid_tier(&[], |_| Ok(()));
    assert!(results.is_empty());
}

#[test]
fn single_message() {
    let engine = default_engine();
    let msgs = make_json_messages(1);
    let results: Vec<Result<usize, String>> =
        engine.process_mid_tier(&msgs, |pm| Ok(pm.raw_payload().len()));
    assert_eq!(results.len(), 1);
    assert!(results[0].is_ok());
}

#[test]
fn chunk_boundary_exact() {
    let config = BatchProcessingConfig {
        max_chunk_size: 10_000,
        ..Default::default()
    };
    let engine = BatchEngine::new(config);
    let msgs = make_json_messages(10_000);
    let results: Vec<Result<(), String>> = engine.process_mid_tier(&msgs, |_| Ok(()));
    assert_eq!(results.len(), 10_000);
    assert!(results.iter().all(|r| r.is_ok()));

    let snap = engine.stats().snapshot();
    assert_eq!(snap.received, 10_000);
    assert_eq!(snap.processed, 10_000);
}

#[test]
fn chunk_boundary_plus_one() {
    let config = BatchProcessingConfig {
        max_chunk_size: 10_000,
        ..Default::default()
    };
    let engine = BatchEngine::new(config);
    let msgs = make_json_messages(10_001);
    let results: Vec<Result<(), String>> = engine.process_mid_tier(&msgs, |_| Ok(()));
    assert_eq!(results.len(), 10_001);
    // Two chunks: 10_000 + 1
    let snap = engine.stats().snapshot();
    assert_eq!(snap.received, 10_001);
}

#[test]
fn all_parse_errors() {
    let engine = default_engine();
    let msgs: Vec<RawMessage> = (0..20)
        .map(|i| make_raw(format!("not json {i} {{{{").as_bytes()))
        .collect();

    let results: Vec<Result<(), String>> = engine.process_mid_tier(&msgs, |_| Ok(()));
    assert_eq!(results.len(), 20);
    assert!(results.iter().all(|r| r.is_err()));

    let snap = engine.stats().snapshot();
    assert_eq!(snap.errors, 20);
    assert_eq!(snap.processed, 0);
}

#[test]
fn mixed_valid_invalid() {
    let engine = default_engine();
    let msgs: Vec<RawMessage> = (0..100)
        .map(|i| {
            if i % 2 == 0 {
                make_raw(format!(r#"{{"id":{i}}}"#).as_bytes())
            } else {
                make_raw(b"definitely not json >>>")
            }
        })
        .collect();

    let results: Vec<Result<(), String>> = engine.process_mid_tier(&msgs, |_| Ok(()));
    assert_eq!(results.len(), 100);

    let ok_count = results.iter().filter(|r| r.is_ok()).count();
    let err_count = results.iter().filter(|r| r.is_err()).count();
    assert_eq!(ok_count, 50);
    assert_eq!(err_count, 50);
}

#[test]
fn deeply_nested_json() {
    // Build 50 levels of nesting: {"a":{"a":{"a":...{}}}}
    let mut payload = String::new();
    for _ in 0..50 {
        payload.push_str(r#"{"a":"#);
    }
    payload.push_str(r#""leaf""#);
    for _ in 0..50 {
        payload.push('}');
    }

    let engine = default_engine();
    let msgs = vec![make_raw(payload.as_bytes())];
    let results: Vec<Result<usize, String>> =
        engine.process_mid_tier(&msgs, |pm| Ok(pm.raw_payload().len()));
    assert_eq!(results.len(), 1);
    assert!(results[0].is_ok());
}

#[test]
fn large_payload() {
    // ~1 MB JSON payload with padding
    let padding = "x".repeat(1_000_000);
    let payload = format!(r#"{{"_table":"events","data":"{padding}"}}"#);
    let engine = default_engine();
    let msgs = vec![make_raw(payload.as_bytes())];
    let results: Vec<Result<usize, String>> =
        engine.process_mid_tier(&msgs, |pm| Ok(pm.raw_payload().len()));
    assert_eq!(results.len(), 1);
    assert!(results[0].is_ok());
    assert!(*results[0].as_ref().unwrap() > 1_000_000);
}

#[test]
fn unicode_field_names() {
    // Routing field with Unicode characters in the value
    let engine = default_engine();
    let msgs = vec![
        make_raw(r#"{"_table":"évènements","id":1}"#.as_bytes()),
        make_raw(r#"{"_table":"事件","id":2}"#.as_bytes()),
        make_raw(r#"{"_table":"أحداث","id":3}"#.as_bytes()),
    ];
    let results: Vec<Result<usize, String>> =
        engine.process_mid_tier(&msgs, |pm| Ok(pm.raw_payload().len()));
    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|r| r.is_ok()));
}

#[test]
fn empty_payload_bytes() {
    let engine = default_engine();
    let msgs = vec![make_raw(b"")];
    let results: Vec<Result<(), String>> = engine.process_mid_tier(&msgs, |_| Ok(()));
    // Empty payload is a parse error (DLQ by default)
    assert_eq!(results.len(), 1);
    assert!(results[0].is_err());

    let snap = engine.stats().snapshot();
    assert_eq!(snap.errors, 1);
}

#[test]
fn null_in_payload() {
    // Payload with embedded NUL bytes — not valid JSON
    let payload = b"{\"id\":1,\x00\"extra\":2}";
    let engine = default_engine();
    let msgs = vec![make_raw(payload)];
    // sonic_rs should reject NUL bytes in the middle of JSON
    let results: Vec<Result<(), String>> = engine.process_mid_tier(&msgs, |_| Ok(()));
    assert_eq!(results.len(), 1);
    // May parse OK or fail depending on sonic_rs behaviour — just verify no panic
    let _ = results[0].as_ref();
}

#[test]
fn pre_route_all_filtered() {
    let config = BatchProcessingConfig {
        routing_field: Some("_table".to_string()),
        pre_route_filters: vec![PreRouteFilterConfig::DropFieldMissing {
            field: "_table".to_string(),
        }],
        ..Default::default()
    };
    let engine = BatchEngine::new(config);

    // All messages are missing _table
    let msgs: Vec<RawMessage> = (0..20)
        .map(|i| make_raw(format!(r#"{{"host":"web-{i}"}}"#).as_bytes()))
        .collect();

    let results: Vec<Result<(), String>> = engine.process_mid_tier(&msgs, |_| Ok(()));
    // All filtered — no results
    assert!(results.is_empty());

    let snap = engine.stats().snapshot();
    assert_eq!(snap.filtered, 20);
    assert_eq!(snap.received, 20);
    assert_eq!(snap.processed, 0);
}

#[test]
fn pre_route_none_filtered() {
    let config = BatchProcessingConfig {
        routing_field: Some("_table".to_string()),
        pre_route_filters: vec![PreRouteFilterConfig::DropFieldMissing {
            field: "_table".to_string(),
        }],
        ..Default::default()
    };
    let engine = BatchEngine::new(config);

    // All messages have _table — none filtered
    let msgs = make_json_messages(50);
    let results: Vec<Result<(), String>> = engine.process_mid_tier(&msgs, |_| Ok(()));
    assert_eq!(results.len(), 50);
    assert!(results.iter().all(|r| r.is_ok()));

    let snap = engine.stats().snapshot();
    assert_eq!(snap.filtered, 0);
    assert_eq!(snap.processed, 50);
}

#[test]
fn chunk_size_one() {
    let config = BatchProcessingConfig {
        max_chunk_size: 1,
        ..Default::default()
    };
    let engine = BatchEngine::new(config);
    let msgs = make_json_messages(20);
    let results: Vec<Result<usize, String>> =
        engine.process_mid_tier(&msgs, |pm| Ok(pm.raw_payload().len()));
    assert_eq!(results.len(), 20);
    assert!(results.iter().all(|r| r.is_ok()));

    let snap = engine.stats().snapshot();
    assert_eq!(snap.received, 20);
    assert_eq!(snap.processed, 20);
}

#[test]
fn chunk_size_zero() {
    // max_chunk_size = 0 means process all at once (no chunking)
    let config = BatchProcessingConfig {
        max_chunk_size: 0,
        ..Default::default()
    };
    let engine = BatchEngine::new(config);
    let msgs = make_json_messages(100);
    let results: Vec<Result<(), String>> = engine.process_mid_tier(&msgs, |_| Ok(()));
    assert_eq!(results.len(), 100);
    assert!(results.iter().all(|r| r.is_ok()));
}

#[test]
fn transform_returns_error() {
    let engine = default_engine();
    let msgs = make_json_messages(10);

    let results: Vec<Result<usize, String>> = engine.process_mid_tier(&msgs, |pm| {
        let id_val = pm.field("id");
        let id = id_val.and_then(|v| v.as_u64()).unwrap_or(0);
        if id % 2 == 0 {
            Err(format!("even id rejected: {id}"))
        } else {
            Ok(usize::try_from(id).unwrap_or(usize::MAX))
        }
    });

    assert_eq!(results.len(), 10);
    let err_count = results.iter().filter(|r| r.is_err()).count();
    let ok_count = results.iter().filter(|r| r.is_ok()).count();
    // ids 0,2,4,6,8 → 5 errors; 1,3,5,7,9 → 5 ok
    assert_eq!(err_count, 5);
    assert_eq!(ok_count, 5);
}

#[test]
fn intern_concurrent_stress() {
    use std::thread;

    let interner = Arc::new(FieldInterner::new());
    let field = "_table";
    let num_threads = 8;
    let calls_per_thread = 10_000;

    let handles: Vec<_> = (0..num_threads)
        .map(|_| {
            let interner = Arc::clone(&interner);
            thread::spawn(move || {
                for _ in 0..calls_per_thread {
                    let result = interner.intern(field);
                    assert_eq!(result.as_ref(), field);
                }
            })
        })
        .collect();

    for h in handles {
        h.join().expect("thread panicked");
    }

    // Only one entry should exist
    assert_eq!(interner.len(), 1);

    // All future interns return the same Arc
    let a = interner.intern(field);
    let b = interner.intern(field);
    assert!(Arc::ptr_eq(&a, &b));
}

#[test]
fn process_raw_large_batch() {
    let engine = default_engine();
    let msgs = make_json_messages(5_000);
    let results: Vec<Result<usize, String>> =
        engine.process_raw(&msgs, |msg| Ok(msg.payload.len()));
    assert_eq!(results.len(), 5_000);
    assert!(results.iter().all(|r| r.is_ok()));
    assert!(results.iter().all(|r| *r.as_ref().unwrap() > 0));

    let snap = engine.stats().snapshot();
    assert_eq!(snap.received, 5_000);
    assert_eq!(snap.processed, 5_000);
}
