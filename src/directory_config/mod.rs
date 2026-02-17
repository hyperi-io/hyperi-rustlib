// Project:   hyperi-rustlib
// File:      src/directory_config/mod.rs
// Purpose:   Directory-based YAML config store module root
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # Directory Config Store
//!
//! A directory-based configuration store backed by YAML files. Each YAML file
//! in the configured directory (or subdirectories) is treated as a "table" and
//! cached in memory with automatic background polling refresh.
//!
//! ## Features
//!
//! - **Read API:** `get()`, `get_key()`, `get_as()`, `list_tables()`
//! - **Write API:** `set()`, `delete_key()` with advisory file locking
//! - **Subdirectory support:** `loaders/dfe-loader` maps to `loaders/dfe-loader.yaml`
//! - **Background refresh:** Polling-based (safe for S3/FUSE mounts)
//! - **Change notifications:** Subscribe via `on_change()`
//! - **Git integration:** Optional commit-on-write (feature `directory-config-git`)
//!
//! ## Example
//!
//! ```rust,no_run
//! use hyperi_rustlib::directory_config::{DirectoryConfigStore, DirectoryConfigStoreConfig};
//! use std::path::PathBuf;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = DirectoryConfigStoreConfig {
//!     directory: PathBuf::from("/etc/dfe/config"),
//!     ..Default::default()
//! };
//!
//! let mut store = DirectoryConfigStore::new(config).await?;
//! store.start().await?;
//!
//! // Read
//! let tables = store.list_tables().await;
//! let value = store.get("dfe-loader").await?;
//! let host = store.get_key("dfe-loader", "kafka.brokers").await?;
//!
//! // Write (if not read-only)
//! store.set("dfe-loader", "kafka.brokers", "broker:9092".into(), None).await?;
//!
//! store.stop().await?;
//! # Ok(())
//! # }
//! ```

pub mod error;
mod refresh;
pub mod store;
pub mod types;

#[cfg(feature = "directory-config-git")]
pub mod git;

// Re-exports for convenience
pub use error::{DirectoryConfigError, DirectoryConfigResult};
pub use store::DirectoryConfigStore;
pub use types::{ChangeEvent, ChangeOperation, DirectoryConfigStoreConfig, WriteMode, WriteResult};
