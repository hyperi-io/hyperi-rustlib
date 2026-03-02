// Project:   hyperi-rustlib
// File:      tests/vector_compat_integration.rs
// Purpose:   Integration tests for Vector wire protocol compatibility
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Integration tests for Vector compat gRPC transport.
//!
//! These tests start a real gRPC server with vector_compat enabled and use
//! the Vector binary (auto-downloaded via `scripts/fetch-vector.sh`) to send
//! events via Vector's native `vector` sink protocol.
//!
//! Run with:
//! ```bash
//! cargo test --test vector_compat_integration --features transport-grpc-vector-compat -- --test-threads=1
//! ```
//!
//! Requirements:
//! - `gh` or `jq` + `curl` (for auto-downloading Vector)

#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use hyperi_rustlib::transport::grpc::{GrpcConfig, GrpcTransport};
use hyperi_rustlib::transport::{SendResult, Transport};
use hyperi_rustlib::transport::VectorCompatClient;

/// Resolve the path to the Vector binary (cached via fetch-vector.sh or system PATH).
///
/// Runs the fetch script once per test binary via `OnceLock`. If the script fails
/// (offline, no `jq`, etc.), falls back to `vector` in PATH.
fn vector_binary_path() -> Option<&'static PathBuf> {
    static VECTOR_BIN: OnceLock<Option<PathBuf>> = OnceLock::new();

    VECTOR_BIN
        .get_or_init(|| {
            let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
            let fetch_script = repo_root.join("scripts/fetch-vector.sh");

            if fetch_script.exists() {
                if let Ok(output) = Command::new("bash").arg(&fetch_script).output() {
                    if output.status.success() {
                        let path = String::from_utf8_lossy(&output.stdout)
                            .trim()
                            .lines()
                            .last()
                            .unwrap_or("")
                            .to_string();
                        let binary = PathBuf::from(&path);
                        if binary.exists() {
                            return Some(binary);
                        }
                    } else {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        eprintln!("fetch-vector.sh failed: {stderr}");
                    }
                }
            }

            // Fall back to system PATH
            Command::new("vector")
                .arg("--version")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|_| PathBuf::from("vector"))
        })
        .as_ref()
}

/// Find an available port for testing.
async fn find_available_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind to ephemeral port");
    listener.local_addr().unwrap().port()
}

/// Write a Vector YAML config to a file.
fn write_vector_config(path: &Path, config_yaml: &str) {
    let mut file = std::fs::File::create(path).expect("Failed to create Vector config file");
    file.write_all(config_yaml.as_bytes())
        .expect("Failed to write Vector config");
    file.flush().expect("Failed to flush Vector config");
}

/// Run Vector as an async subprocess with timeout.
///
/// Returns (exit_status_success, stderr_output).
async fn run_vector_async(
    vector_bin: &Path,
    config_path: &Path,
    timeout_secs: u64,
) -> (bool, String) {
    let child = tokio::process::Command::new(vector_bin)
        .arg("--config")
        .arg(config_path)
        .arg("--quiet")
        .env("VECTOR_LOG", "warn")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("Failed to spawn vector binary");

    let result =
        tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await;

    match result {
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            (output.status.success(), stderr)
        }
        Ok(Err(e)) => {
            panic!("Failed to wait for vector process: {e}");
        }
        Err(elapsed) => {
            panic!(
                "Vector did not exit within {timeout_secs}s ({elapsed}) — likely stuck retrying failed deliveries"
            );
        }
    }
}

