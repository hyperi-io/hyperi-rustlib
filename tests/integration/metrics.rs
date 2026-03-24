// Project:   hyperi-rustlib
// File:      tests/integration/metrics.rs
// Purpose:   Integration tests for metrics HTTP server
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Integration tests for the metrics HTTP server.
//!
//! These tests must run serially because the metrics crate uses a global recorder.
//! Run with: `cargo test --test integration_tests -- --test-threads=1`

use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use hyperi_rustlib::metrics::{MetricsConfig, MetricsError, MetricsManager};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// Global lock to ensure tests run one at a time.
static TEST_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Global manager created once for all tests.
static MANAGER: LazyLock<Mutex<Option<MetricsManager>>> = LazyLock::new(|| Mutex::new(None));

/// Initialise the global manager if not already done.
fn init_manager() {
    let mut guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let config = MetricsConfig {
            namespace: "test".to_string(),
            enable_process_metrics: false,
            enable_container_metrics: false,
            ..Default::default()
        };
        *guard = Some(MetricsManager::with_config(config));
    }
}

/// Find an available port for testing.
async fn find_available_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind to ephemeral port");
    let addr = listener.local_addr().expect("failed to get local address");
    addr.port()
}

/// Send an HTTP GET request and return status line and body.
async fn http_get(addr: &str, path: &str) -> (String, String) {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("failed to connect to server");

    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("failed to write request");

    let mut reader = BufReader::new(&mut stream);

    // Read status line
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .await
        .expect("failed to read status line");

    // Skip headers until empty line
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .expect("failed to read header");
        if line == "\r\n" || line.is_empty() {
            break;
        }
    }

    // Read body
    let mut body = String::new();
    reader
        .read_to_string(&mut body)
        .await
        .expect("failed to read body");

    (status_line.trim().to_string(), body)
}

#[tokio::test]
async fn test_01_server_start_and_stop() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let mut guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_mut().expect("manager not initialised");

    // Start server
    manager
        .start_server(&addr)
        .await
        .expect("failed to start server");

    // Give server time to bind
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify server is running by connecting
    let connect_result = timeout(Duration::from_secs(1), TcpStream::connect(&addr)).await;
    assert!(
        connect_result.is_ok() && connect_result.unwrap().is_ok(),
        "server should be accepting connections"
    );

    // Stop server
    manager.stop_server().await.expect("failed to stop server");

    // Give server time to shut down
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn test_02_server_already_running_error() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let mut guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_mut().expect("manager not initialised");

    // Start server first time
    manager
        .start_server(&addr)
        .await
        .expect("failed to start server");

    // Attempt to start again
    let result = manager.start_server(&addr).await;
    assert!(matches!(result, Err(MetricsError::AlreadyRunning)));

    // Cleanup
    let _ = manager.stop_server().await;
}

#[tokio::test]
async fn test_03_server_not_running_error() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let mut guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_mut().expect("manager not initialised");

    // Ensure server is stopped
    let _ = manager.stop_server().await;

    // Attempt to stop without running server
    let result = manager.stop_server().await;
    assert!(matches!(result, Err(MetricsError::NotRunning)));
}

#[tokio::test]
async fn test_04_metrics_endpoint() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let mut guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_mut().expect("manager not initialised");

    // Create and record some metrics
    let counter = manager.counter("requests_total", "Total requests");
    let gauge = manager.gauge("active_connections", "Active connections");
    let histogram = manager.histogram("request_duration_seconds", "Request latency");

    counter.increment(5);
    gauge.set(42.0);
    histogram.record(0.123);

    // Start server
    manager
        .start_server(&addr)
        .await
        .expect("failed to start server");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Fetch metrics
    let (status, body) = http_get(&addr, "/metrics").await;

    assert!(status.contains("200 OK"), "expected 200 OK, got: {status}");

    // Verify counter is present
    assert!(
        body.contains("test_requests_total"),
        "counter not found in metrics output: {body}"
    );

    // Verify gauge is present
    assert!(
        body.contains("test_active_connections"),
        "gauge not found in metrics output: {body}"
    );

    // Verify histogram is present
    assert!(
        body.contains("test_request_duration_seconds"),
        "histogram not found in metrics output: {body}"
    );

    // Cleanup
    let _ = manager.stop_server().await;
}

