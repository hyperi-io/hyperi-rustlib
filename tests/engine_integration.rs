// Project:   hyperi-rustlib
// File:      tests/engine_integration.rs
// Purpose:   Integration tests for BatchEngine transport-wired run loop
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

#![cfg(all(feature = "worker", feature = "transport-memory"))]

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use hyperi_rustlib::transport::{MemoryConfig, MemoryTransport};
use hyperi_rustlib::worker::{BatchEngine, BatchProcessingConfig, EngineError};
use tokio_util::sync::CancellationToken;

/// Build a default engine.
fn make_engine() -> BatchEngine {
    BatchEngine::new(BatchProcessingConfig::default())
}

/// Build a memory transport with no recv timeout (returns immediately).
fn make_transport() -> MemoryTransport {
    MemoryTransport::new(&MemoryConfig {
        buffer_size: 1000,
        recv_timeout_ms: 0,
    })
}

/// Inject JSON messages into the transport.
async fn inject_json(transport: &MemoryTransport, n: usize) {
    for i in 0..n {
        let payload = format!(r#"{{"_table":"events","id":{i}}}"#).into_bytes();
        transport.inject(None, payload).await.unwrap();
    }
}

// --- run() tests ---

#[tokio::test]
async fn run_processes_injected_messages_then_shuts_down() {
    let engine = make_engine();
    let transport = make_transport();

    // Inject 10 messages, then immediately cancel.
    inject_json(&transport, 10).await;

    let collected: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let collected_clone = Arc::clone(&collected);

    let shutdown = CancellationToken::new();
    let token = shutdown.clone();
    // Cancel after a tiny yield so the run loop has a chance to drain.
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        token.cancel();
    });

    let result = engine
        .run(
            &transport,
            shutdown,
            |pm| -> Result<String, String> {
                Ok(pm
                    .field("_table")
                    .and_then(|v| sonic_rs::JsonValueTrait::as_str(v))
                    .unwrap_or("?")
                    .to_string())
            },
            |results| {
                let mut guard = collected_clone.lock().unwrap();
                guard.extend(results.into_iter().flatten());
                Ok(())
            },
        )
        .await;

    // run() exits cleanly on shutdown.
    assert!(
        result.is_ok(),
        "run() should return Ok on shutdown: {result:?}"
    );
    // At least some messages were processed (exact count depends on scheduling).
    let guard = collected.lock().unwrap();
    assert!(!guard.is_empty(), "Expected at least one processed message");
    assert!(guard.iter().all(|s| s == "events"));
}

#[tokio::test]
async fn run_shuts_down_immediately_when_empty() {
    let engine = make_engine();
    let transport = make_transport();

    // No messages — cancel immediately.
    let shutdown = CancellationToken::new();
    shutdown.cancel();

    let result = engine
        .run(
            &transport,
            shutdown,
            |_pm| -> Result<(), String> { Ok(()) },
            |_results| Ok(()),
        )
        .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn run_sink_error_skips_commit() {
    let engine = make_engine();
    let transport = make_transport();

    inject_json(&transport, 5).await;

    let shutdown = CancellationToken::new();
    let token = shutdown.clone();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        token.cancel();
    });

    // Sink always returns an error — committed_sequence should remain 0.
    let result = engine
        .run(
            &transport,
            shutdown,
            |_pm| -> Result<(), String> { Ok(()) },
            |_results| Err(EngineError::Sink("intentional".into())),
        )
        .await;

    assert!(
        result.is_ok(),
        "run() should still exit cleanly: {result:?}"
    );
    // Commit was skipped because sink errored.
    assert_eq!(
        transport.committed_sequence(),
        0,
        "Commit should be skipped on sink error"
    );
}

// --- run_raw() tests ---

#[tokio::test]
async fn run_raw_processes_messages_as_bytes() {
    let engine = make_engine();
    let transport = make_transport();

    inject_json(&transport, 5).await;

    let lengths: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let lengths_clone = Arc::clone(&lengths);

    let shutdown = CancellationToken::new();
    let token = shutdown.clone();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        token.cancel();
    });

    let result = engine
        .run_raw(
            &transport,
            shutdown,
            |raw| -> Result<usize, String> { Ok(raw.payload.len()) },
            |results| {
                let mut guard = lengths_clone.lock().unwrap();
                for r in results {
                    guard.push(r.unwrap());
                }
                Ok(())
            },
        )
        .await;

    assert!(result.is_ok());
    let guard = lengths.lock().unwrap();
    assert!(
        !guard.is_empty(),
        "Expected at least one raw message processed"
    );
    assert!(guard.iter().all(|&n| n > 0));
}

#[tokio::test]
async fn run_raw_shuts_down_immediately_when_empty() {
    let engine = make_engine();
    let transport = make_transport();

    let shutdown = CancellationToken::new();
    shutdown.cancel();

    let result = engine
        .run_raw(
            &transport,
            shutdown,
            |_raw| -> Result<(), String> { Ok(()) },
            |_results| Ok(()),
        )
        .await;

    assert!(result.is_ok());
}

// --- Type constraint compilation tests ---

/// Verify EngineError variants are constructable.
#[test]
fn engine_error_variants_constructable() {
    let sink_err = EngineError::Sink("test".into());
    assert!(matches!(sink_err, EngineError::Sink(_)));

    let shutdown_err = EngineError::Shutdown;
    assert!(matches!(shutdown_err, EngineError::Shutdown));
}

/// Verify EngineError is Debug + Display.
#[test]
fn engine_error_debug_display() {
    let e = EngineError::Sink("oops".into());
    let _ = format!("{e}");
    let _ = format!("{e:?}");
}

/// Verify raw message bytes roundtrip through transport inject → recv.
#[tokio::test]
async fn transport_inject_recv_roundtrip() {
    let transport = make_transport();
    transport.inject(None, b"hello".to_vec()).await.unwrap();

    use hyperi_rustlib::transport::TransportReceiver;
    let messages = transport.recv(1).await.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].payload, b"hello");
}

/// Verify RawMessage::from works for transport messages.
#[tokio::test]
async fn raw_message_from_transport_message() {
    use hyperi_rustlib::transport::TransportReceiver;
    use hyperi_rustlib::worker::RawMessage;

    let transport = make_transport();
    transport
        .inject(None, br#"{"x":1}"#.to_vec())
        .await
        .unwrap();

    let messages = transport.recv(1).await.unwrap();
    let raw: Vec<RawMessage> = messages.into_iter().map(RawMessage::from).collect();
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].payload, Bytes::from(br#"{"x":1}"#.to_vec()));
}