/// Validate a Vector config file. Panics if validation fails.
fn validate_vector_config(vector_bin: &Path, config_path: &Path) {
    let output = Command::new(vector_bin)
        .arg("validate")
        .arg("--no-environment")
        .arg("--config-yaml")
        .arg(config_path)
        .output()
        .expect("Failed to run vector validate");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "Vector config validation failed:\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// Assert no ERROR lines appear in Vector's stderr output.
fn assert_no_vector_errors(stderr: &str) {
    let error_lines: Vec<&str> = stderr.lines().filter(|l| l.contains("ERROR")).collect();

    assert!(
        error_lines.is_empty(),
        "Vector produced errors:\n{}",
        error_lines.join("\n")
    );
}

// =============================================================================
// Vector gRPC Sink → GrpcTransport (vector_compat) Tests
// =============================================================================

/// Test: Vector binary sends events via its native `vector` sink to our
/// GrpcTransport server with vector_compat enabled.
///
/// This verifies end-to-end compatibility: Vector → PushEvents RPC →
/// VectorCompatService → message channel → recv().
#[tokio::test]
async fn test_vector_grpc_sink_to_transport() {
    let Some(vector_bin) = vector_binary_path() else {
        eprintln!("Skipping test: vector binary not available");
        return;
    };

    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    // Start server with vector_compat
    let mut server_config = GrpcConfig::server(&addr);
    server_config.recv_timeout_ms = 5000;
    let server_config = server_config.with_vector_compat();

    let server = GrpcTransport::new(&server_config)
        .await
        .expect("failed to create vector-compat server");

    // Give server time to bind
    tokio::time::sleep(Duration::from_millis(200)).await;

    let tmp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let config_path = tmp_dir.path().join("vector.yaml");
    let data_dir = tmp_dir.path().join("vector-data");
    let data_dir_str = data_dir.to_str().unwrap();

    let vector_config = format!(
        r#"
data_dir: "{data_dir_str}"

sources:
  demo:
    type: demo_logs
    format: json
    count: 5
    interval: 0.1

sinks:
  receiver:
    type: vector
    inputs: ["demo"]
    address: "127.0.0.1:{port}"
    compression: false
    healthcheck:
      enabled: false
"#
    );

    write_vector_config(&config_path, &vector_config);
    validate_vector_config(vector_bin, &config_path);

    let (success, stderr) = run_vector_async(vector_bin, &config_path, 30).await;

    if !success {
        eprintln!("Vector stderr: {stderr}");
    }

    assert_no_vector_errors(&stderr);

    // Receive events from the server
    let messages = server.recv(100).await.expect("recv should succeed");

    assert!(
        !messages.is_empty(),
        "should receive at least one event from Vector"
    );

    // Each message payload should be valid JSON (Vector demo_logs are JSON)
    for msg in &messages {
        let json: serde_json::Value =
            serde_json::from_slice(&msg.payload).expect("payload should be valid JSON");
        assert!(json.is_object(), "each event should be a JSON object");
    }

    let _ = server.close().await;
}

/// Test: Vector sends multiple events and all are received with correct count.
#[tokio::test]
async fn test_vector_grpc_multiple_events() {
    let Some(vector_bin) = vector_binary_path() else {
        eprintln!("Skipping test: vector binary not available");
        return;
    };

    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let mut server_config = GrpcConfig::server(&addr);
    server_config.recv_timeout_ms = 5000;
    let server_config = server_config.with_vector_compat();

    let server = GrpcTransport::new(&server_config)
        .await
        .expect("failed to create vector-compat server");

    tokio::time::sleep(Duration::from_millis(200)).await;

    let tmp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let config_path = tmp_dir.path().join("vector.yaml");
    let data_dir = tmp_dir.path().join("vector-data");
    let data_dir_str = data_dir.to_str().unwrap();

    let vector_config = format!(
        r#"
data_dir: "{data_dir_str}"

sources:
  demo:
    type: demo_logs
    format: json
    count: 20
    interval: 0.05

sinks:
  receiver:
    type: vector
    inputs: ["demo"]
    address: "127.0.0.1:{port}"
    compression: false
    healthcheck:
      enabled: false
"#
    );

    write_vector_config(&config_path, &vector_config);
    validate_vector_config(vector_bin, &config_path);

    let (success, stderr) = run_vector_async(vector_bin, &config_path, 30).await;

    if !success {
        eprintln!("Vector stderr: {stderr}");
    }

    assert_no_vector_errors(&stderr);

    // Collect all messages (may need multiple recv calls)
    let mut all_messages = Vec::new();
    loop {
        let messages = server.recv(100).await.expect("recv should succeed");
        if messages.is_empty() {
            break;
        }
        all_messages.extend(messages);
    }

    assert_eq!(
        all_messages.len(),
        20,
        "should receive all 20 events from Vector"
    );

    let _ = server.close().await;
}

/// Test: Both native DFE client and Vector CLI can send to the same server.
///
/// Verifies that the vector_compat server accepts events on both the
/// DFE native proto and the Vector proto simultaneously.
#[tokio::test]
async fn test_vector_and_native_coexist() {
    let Some(vector_bin) = vector_binary_path() else {
        eprintln!("Skipping test: vector binary not available");
        return;
    };

    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let mut server_config = GrpcConfig::server(&addr);
    server_config.recv_timeout_ms = 5000;
    let server_config = server_config.with_vector_compat();

    let server = GrpcTransport::new(&server_config)
        .await
        .expect("failed to create vector-compat server");

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Send native DFE messages
    let client_config = GrpcConfig::client(&format!("http://127.0.0.1:{port}"));
    let client = GrpcTransport::new(&client_config)
        .await
        .expect("failed to create native DFE client");

    for i in 0..3u32 {
        let payload = format!("native-{i}");
        let result = client.send("topic", payload.as_bytes()).await;
        assert!(
            matches!(result, SendResult::Ok),
            "native send {i} should succeed: {result:?}"
        );
    }

    // Send Vector events via Vector CLI
    let tmp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let config_path = tmp_dir.path().join("vector.yaml");
    let data_dir = tmp_dir.path().join("vector-data");
    let data_dir_str = data_dir.to_str().unwrap();

    let vector_config = format!(
        r#"
data_dir: "{data_dir_str}"

sources:
  demo:
    type: demo_logs
    format: json
    count: 3
    interval: 0.1

sinks:
  receiver:
    type: vector
    inputs: ["demo"]
    address: "127.0.0.1:{port}"
    compression: false
    healthcheck:
      enabled: false
"#
    );

    write_vector_config(&config_path, &vector_config);
    validate_vector_config(vector_bin, &config_path);

    let (success, stderr) = run_vector_async(vector_bin, &config_path, 30).await;

    if !success {
        eprintln!("Vector stderr: {stderr}");
    }

    assert_no_vector_errors(&stderr);

    // Collect all messages
    let mut all_messages = Vec::new();
    loop {
        let messages = server.recv(100).await.expect("recv should succeed");
        if messages.is_empty() {
            break;
        }
        all_messages.extend(messages);
    }

    // Should have 3 native + 3 Vector = 6 total
    assert_eq!(
        all_messages.len(),
        6,
        "should receive 3 native + 3 vector = 6 total events"
    );

    let _ = client.close().await;
    let _ = server.close().await;
}

/// Test: VectorCompatClient can send events and they arrive at the server.
///
/// Uses the library's VectorCompatClient (not the Vector binary) to verify
/// the client-side API works correctly.
#[tokio::test]
async fn test_vector_compat_client_send() {
    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let mut server_config = GrpcConfig::server(&addr);
    server_config.recv_timeout_ms = 2000;
    let server_config = server_config.with_vector_compat();

    let server = GrpcTransport::new(&server_config)
        .await
        .expect("failed to create vector-compat server");

    tokio::time::sleep(Duration::from_millis(200)).await;

    let client = VectorCompatClient::connect_lazy(&format!("http://127.0.0.1:{port}"))
        .expect("failed to create VectorCompatClient");

    // Send JSON events
    let events: Vec<serde_json::Value> = (0..5)
        .map(|i| {
            serde_json::json!({
                "message": format!("event-{i}"),
                "level": "info",
                "timestamp": "2026-03-02T00:00:00Z"
            })
        })
        .collect();

    client
        .send_events(&events)
        .await
        .expect("send_events should succeed");

    tokio::time::sleep(Duration::from_millis(100)).await;

    let messages = server.recv(100).await.expect("recv should succeed");

    assert_eq!(messages.len(), 5, "should receive all 5 events");

    // Verify JSON payloads contain our fields
    for (i, msg) in messages.iter().enumerate() {
        let json: serde_json::Value =
            serde_json::from_slice(&msg.payload).expect("payload should be valid JSON");
        let message = json.get("message").and_then(|v| v.as_str());
        assert_eq!(
            message,
            Some(&*format!("event-{i}")),
            "event {i} message field should match"
        );
    }

    let _ = server.close().await;
}

/// Test: VectorCompatClient sends events to a Vector binary running as source.
///
/// Starts Vector with a `vector` source (acting as server) and a `file` sink,
/// then uses our VectorCompatClient to push events to it. Verifies Vector
/// receives and writes the events to the output file.
#[tokio::test]
async fn test_vector_compat_client_to_vector_source() {
    let Some(vector_bin) = vector_binary_path() else {
        eprintln!("Skipping test: vector binary not available");
        return;
    };

    let port = find_available_port().await;

    let tmp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let config_path = tmp_dir.path().join("vector.yaml");
    let output_path = tmp_dir.path().join("output.json");
    let data_dir = tmp_dir.path().join("vector-data");
    let data_dir_str = data_dir.to_str().unwrap();
    let output_path_str = output_path.to_str().unwrap();

    // Vector config: vector source (server) → file sink
    let vector_config = format!(
        r#"
data_dir: "{data_dir_str}"

sources:
  grpc_in:
    type: vector
    address: "127.0.0.1:{port}"

sinks:
  file_out:
    type: file
    inputs: ["grpc_in"]
    path: "{output_path_str}"
    encoding:
      codec: json
"#
    );

    write_vector_config(&config_path, &vector_config);
    validate_vector_config(vector_bin, &config_path);

    // Start Vector as a background server
    let mut child = tokio::process::Command::new(vector_bin)
        .arg("--config")
        .arg(&config_path)
        .arg("--quiet")
        .env("VECTOR_LOG", "warn")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("Failed to spawn vector binary");

    // Wait for Vector to start listening
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Send events via our VectorCompatClient
    let client = VectorCompatClient::connect_lazy(&format!("http://127.0.0.1:{port}"))
        .expect("failed to create VectorCompatClient");

    let events: Vec<serde_json::Value> = (0..5)
        .map(|i| {
            serde_json::json!({
                "message": format!("client-event-{i}"),
                "source": "rustlib-test"
            })
        })
        .collect();

    client
        .send_events(&events)
        .await
        .expect("send_events to Vector source should succeed");

    // Give Vector time to flush to file
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Kill Vector
    child.kill().await.expect("failed to kill vector");
    let _ = child.wait().await;

    // Verify output file has our events
    // Vector file sink may split across lines or use a single NDJSON file
    let output = std::fs::read_to_string(&output_path)
        .unwrap_or_else(|_| {
            // Vector may append a timestamp suffix to the path
            let entries: Vec<_> = std::fs::read_dir(tmp_dir.path())
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .is_some_and(|s| s.starts_with("output"))
                })
                .collect();
            if let Some(entry) = entries.first() {
                std::fs::read_to_string(entry.path()).unwrap_or_default()
            } else {
                String::new()
            }
        });

    assert!(
        !output.is_empty(),
        "Vector should have written events to the output file"
    );

    // Count JSON lines with our marker
    let matching_lines: Vec<&str> = output
        .lines()
        .filter(|line| line.contains("rustlib-test"))
        .collect();

    assert_eq!(
        matching_lines.len(),
        5,
        "should find 5 events with our source marker in Vector output"
    );
}

/// Test: VectorCompatClient health check returns true when server is running.
#[tokio::test]
async fn test_vector_compat_client_health_check() {
    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let server_config = GrpcConfig::server(&addr).with_vector_compat();

    let server = GrpcTransport::new(&server_config)
        .await
        .expect("failed to create vector-compat server");

    tokio::time::sleep(Duration::from_millis(200)).await;

    let client = VectorCompatClient::connect_lazy(&format!("http://127.0.0.1:{port}"))
        .expect("failed to create VectorCompatClient");

    let healthy = client
        .health_check()
        .await
        .expect("health_check should succeed");

    assert!(healthy, "server should be healthy");

    let _ = server.close().await;
}