#[tokio::test]
async fn test_05_healthz_endpoint() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let mut guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_mut().expect("manager not initialised");

    manager
        .start_server(&addr)
        .await
        .expect("failed to start server");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Test /healthz
    let (status, body) = http_get(&addr, "/healthz").await;
    assert!(status.contains("200 OK"), "expected 200 OK for /healthz");
    assert!(
        body.contains(r#""status":"alive""#),
        "expected alive status in body"
    );

    // Cleanup
    let _ = manager.stop_server().await;
}

#[tokio::test]
async fn test_06_health_live_endpoint() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let mut guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_mut().expect("manager not initialised");

    manager
        .start_server(&addr)
        .await
        .expect("failed to start server");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Test /health/live
    let (status, body) = http_get(&addr, "/health/live").await;
    assert!(
        status.contains("200 OK"),
        "expected 200 OK for /health/live"
    );
    assert!(
        body.contains(r#""status":"alive""#),
        "expected alive status in body"
    );

    // Cleanup
    let _ = manager.stop_server().await;
}

#[tokio::test]
async fn test_07_readyz_endpoint() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let mut guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_mut().expect("manager not initialised");

    manager
        .start_server(&addr)
        .await
        .expect("failed to start server");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Test /readyz
    let (status, body) = http_get(&addr, "/readyz").await;
    assert!(status.contains("200 OK"), "expected 200 OK for /readyz");
    assert!(
        body.contains(r#""status":"ready""#),
        "expected ready status in body"
    );

    // Cleanup
    let _ = manager.stop_server().await;
}

#[tokio::test]
async fn test_08_health_ready_endpoint() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let mut guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_mut().expect("manager not initialised");

    manager
        .start_server(&addr)
        .await
        .expect("failed to start server");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Test /health/ready
    let (status, body) = http_get(&addr, "/health/ready").await;
    assert!(
        status.contains("200 OK"),
        "expected 200 OK for /health/ready"
    );
    assert!(
        body.contains(r#""status":"ready""#),
        "expected ready status in body"
    );

    // Cleanup
    let _ = manager.stop_server().await;
}

#[tokio::test]
async fn test_09_404_for_unknown_path() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let mut guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_mut().expect("manager not initialised");

    manager
        .start_server(&addr)
        .await
        .expect("failed to start server");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Test unknown path
    let (status, body) = http_get(&addr, "/unknown").await;
    assert!(
        status.contains("404 Not Found"),
        "expected 404 for unknown path, got: {status}"
    );
    assert!(body.contains("Not Found"), "expected 'Not Found' in body");

    // Cleanup
    let _ = manager.stop_server().await;
}

#[tokio::test]
async fn test_10_render_without_server() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_ref().expect("manager not initialised");

    // Create and record metrics
    let counter = manager.counter("render_test_counter", "A test counter");
    counter.increment(10);

    // Render metrics directly
    let output = manager.render();

    // Verify the counter is in the output
    assert!(
        output.contains("test_render_test_counter"),
        "counter should be in rendered output: {output}"
    );
}

#[tokio::test]
async fn test_11_counter_increment() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_ref().expect("manager not initialised");

    let counter = manager.counter("hits", "Number of hits");

    counter.increment(1);
    counter.increment(5);
    counter.increment(4);

    let output = manager.render();

    // Counter should show total of 10
    assert!(
        output.contains("test_hits 10"),
        "counter should show 10, got: {output}"
    );
}

#[tokio::test]
async fn test_12_gauge_set() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_ref().expect("manager not initialised");

    let gauge = manager.gauge("temperature", "Current temperature");

    gauge.set(25.5);

    let output = manager.render();

    // Gauge should show 25.5
    assert!(
        output.contains("test_temperature 25.5"),
        "gauge should show 25.5, got: {output}"
    );

    // Update gauge
    gauge.set(30.0);
    let output = manager.render();

    assert!(
        output.contains("test_temperature 30"),
        "gauge should show 30, got: {output}"
    );
}

