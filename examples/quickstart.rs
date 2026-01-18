// Project:   hs-rustlib
// File:      examples/quickstart.rs
// Purpose:   Minimal quickstart example
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Quickstart example showing the most common usage patterns.
//!
//! Run with:
//! ```bash
//! cargo run --example quickstart
//! ```

use hs_rustlib::config::{Config, ConfigOptions};
use hs_rustlib::env::Environment;
use hs_rustlib::logger;

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
