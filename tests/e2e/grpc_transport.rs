// Project:   hyperi-rustlib
// File:      tests/e2e/grpc_transport.rs
// Purpose:   Integration tests for gRPC transport (bidirectional client/server)
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Integration tests for the gRPC transport layer.
//!
//! These tests start real tonic gRPC servers and clients to verify
//! end-to-end message delivery. Run with `--test-threads=1` to avoid
//! port conflicts.
//!
//! `cargo test --test e2e_tests --features transport-grpc -- --test-threads=1`

use std::time::Duration;

use std::sync::Arc;

use hyperi_rustlib::transport::grpc::{GrpcConfig, GrpcTransport};
use hyperi_rustlib::transport::{
    PayloadFormat, Record, RecordMeta, SendResult, TransportBase, TransportReceiver,
    TransportSender,
};

/// Find an available port for testing.
async fn find_available_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind to ephemeral port");
    listener.local_addr().unwrap().port()
}

#[tokio::test]
async fn test_close_frees_port_and_is_idempotent() {
    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let server = GrpcTransport::new(&GrpcConfig::server(&addr))
        .await
        .expect("first server should bind");
    // close() must actually stop the listener (not just on Drop), and be
    // idempotent.
    server.close().await.expect("first close");
    server.close().await.expect("close is idempotent");

    // The serve task needs a moment to react to the shutdown signal and drop
    // the listener; poll-retry the rebind rather than sleep a fixed budget.
    let mut rebound = None;
    for _ in 0..40 {
        match GrpcTransport::new(&GrpcConfig::server(&addr)).await {
            Ok(s) => {
                rebound = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    assert!(
        rebound.is_some(),
        "port {port} not freed within 2s after close() -- the listener did not stop"
    );
}

/// Create a server+client pair on a given port.
async fn create_pair(port: u16) -> (GrpcTransport, GrpcTransport) {
    let addr = format!("127.0.0.1:{port}");

    let server_config = GrpcConfig::server(&addr);
    // No post-construction sleep needed: GrpcTransport::new binds the
    // listener synchronously, so the server is accepting connections the
    // moment this returns.
    let server = GrpcTransport::new(&server_config)
        .await
        .expect("failed to create server");

    let client_config = GrpcConfig::client(&format!("http://{addr}"));
    let client = GrpcTransport::new(&client_config)
        .await
        .expect("failed to create client");

    (server, client)
}

#[tokio::test]
async fn test_send_and_receive() {
    let port = find_available_port().await;
    let (server, client) = create_pair(port).await;

    // Send a message
    let result = client
        .send("test-topic", bytes::Bytes::from_static(b"hello world"))
        .await;
    assert!(
        matches!(result, SendResult::Ok),
        "send should succeed: {result:?}"
    );

    // Receive the message
    tokio::time::sleep(Duration::from_millis(50)).await;
    let records = server.recv(10).await.expect("recv should succeed").records;

    assert_eq!(records.len(), 1, "should receive exactly one record");
    assert_eq!(records[0].payload.as_ref(), b"hello world");
    assert_eq!(records[0].key.as_deref(), Some("test-topic"));

    // Cleanup
    let _ = client.close().await;
    let _ = server.close().await;
}

#[tokio::test]
async fn test_multiple_messages() {
    let port = find_available_port().await;
    let (server, client) = create_pair(port).await;

    // Send 10 messages
    for i in 0..10u32 {
        let payload = format!("message-{i}");
        let result = client.send("topic", bytes::Bytes::from(payload)).await;
        assert!(
            matches!(result, SendResult::Ok),
            "send {i} should succeed: {result:?}"
        );
    }

    // Receive all messages
    tokio::time::sleep(Duration::from_millis(100)).await;
    let records = server.recv(100).await.expect("recv should succeed").records;

    assert_eq!(records.len(), 10, "should receive all 10 records");

    // Verify ordering (sequence numbers should be monotonically increasing)
    for (i, record) in records.iter().enumerate() {
        let expected = format!("message-{i}");
        assert_eq!(
            record.payload,
            expected.as_bytes(),
            "record {i} payload mismatch"
        );
    }

    let _ = client.close().await;
    let _ = server.close().await;
}

#[tokio::test]
async fn test_large_payload() {
    let port = find_available_port().await;
    let (server, client) = create_pair(port).await;

    // Send 1MB payload
    let payload = vec![0xABu8; 1024 * 1024];
    let result = client.send("large", bytes::Bytes::from(payload)).await;
    assert!(
        matches!(result, SendResult::Ok),
        "large send should succeed: {result:?}"
    );

    tokio::time::sleep(Duration::from_millis(100)).await;
    let records = server.recv(10).await.expect("recv should succeed").records;

    assert_eq!(records.len(), 1, "should receive the large record");
    assert_eq!(records[0].payload.len(), 1024 * 1024);
    assert!(
        records[0].payload.iter().all(|&b| b == 0xAB),
        "payload should be intact"
    );

    let _ = client.close().await;
    let _ = server.close().await;
}

#[tokio::test]
async fn test_commit_is_noop() {
    let port = find_available_port().await;
    let (server, client) = create_pair(port).await;

    // Send and receive
    let _ = client
        .send("topic", bytes::Bytes::from_static(b"data"))
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let batch = server.recv(10).await.expect("recv should succeed");
    assert!(!batch.records.is_empty());

    // Commit tokens — should succeed (no-op)
    let result = server.commit(&batch.commit_tokens).await;
    assert!(result.is_ok(), "commit should succeed (no-op): {result:?}");

    let _ = client.close().await;
    let _ = server.close().await;
}

#[tokio::test]
async fn test_close_prevents_operations() {
    let port = find_available_port().await;
    let (server, client) = create_pair(port).await;

    // Close the transport
    client.close().await.expect("close should succeed");

    // Send after close should fail
    let result = client
        .send("topic", bytes::Bytes::from_static(b"data"))
        .await;
    assert!(
        matches!(result, SendResult::Fatal(_)),
        "send after close should fail: {result:?}"
    );

    // Close the server too
    server.close().await.expect("close should succeed");

    let _ = client.close().await;
    let _ = server.close().await;
}

#[tokio::test]
async fn test_health_check() {
    let port = find_available_port().await;
    let (server, client) = create_pair(port).await;

    assert!(server.is_healthy(), "server should be healthy before close");
    assert!(client.is_healthy(), "client should be healthy before close");

    server.close().await.expect("close should succeed");
    assert!(
        !server.is_healthy(),
        "server should not be healthy after close"
    );

    client.close().await.expect("close should succeed");
    assert!(
        !client.is_healthy(),
        "client should not be healthy after close"
    );
}

#[tokio::test]
async fn test_compression() {
    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let server_config = GrpcConfig::server(&addr).with_compression();
    let server = GrpcTransport::new(&server_config)
        .await
        .expect("failed to create compressed server");

    let client_config = GrpcConfig::client(&format!("http://{addr}")).with_compression();
    let client = GrpcTransport::new(&client_config)
        .await
        .expect("failed to create compressed client");

    // Send and receive with compression
    let payload = b"compressed payload test data";
    let result = client
        .send("compressed", bytes::Bytes::from_static(payload))
        .await;
    assert!(
        matches!(result, SendResult::Ok),
        "compressed send should succeed: {result:?}"
    );

    tokio::time::sleep(Duration::from_millis(50)).await;
    let records = server.recv(10).await.expect("recv should succeed").records;

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].payload.as_ref(), payload);

    let _ = client.close().await;
    let _ = server.close().await;
}

#[tokio::test]
async fn test_route_batch_native_transport() {
    // Native batch transport (Task 0.6): a whole WorkBatch's records cross the
    // wire in ONE RouteBatch RPC, payloads OPAQUE. Include a non-UTF8 binary
    // payload (NOT valid JSON or MsgPack) to prove no codec ran in transit.
    let port = find_available_port().await;
    let (server, client) = create_pair(port).await;

    let records = vec![
        Record {
            payload: bytes::Bytes::from_static(b"{\"a\":1}"),
            key: Some(Arc::from("events")),
            headers: vec![("trace".to_string(), b"abc".to_vec())],
            metadata: RecordMeta {
                timestamp_ms: Some(1_717_000_000_000),
                format: PayloadFormat::Json,
            },
        },
        Record {
            // Non-UTF8 binary: not JSON, not MsgPack -- must survive intact.
            payload: bytes::Bytes::from_static(&[0x00, 0xff, 0xfe, 0x80, 0x01]),
            key: None,
            headers: Vec::new(),
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Auto,
            },
        },
        Record {
            payload: bytes::Bytes::from_static(&[0x81, 0xa1, b'k', 0x07]),
            key: Some(Arc::from("metrics")),
            headers: Vec::new(),
            metadata: RecordMeta {
                timestamp_ms: Some(42),
                format: PayloadFormat::MsgPack,
            },
        },
    ];

    let result = client.send_batch(&records).await;
    assert!(
        matches!(result, SendResult::Ok),
        "send_batch should succeed: {result:?}"
    );

    // Records fan into the same mpsc channel the single-message path uses, so
    // the unchanged recv() trait path delivers them.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let received = server.recv(100).await.expect("recv should succeed").records;

    assert_eq!(received.len(), 3, "should receive all 3 batch records");

    // Record 0: JSON payload + key preserved.
    assert_eq!(received[0].payload.as_ref(), b"{\"a\":1}");
    assert_eq!(received[0].key.as_deref(), Some("events"));

    // Record 1: the non-UTF8 binary payload survived byte-for-byte (opaque).
    assert_eq!(
        received[1].payload.as_ref(),
        &[0x00, 0xff, 0xfe, 0x80, 0x01]
    );
    assert_eq!(received[1].key, None);

    // Record 2: MsgPack-lead payload + key preserved.
    assert_eq!(received[2].payload.as_ref(), &[0x81, 0xa1, b'k', 0x07]);
    assert_eq!(received[2].key.as_deref(), Some("metrics"));

    let _ = client.close().await;
    let _ = server.close().await;
}

