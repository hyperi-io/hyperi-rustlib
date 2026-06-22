// Project:   hyperi-rustlib
// File:      src/http_server/mod.rs
// Purpose:   High-performance HTTP server with axum
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! HTTP server built on axum. Compatible with Tonic for gRPC.
//!
//! ## Features
//!
//! - Graceful shutdown
//! - Configurable request timeout, in-flight cap, Tower middleware
//! - Health endpoints (`/healthz`, `/readyz`, plus `/health/*` aliases)
//!
//! In-process TLS and a `/metrics` endpoint are NOT wired here -- see
//! [`HttpServerConfig`]. Terminate TLS at the ingress / mesh; metrics are
//! served by `MetricsManager` on its own listener.
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
    Extension, Router,
    extract::{Path, Query, State},
    response::{IntoResponse, Json, Response},
    routing::{delete, get, post, put},
};

/// Result type for HTTP server operations.
pub type Result<T> = std::result::Result<T, HttpServerError>;
