// Project:   hyperi-rustlib
// File:      examples/full_demo.rs
// Purpose:   Demonstrate all hyperi-rustlib core features
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Full demonstration of hyperi-rustlib features.
//!
//! This example shows how to use:
//! - Environment detection
//! - Runtime paths
//! - Configuration cascade
//! - Structured logging
//! - Prometheus metrics
//!
//! Run with:
//! ```bash
//! cargo run --example full_demo
//! ```
//!
//! Or with environment variables:
//! ```bash
//! DEMO_LOG_LEVEL=debug DEMO_SERVER__PORT=9090 cargo run --example full_demo
//! ```

use std::time::Duration;

use hyperi_rustlib::config::{Config, ConfigOptions};
use hyperi_rustlib::env::{Environment, get_app_env};
use hyperi_rustlib::logger;
use hyperi_rustlib::metrics::{MetricsConfig, MetricsManager};
use hyperi_rustlib::runtime::RuntimePaths;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // =========================================================================
    // 1. Environment Detection
    // =========================================================================
    println!("=== Environment Detection ===\n");

    let environment = Environment::detect();
    println!("Detected environment: {environment}");
    println!("Is container: {}", environment.is_container());
    println!("Is Kubernetes: {}", environment.is_kubernetes());
    println!("App environment: {}", get_app_env());
    println!();

    // =========================================================================
    // 2. Runtime Paths
    // =========================================================================
    println!("=== Runtime Paths ===\n");

    let paths = RuntimePaths::discover();
    println!("Config directory: {}", paths.config_dir.display());
    println!("Data directory: {}", paths.data_dir.display());
    println!("Secrets directory: {}", paths.secrets_dir.display());
    println!("Logs directory: {}", paths.logs_dir.display());
    println!("Temp directory: {}", paths.temp_dir.display());
    println!();

    // =========================================================================
    // 3. Configuration Cascade
    // =========================================================================
    println!("=== Configuration (7-Layer Cascade) ===\n");

    // Load configuration with DEMO_ prefix for environment variables
    let config = Config::new(ConfigOptions {
        env_prefix: "DEMO".to_string(),
        load_dotenv: true,
        ..Default::default()
    })?;

    // Display configuration values (showing cascade in action)
    println!("Configuration values:");
    println!(
        "  log_level: {:?}",
        config.get_string("log_level").unwrap_or_default()
    );
    println!(
        "  log_format: {:?}",
        config.get_string("log_format").unwrap_or_default()
    );

    // Nested configuration (use DEMO_SERVER__PORT=9090)
    if let Some(port) = config.get_int("server.port") {
        println!("  server.port: {port}");
    }

    // Duration parsing
    if let Some(timeout) = config.get_duration("timeout") {
        println!("  timeout: {timeout:?}");
    }
    println!();

    // =========================================================================
    // 4. Structured Logging
    // =========================================================================
    println!("=== Structured Logging ===\n");

    // Initialise logger with defaults (auto-detects format)
    logger::setup_default()?;

    // Log some messages
    tracing::info!("Application starting");
    tracing::debug!(environment = %environment, "Runtime environment detected");
    tracing::info!(
        config_dir = ?paths.config_dir,
        data_dir = ?paths.data_dir,
        "Paths configured"
    );
    println!();

    // =========================================================================
    // 5. Prometheus Metrics
    // =========================================================================
    println!("=== Prometheus Metrics ===\n");

    let metrics_config = MetricsConfig {
        namespace: "demo".to_string(),
        enable_process_metrics: true,
        enable_container_metrics: environment.is_container(),
        update_interval: Duration::from_secs(15),
        #[cfg(feature = "otel-metrics")]
        otel: hyperi_rustlib::OtelMetricsConfig::default(),
    };

    let mut manager = MetricsManager::with_config(metrics_config);

    // Create application metrics
    let requests_total = manager.counter("http_requests_total", "Total HTTP requests");
    let active_connections = manager.gauge("active_connections", "Current active connections");
    let request_duration = manager.histogram("http_request_duration_seconds", "Request latency");

    // Simulate some activity
    requests_total.increment(100);
    active_connections.set(42.0);
    request_duration.record(0.025);
    request_duration.record(0.150);
    request_duration.record(0.005);

    // Update process/container metrics
    manager.update();

    // Render metrics
    println!("Prometheus metrics output:\n");
    let metrics_output = manager.render();
    for line in metrics_output.lines().take(30) {
        println!("  {line}");
    }
    if metrics_output.lines().count() > 30 {
        println!("  ... (truncated)");
    }
    println!();

    // =========================================================================
    // 6. Metrics Server (Optional)
    // =========================================================================
    println!("=== Metrics Server ===\n");

    let metrics_port = config.get_int("metrics.port").unwrap_or(9090);
    let metrics_addr = format!("127.0.0.1:{metrics_port}");

    println!("Starting metrics server on {metrics_addr}");
    println!("Endpoints:");
    println!("  - http://{metrics_addr}/metrics");
    println!("  - http://{metrics_addr}/healthz");
    println!("  - http://{metrics_addr}/readyz");
    println!();

    // Start the server
    manager.start_server(&metrics_addr).await?;

    tracing::info!(
        addr = %metrics_addr,
        "Metrics server started"
    );

    // Keep running for a few seconds to allow testing
    println!("Server running for 5 seconds (try: curl http://{metrics_addr}/metrics)");
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Graceful shutdown
    manager.stop_server().await?;
    tracing::info!("Metrics server stopped");

    println!("\n=== Demo Complete ===");
    Ok(())
}
