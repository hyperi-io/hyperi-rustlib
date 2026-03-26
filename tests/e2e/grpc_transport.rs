// Project:   hyperi-rustlib
// File:      tests/e2e/grpc_transport.rs
// Purpose:   Integration tests for gRPC transport (bidirectional client/server)
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Integration tests for the gRPC transport layer.
//!
//! These tests start real tonic gRPC servers and clients to verify
//! end-to-end message delivery. Run with `--test-threads=1` to avoid
//! port conflicts.
//!
//! `cargo test --test e2e_tests --features transport-grpc -- --test-threads=1`

use std::time::Duration;

use hyperi_rustlib::transport::grpc::{GrpcConfig, GrpcTransport};
use hyperi_rustlib::transport::{SendResult, TransportBase, TransportReceiver, TransportSender};

/// Find an available port for testing.
async fn find_available_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind to ephemeral port");
    listener.local_addr().unwrap().port()
}

/// Create a server+client pair on a given port.
async fn create_pair(port: u16) -> (GrpcTransport, GrpcTransport) {
    let addr = format!("127.0.0.1:{port}");

    let server_config = GrpcConfig::server(&addr);
    let server = GrpcTransport::new(&server_config)
        .await
        .expect("failed to create server");

    // Give server time to bind
    tokio::time::sleep(Duration::from_millis(100)).await;

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
    let result = client.send("test-topic", b"hello world").await;
    assert!(
        matches!(result, SendResult::Ok),
        "send should succeed: {result:?}"
    );

    // Receive the message
    tokio::time::sleep(Duration::from_millis(50)).await;
    let messages = server.recv(10).await.expect("recv should succeed");

    assert_eq!(messages.len(), 1, "should receive exactly one message");
    assert_eq!(messages[0].payload, b"hello world");
    assert_eq!(messages[0].key.as_deref(), Some("test-topic"));

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
        let result = client.send("topic", payload.as_bytes()).await;
        assert!(
            matches!(result, SendResult::Ok),
            "send {i} should succeed: {result:?}"
        );
    }

    // Receive all messages
    tokio::time::sleep(Duration::from_millis(100)).await;
    let messages = server.recv(100).await.expect("recv should succeed");

    assert_eq!(messages.len(), 10, "should receive all 10 messages");

    // Verify ordering (sequence numbers should be monotonically increasing)
    for (i, msg) in messages.iter().enumerate() {
        let expected = format!("message-{i}");
        assert_eq!(
            msg.payload,
            expected.as_bytes(),
            "message {i} payload mismatch"
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
    let result = client.send("large", &payload).await;
    assert!(
        matches!(result, SendResult::Ok),
        "large send should succeed: {result:?}"
    );

    tokio::time::sleep(Duration::from_millis(100)).await;
    let messages = server.recv(10).await.expect("recv should succeed");

    assert_eq!(messages.len(), 1, "should receive the large message");
    assert_eq!(messages[0].payload.len(), 1024 * 1024);
    assert!(
        messages[0].payload.iter().all(|&b| b == 0xAB),
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
    let _ = client.send("topic", b"data").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let messages = server.recv(10).await.expect("recv should succeed");
    assert!(!messages.is_empty());

    // Commit tokens — should succeed (no-op)
    let tokens: Vec<_> = messages.iter().map(|m| m.token.clone()).collect();
    let result = server.commit(&tokens).await;
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
    let result = client.send("topic", b"data").await;
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

    tokio::time::sleep(Duration::from_millis(100)).await;

    let client_config = GrpcConfig::client(&format!("http://{addr}")).with_compression();
    let client = GrpcTransport::new(&client_config)
        .await
        .expect("failed to create compressed client");

    // Send and receive with compression
    let payload = b"compressed payload test data";
    let result = client.send("compressed", payload).await;
    assert!(
        matches!(result, SendResult::Ok),
        "compressed send should succeed: {result:?}"
    );

    tokio::time::sleep(Duration::from_millis(50)).await;
    let messages = server.recv(10).await.expect("recv should succeed");

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].payload, payload);

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

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Recv with no messages sent — should return empty after timeout
    let messages = server.recv(10).await.expect("recv should succeed");
    assert!(
        messages.is_empty(),
        "recv with no messages should return empty, got {} messages",
        messages.len()
    );

    let _ = server.close().await;
}