/// Build a server with an explicit `recv_buffer_size` (channel capacity).
async fn create_pair_with_capacity(port: u16, capacity: usize) -> (GrpcTransport, GrpcTransport) {
    let addr = format!("127.0.0.1:{port}");

    let mut server_config = GrpcConfig::server(&addr);
    server_config.recv_buffer_size = capacity;
    let server = GrpcTransport::new(&server_config)
        .await
        .expect("failed to create server");

    let client_config = GrpcConfig::client(&format!("http://{addr}"));
    let client = GrpcTransport::new(&client_config)
        .await
        .expect("failed to create client");

    (server, client)
}

/// Phase 4 (atomicity): a `RouteBatch` larger than the free receiver capacity
/// must be rejected ALL-OR-NOTHING. With a capacity-1 channel and a 2-record
/// batch, the RPC errors (Backpressured) AND the receiver accepts ZERO records
/// -- no partial-acceptance window. This is the contract the doc-comment on
/// `send_batch` claims ("no partial-send window: the block is accepted or not
/// as a unit"). Before the fix the server enqueued record 0 then errored on
/// record 1, leaving 1 record stranded in the channel = partial acceptance +
/// duplicate-on-retry.
#[tokio::test]
async fn test_route_batch_is_atomic_under_capacity() {
    let port = find_available_port().await;
    let (server, client) = create_pair_with_capacity(port, 1).await;

    let records = vec![
        Record {
            payload: bytes::Bytes::from_static(b"{\"r\":0}"),
            key: Some(Arc::from("events")),
            headers: Vec::new(),
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Json,
            },
        },
        Record {
            payload: bytes::Bytes::from_static(b"{\"r\":1}"),
            key: Some(Arc::from("events")),
            headers: Vec::new(),
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Json,
            },
        },
    ];

    // Batch of 2 into a capacity-1 channel: cannot fit, must reject atomically.
    let result = client.send_batch(&records).await;
    assert!(
        matches!(result, SendResult::Backpressured),
        "over-capacity batch must surface as backpressure, got {result:?}"
    );

    // The receiver must have accepted ZERO records -- not 1 (partial). Drain
    // non-blocking; any record present proves a partial-acceptance window.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let received = server.recv(10).await.expect("recv should succeed").records;
    assert_eq!(
        received.len(),
        0,
        "atomic batch must accept 0 records on rejection, got {} (partial acceptance)",
        received.len()
    );

    let _ = client.close().await;
    let _ = server.close().await;
}