#[tokio::test]
async fn test_13_histogram_record() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_ref().expect("manager not initialised");

    let histogram = manager.histogram("hist_latency", "Request latency");

    histogram.record(0.1);
    histogram.record(0.2);
    histogram.record(0.3);

    let output = manager.render();

    // The metrics crate uses summary type by default for histograms
    // Verify sum and count are present
    assert!(
        output.contains("test_hist_latency_sum"),
        "histogram sum not found in output: {output}"
    );
    assert!(
        output.contains("test_hist_latency_count 3"),
        "histogram count should be 3, got: {output}"
    );
}

/// Test invalid address handling.
#[tokio::test]
async fn test_14_invalid_address_error() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let mut guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_mut().expect("manager not initialised");

    // Attempt to bind to invalid address
    let result = manager.start_server("invalid:address:format").await;
    assert!(
        matches!(result, Err(MetricsError::ServerError(_))),
        "expected ServerError for invalid address, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Graceful shutdown tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_15_shutdown_stops_accepting() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let mut guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_mut().expect("manager not initialised");

    manager
        .start_server(&addr)
        .await
        .expect("failed to start server");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify server is accepting
    let connect = timeout(Duration::from_millis(500), TcpStream::connect(&addr)).await;
    assert!(
        connect.is_ok() && connect.unwrap().is_ok(),
        "server should accept connections before shutdown"
    );

    // Stop the server
    manager.stop_server().await.expect("failed to stop server");
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify TCP connect is refused after shutdown
    let connect = timeout(Duration::from_millis(500), TcpStream::connect(&addr)).await;
    let refused = match connect {
        Ok(Ok(_)) => false,          // Connected — not shut down
        Ok(Err(_)) | Err(_) => true, // Connection refused or timed out
    };
    assert!(refused, "server should refuse connections after shutdown");
}

#[tokio::test]
async fn test_16_rapid_start_stop_cycle() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let mut guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_mut().expect("manager not initialised");

    // Rapid start/stop 3 times on different ports
    for _ in 0..3 {
        let port = find_available_port().await;
        let addr = format!("127.0.0.1:{port}");

        manager
            .start_server(&addr)
            .await
            .expect("failed to start server");
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Verify it's running
        let (status, _) = http_get(&addr, "/healthz").await;
        assert!(
            status.contains("200 OK"),
            "server should respond during cycle"
        );

        manager.stop_server().await.expect("failed to stop server");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn test_17_render_survives_server_stop() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let mut guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_mut().expect("manager not initialised");

    // Record a metric
    let counter = manager.counter("shutdown_test", "Shutdown test counter");
    counter.increment(99);

    // Start and stop the server
    manager
        .start_server(&addr)
        .await
        .expect("failed to start server");
    tokio::time::sleep(Duration::from_millis(50)).await;
    manager.stop_server().await.expect("failed to stop server");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // render() should still work after server stop
    let output = manager.render();
    assert!(
        output.contains("test_shutdown_test"),
        "render should work after server stop: {output}"
    );
}

#[tokio::test]
async fn test_18_concurrent_requests_during_shutdown() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_manager();

    let port = find_available_port().await;
    let addr = format!("127.0.0.1:{port}");

    let mut guard = MANAGER.lock().unwrap_or_else(|e| e.into_inner());
    let manager = guard.as_mut().expect("manager not initialised");

    manager
        .start_server(&addr)
        .await
        .expect("failed to start server");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Spawn concurrent HTTP requests
    let addr_clone = addr.clone();
    let handles: Vec<_> = (0..5)
        .map(|_| {
            let a = addr_clone.clone();
            tokio::spawn(async move {
                let result = timeout(Duration::from_secs(2), async {
                    // Connection or HTTP error is acceptable during shutdown
                    let _ = TcpStream::connect(&a).await;
                })
                .await;
                result.is_ok() // No timeout/panic = success
            })
        })
        .collect();

    // Trigger shutdown while requests are in flight
    tokio::time::sleep(Duration::from_millis(10)).await;
    let _ = manager.stop_server().await;

    // All handles should complete without panic or hang
    let overall = timeout(Duration::from_secs(5), async {
        for handle in handles {
            let _ = handle.await;
        }
    })
    .await;
    assert!(
        overall.is_ok(),
        "concurrent requests should not hang during shutdown"
    );
}
