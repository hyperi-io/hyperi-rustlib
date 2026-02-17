// Project:   hyperi-rustlib
// File:      src/http_server/mod.rs
// Purpose:   High-performance HTTP server with axum
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! High-performance HTTP server built on axum.
//!
//! This module provides a configurable HTTP server suitable for building
//! APIs, health endpoints, and metrics servers. It uses axum for ergonomics
//! and is compatible with Tonic for gRPC.
//!
//! ## Features
//!
//! - Graceful shutdown support
//! - Configurable timeouts (request, keep-alive)
//! - Optional TLS support
//! - Health check endpoints (`/health/live`, `/health/ready`)
//! - Metrics endpoint (`/metrics`)
//! - Tower middleware integration
//!
//! ## Example
//!
//! ```rust,no_run
//! use hyperi_rustlib::http_server::{HttpServer, HttpServerConfig};
//! use axum::{Router, routing::get};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = HttpServerConfig {
//!     bind_address: "0.0.0.0:8080".to_string(),
//!     ..Default::default()
//! };
//!
//! let app = Router::new()
//!     .route("/", get(|| async { "Hello, World!" }));
//!
//! let server = HttpServer::new(config);
//! server.serve(app).await?;
//! # Ok(())
//! # }
//! ```

mod config;
mod error;
mod server;

pub use config::HttpServerConfig;
pub use error::HttpServerError;
pub use server::HttpServer;

// Re-export axum types users commonly need
pub use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Json, Response},
    routing::{delete, get, post, put},
    Extension, Router,
};

/// Result type for HTTP server operations.
pub type Result<T> = std::result::Result<T, HttpServerError>;