/// Phase 4 (atomicity): a `RouteBatch` that FITS the free capacity succeeds and
/// the receiver accepts the whole batch. Capacity 2, batch 2 -> Ok + 2 records.
#[tokio::test]
async fn test_route_batch_fits_capacity_accepts_all() {
    let port = find_available_port().await;
    let (server, client) = create_pair_with_capacity(port, 2).await;

    let records = vec![
        Record {
            payload: bytes::Bytes::from_static(b"{\"r\":0}"),
            key: Some(Arc::from("events")),
            headers: Vec::new(),
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Json,
            },
        },
        Record {
            payload: bytes::Bytes::from_static(b"{\"r\":1}"),
            key: Some(Arc::from("events")),
            headers: Vec::new(),
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Json,
            },
        },
    ];

    let result = client.send_batch(&records).await;
    assert!(
        matches!(result, SendResult::Ok),
        "in-capacity batch should succeed: {result:?}"
    );

    tokio::time::sleep(Duration::from_millis(50)).await;
    let received = server.recv(10).await.expect("recv should succeed").records;
    assert_eq!(received.len(), 2, "should receive both records");

    let _ = client.close().await;
    let _ = server.close().await;
}

/// Phase 4 (atomicity): a pressure-holding governor must reject the WHOLE
/// `RouteBatch` with `unavailable` BEFORE accepting ANY record -- consistent
/// with all-or-nothing. With the governor pinned high, a batch of 2 surfaces as
/// Backpressured and the receiver accepts ZERO records.
#[cfg(feature = "governor")]
#[tokio::test]
async fn test_route_batch_pressure_hold_accepts_nothing() {
    use hyperi_rustlib::governor::{
        Hysteresis, MemoryPressureSource, PressureSource, UnifiedPressure,
    };
    use hyperi_rustlib::memory::{MemoryGuard, MemoryGuardConfig};

    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let guard = Arc::new(MemoryGuard::new(MemoryGuardConfig {
        limit_bytes: 1000,
        pressure_threshold: 0.80,
        ..Default::default()
    }));
    guard.add_bytes(950); // 95% -> hold
    let pressure = Arc::new(UnifiedPressure::new(
        vec![Arc::new(MemoryPressureSource::new(Arc::clone(&guard))) as Arc<dyn PressureSource>],
        Hysteresis::new(0.80, 0.65).expect("valid band"),
    ));
    assert!(pressure.should_hold(), "pinned-high governor must hold");

    // Server bound to the governor, ample channel capacity (so the rejection is
    // purely pressure-driven, NOT capacity-driven).
    let mut server_config = GrpcConfig::server(&addr);
    server_config.recv_buffer_size = 100;
    let server = GrpcTransport::with_pressure(&server_config, Some(Arc::clone(&pressure)))
        .await
        .expect("failed to create server");

    let client = GrpcTransport::new(&GrpcConfig::client(&format!("http://{addr}")))
        .await
        .expect("failed to create client");

    let records = vec![
        Record {
            payload: bytes::Bytes::from_static(b"{\"r\":0}"),
            key: Some(Arc::from("events")),
            headers: Vec::new(),
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Json,
            },
        },
        Record {
            payload: bytes::Bytes::from_static(b"{\"r\":1}"),
            key: Some(Arc::from("events")),
            headers: Vec::new(),
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Json,
            },
        },
    ];

    let result = client.send_batch(&records).await;
    assert!(
        matches!(result, SendResult::Backpressured),
        "batch under pressure must surface as backpressure, got {result:?}"
    );

    tokio::time::sleep(Duration::from_millis(50)).await;
    let received = server.recv(10).await.expect("recv should succeed").records;
    assert_eq!(
        received.len(),
        0,
        "pressure-held batch must accept 0 records, got {}",
        received.len()
    );

    let _ = client.close().await;
    let _ = server.close().await;
}

#[tokio::test]
async fn test_route_batch_empty() {
    // An empty batch is a valid, harmless no-op over the wire.
    let port = find_available_port().await;
    let (server, client) = create_pair(port).await;

    let result = client.send_batch(&[]).await;
    assert!(
        matches!(result, SendResult::Ok),
        "empty send_batch should succeed: {result:?}"
    );

    let _ = client.close().await;
    let _ = server.close().await;
}

#[tokio::test]
async fn test_recv_timeout_returns_empty() {
    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    // Create server with short timeout
    let mut server_config = GrpcConfig::server(&addr);
    server_config.recv_timeout_ms = 50;
    let server = GrpcTransport::new(&server_config)
        .await
        .expect("failed to create server");

    // Recv with no messages sent — should return empty after timeout
    let records = server.recv(10).await.expect("recv should succeed").records;
    assert!(
        records.is_empty(),
        "recv with no messages should return empty, got {} records",
        records.len()
    );

    let _ = server.close().await;
}
