// Project:   hyperi-rustlib
// File:      tests/engine_integration.rs
// Purpose:   Integration tests for BatchEngine WorkBatch driver run loop
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

#![cfg(all(feature = "worker", feature = "transport-memory"))]

use std::sync::{Arc, Mutex};

use hyperi_rustlib::transport::{MemoryConfig, MemoryTransport, WorkBatch};
use hyperi_rustlib::worker::{BatchEngine, BatchProcessingConfig, CommitMode, EngineError};
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
        ..Default::default()
    })
    .expect("memory transport with valid config must construct")
}

/// Inject JSON messages into the transport.
async fn inject_json(transport: &MemoryTransport, n: usize) {
    for i in 0..n {
        let payload = format!(r#"{{"_table":"events","id":{i}}}"#).into_bytes();
        transport.inject(None, payload).await.unwrap();
    }
}

/// No-ticker placeholder for the driver's `ticker` argument.
#[allow(clippy::type_complexity)]
fn no_ticker() -> Option<(
    std::time::Duration,
    fn() -> std::future::Ready<Result<(), EngineError>>,
)> {
    None
}

// --- run_workbatch() tests (the WorkBatch driver replaced the legacy loops) ---

#[tokio::test]
async fn run_workbatch_processes_injected_records_then_shuts_down() {
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
        .run_workbatch(
            &transport,
            shutdown,
            // On-demand process: parse each record's payload to read `_table`.
            |batch| Ok(batch),
            |out: &WorkBatch<_>| {
                let collected = Arc::clone(&collected_clone);
                let tables: Vec<String> = out
                    .records
                    .iter()
                    .map(|r| {
                        let parsed =
                            hyperi_rustlib::transport::codec::parse(&r.payload, r.metadata.format)
                                .expect("valid json");
                        parsed.field_str("_table").unwrap_or("?").to_string()
                    })
                    .collect();
                async move {
                    collected.lock().unwrap().extend(tables);
                    Ok(())
                }
            },
            CommitMode::Auto,
            no_ticker(),
        )
        .await;

    // run_workbatch() exits cleanly on shutdown.
    assert!(
        result.is_ok(),
        "run_workbatch() should return Ok on shutdown: {result:?}"
    );
    // At least some records were processed (exact count depends on scheduling).
    let guard = collected.lock().unwrap();
    assert!(!guard.is_empty(), "Expected at least one processed record");
    assert!(guard.iter().all(|s| s == "events"));
}

#[tokio::test]
async fn run_workbatch_shuts_down_immediately_when_empty() {
    let engine = make_engine();
    let transport = make_transport();

    // No messages — cancel immediately.
    let shutdown = CancellationToken::new();
    shutdown.cancel();

    let result = engine
        .run_workbatch(
            &transport,
            shutdown,
            |batch| Ok(batch),
            |_out: &WorkBatch<_>| async { Ok(()) },
            CommitMode::Auto,
            no_ticker(),
        )
        .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn run_workbatch_sink_error_skips_commit() {
    let engine = make_engine();
    let transport = make_transport();

    inject_json(&transport, 5).await;

    let shutdown = CancellationToken::new();
    let token = shutdown.clone();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        token.cancel();
    });

    // Sink always returns an error. The sink error is now a TERMINAL ack-barrier
    // error (Remediation Phase 1): the run stops and the commit is skipped, so
    // the ORDERED/cumulative source commit can never advance past the unsent
    // block. committed_sequence must remain 0.
    let result = engine
        .run_workbatch(
            &transport,
            shutdown,
            |batch| Ok(batch),
            |_out: &WorkBatch<_>| async { Err(EngineError::Sink("intentional".into())) },
            CommitMode::Auto,
            no_ticker(),
        )
        .await;

    assert!(
        matches!(result, Err(EngineError::Sink(_))),
        "sink error must be a terminal ack-barrier error: {result:?}"
    );
    // Commit was skipped because sink errored.
    assert_eq!(
        transport.committed_sequence(),
        0,
        "Commit should be skipped on sink error"
    );
}

// --- raw byte processing via the on-demand driver (no parse) ---

#[tokio::test]
async fn run_workbatch_processes_records_as_bytes() {
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
        .run_workbatch(
            &transport,
            shutdown,
            // Pass-through process pays no parse cost (raw byte handling).
            |batch| Ok(batch),
            |out: &WorkBatch<_>| {
                let lengths = Arc::clone(&lengths_clone);
                let lens: Vec<usize> = out.records.iter().map(|r| r.payload.len()).collect();
                async move {
                    lengths.lock().unwrap().extend(lens);
                    Ok(())
                }
            },
            CommitMode::Auto,
            no_ticker(),
        )
        .await;

    assert!(result.is_ok());
    let guard = lengths.lock().unwrap();
    assert!(
        !guard.is_empty(),
        "Expected at least one raw record processed"
    );
    assert!(guard.iter().all(|&n| n > 0));
}

#[tokio::test]
async fn run_workbatch_raw_shuts_down_immediately_when_empty() {
    let engine = make_engine();
    let transport = make_transport();

    let shutdown = CancellationToken::new();
    shutdown.cancel();

    let result = engine
        .run_workbatch(
            &transport,
            shutdown,
            |batch| Ok(batch),
            |_out: &WorkBatch<_>| async { Ok(()) },
            CommitMode::Auto,
            no_ticker(),
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

/// Verify record bytes roundtrip through transport inject -> recv (WorkBatch).
#[tokio::test]
async fn transport_inject_recv_roundtrip() {
    let transport = make_transport();
    transport.inject(None, b"hello".to_vec()).await.unwrap();

    use hyperi_rustlib::transport::TransportReceiver;
    let records = transport.recv(1).await.unwrap().records;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].payload.as_ref(), b"hello");
}
