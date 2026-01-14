// Project:   hs-rustlib
// File:      src/clickhouse/mod.rs
// Purpose:   ClickHouse client abstraction with Arrow protocol support
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! ClickHouse client abstraction with native Arrow protocol support.
//!
//! This module provides a high-level client for ClickHouse that uses the native
//! Arrow protocol for efficient columnar data transfer. It wraps the
//! `clickhouse-arrow` crate with a simplified API suitable for common use cases.
//!
//! ## Features
//!
//! - Native Arrow protocol for efficient inserts and queries
//! - Schema introspection from ClickHouse tables
//! - Runtime type parsing (ClickHouse as SSOT)
//! - Connection pooling via `clickhouse-arrow`
//!
//! ## Example
//!
//! ```rust,no_run
//! use hs_rustlib::clickhouse::{ArrowClickHouseClient, ClickHouseConfig};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = ClickHouseConfig {
//!     hosts: vec!["localhost:9000".to_string()],
//!     database: "default".to_string(),
//!     username: "default".to_string(),
//!     password: String::new(),
//!     ..Default::default()
//! };
//!
//! let client = ArrowClickHouseClient::new(&config).await?;
//!
//! // Check connection
//! client.health_check().await?;
//!
//! // Query data
//! let batches = client.select("SELECT * FROM events LIMIT 10").await?;
//! # Ok(())
//! # }
//! ```

mod client;
mod config;
mod error;
mod types;

pub use client::{ArrowClickHouseClient, SharedArrowClient};
pub use config::ClickHouseConfig;
pub use error::ClickHouseError;
pub use types::{
    default_value_for_category, is_null_string, ColumnInfo, ParsedType, TableSchema, NULL_STRINGS,
};

/// Result type for ClickHouse operations.
pub type Result<T> = std::result::Result<T, ClickHouseError>;
