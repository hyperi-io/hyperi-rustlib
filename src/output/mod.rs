// Project:   hyperi-rustlib
// File:      src/output/mod.rs
// Purpose:   File output sink module
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! File output sink for local NDJSON event writing.
//!
//! Provides a simple file-based output for DFE services — useful for testing,
//! bare-metal deployments, and debugging where Kafka is not available.
//!
//! ## Example
//!
//! ```rust,no_run
//! use hyperi_rustlib::output::{FileOutput, FileOutputConfig};
//!
//! let config = FileOutputConfig {
//!     enabled: true,
//!     path: "/tmp/dfe/output".into(),
//!     ..Default::default()
//! };
//!
//! let output = FileOutput::new(&config, "my-service").expect("create output");
//! output.write(b"{\"event\":\"login\"}").expect("write");
//! ```

mod config;
mod error;
mod file;

pub use config::FileOutputConfig;
pub use error::OutputError;
pub use file::FileOutput;
