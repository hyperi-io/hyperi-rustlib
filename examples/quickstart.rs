// Project:   scalo
// File:      examples/quickstart.rs
// Purpose:   Minimal quickstart example
// Language:  Rust
//
// License:   Apache-2.0
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Quickstart example showing the most common usage patterns.
//!
//! Run with:
//! ```bash
//! cargo run --example quickstart
//! ```

use scalo::config::{Config, ConfigOptions};
use scalo::env::Environment;
use scalo::logger;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Detect environment
    let env = Environment::detect();
    println!("Running in: {env}");

    // Load configuration (uses APP_ prefix by default)
    let config = Config::new(ConfigOptions {
        env_prefix: "APP".to_string(),
        ..Default::default()
    })?;

    // Initialise logging (auto-detects format based on environment)
    logger::setup_default()?;

    // Log with structured fields
    tracing::info!(
        environment = %env,
        "Application started"
    );

    // Access configuration
    let debug_mode = config.get_bool("debug").unwrap_or(false);
    tracing::debug!(debug_mode, "Configuration loaded");

    tracing::info!("Quickstart complete");
    Ok(())
}
