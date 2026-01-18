// Project:   hs-rustlib
// File:      src/spool/mod.rs
// Purpose:   Disk-backed async FIFO queue with optional compression
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Disk-backed async FIFO queue with optional zstd compression.
//!
//! This module provides a persistent queue for spooling data to disk,
//! useful for buffering data when downstream systems are unavailable
//! or for implementing store-and-forward patterns.
//!
//! Built on [yaque](https://crates.io/crates/yaque), a fast, async,
//! persistent queue with transactional semantics.
//!
//! ## Features
//!
//! - Persistent storage survives restarts
//! - Transactional writes (crash-safe)
//! - Optional zstd compression for reduced disk usage
//! - Configurable size limits
//! - Async-native API
//!
//! ## Example
//!
//! ```rust,no_run
//! use hs_rustlib::spool::{Spool, SpoolConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = SpoolConfig {
//!     path: "/tmp/my-spool".into(),
//!     compress: true,
//!     ..Default::default()
//! };
//!
//! let mut spool = Spool::open(config).await?;
//!
//! // Add items to the queue
//! spool.push(b"first message").await?;
//! spool.push(b"second message").await?;
//!
//! // Process items (FIFO order)
//! while let Some(data) = spool.pop_front().await? {
//!     println!("Processing: {:?}", data);
//! }
//! # Ok(())
//! # }
//! ```

mod config;
mod error;
mod queue;

pub use config::SpoolConfig;
pub use error::SpoolError;
pub use queue::Spool;

/// Result type for spool operations.
pub type Result<T> = std::result::Result<T, SpoolError>;
