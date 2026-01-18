// Project:   hs-rustlib
// File:      src/http_server/server.rs
// Purpose:   HTTP server implementation
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! HTTP server implementation using axum.

use crate::http_server::{HttpServerConfig, HttpServerError, Result};
use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Router,
};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::watch;

/// High-performance HTTP server built on axum.
///
/// Provides graceful shutdown, health endpoints, and Tower middleware support.
pub struct HttpServer {
    config: HttpServerConfig,
    ready: Arc<AtomicBool>,
}

impl HttpServer {
    /// Create a new HTTP server with the given configuration.
    #[must_use]
    pub fn new(config: HttpServerConfig) -> Self {
        Self {
            config,
            ready: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Create a new HTTP server bound to the specified address.
    #[must_use]
    pub fn bind(address: impl Into<String>) -> Self {
        Self::new(HttpServerConfig::new(address))
    }

    /// Set the readiness state for the /health/ready endpoint.
    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::SeqCst);
    }

    /// Get the current readiness state.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    /// Get a clone of the readiness flag for use in application state.
    #[must_use]
    pub fn ready_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.ready)
    }

    /// Serve the given router until a shutdown signal is received.
    ///
    /// This method will:
    /// 1. Bind to the configured address
    /// 2. Optionally add health check endpoints
    /// 3. Run until SIGTERM or SIGINT is received
    /// 4. Perform graceful shutdown
    ///
    /// # Errors
    ///
    /// Returns an error if binding fails or the server encounters an error.
    pub async fn serve(self, app: Router) -> Result<()> {
        self.serve_with_shutdown(app, shutdown_signal()).await
    }

    /// Serve with a custom shutdown signal.
    ///
    /// This is useful for testing or when you need custom shutdown logic.
    ///
    /// # Errors
    ///
    /// Returns an error if binding fails or the server encounters an error.
    pub async fn serve_with_shutdown<F>(self, app: Router, shutdown: F) -> Result<()>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let app = self.build_router(app);

        let addr: SocketAddr = self
            .config
            .bind_address
            .parse()
            .map_err(|e| HttpServerError::Bind {
                address: self.config.bind_address.clone(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidInput, e),
            })?;

        let listener = TcpListener::bind(addr).await.map_err(|e| HttpServerError::Bind {
            address: self.config.bind_address.clone(),
            source: e,
        })?;

        #[cfg(feature = "logger")]
        tracing::info!(address = %addr, "HTTP server listening");

        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(HttpServerError::Io)?;

        #[cfg(feature = "logger")]
        tracing::info!("HTTP server shut down gracefully");

        Ok(())
    }

    /// Serve and return a handle for programmatic shutdown.
    ///
    /// Returns a `ShutdownHandle` that can be used to trigger shutdown.
    ///
    /// # Errors
    ///
    /// Returns an error if binding fails.
    pub async fn serve_with_handle(self, app: Router) -> Result<(ShutdownHandle, ServerFuture)> {
        let (tx, rx) = watch::channel(());
        let handle = ShutdownHandle { sender: tx };

        let shutdown = async move {
            let _ = rx.clone().changed().await;
        };

        let app = self.build_router(app);

        let addr: SocketAddr = self
            .config
            .bind_address
            .parse()
            .map_err(|e| HttpServerError::Bind {
                address: self.config.bind_address.clone(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidInput, e),
            })?;

        let listener = TcpListener::bind(addr).await.map_err(|e| HttpServerError::Bind {
            address: self.config.bind_address.clone(),
            source: e,
        })?;

        #[cfg(feature = "logger")]
        tracing::info!(address = %addr, "HTTP server listening");

        let future = ServerFuture {
            inner: Box::pin(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(shutdown)
                    .await
                    .map_err(HttpServerError::Io)
            }),
        };

        Ok((handle, future))
    }

    /// Build the final router with optional health endpoints.
    fn build_router(&self, app: Router) -> Router {
        let mut router = app;

        if self.config.enable_health_endpoints {
            let ready = Arc::clone(&self.ready);
            router = router
                .route("/health/live", get(health_live))
                .route("/health/ready", get(move || health_ready(Arc::clone(&ready))));
        }

        router
    }
}

/// Handle for triggering server shutdown.
#[derive(Clone)]
pub struct ShutdownHandle {
    sender: watch::Sender<()>,
}

impl ShutdownHandle {
    /// Trigger graceful shutdown.
    pub fn shutdown(self) {
        let _ = self.sender.send(());
    }
}

/// Future representing the running server.
pub struct ServerFuture {
    inner: std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>,
}

impl std::future::Future for ServerFuture {
    type Output = Result<()>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

/// Liveness endpoint handler.
async fn health_live() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

/// Readiness endpoint handler.
async fn health_ready(ready: Arc<AtomicBool>) -> impl IntoResponse {
    if ready.load(Ordering::SeqCst) {
        (StatusCode::OK, "OK")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "NOT READY")
    }
}

/// Wait for a shutdown signal (SIGTERM or SIGINT).
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    #[cfg(feature = "logger")]
    tracing::info!("Shutdown signal received, starting graceful shutdown");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_health_live() {
        let config = HttpServerConfig::default();
        let server = HttpServer::new(config);
        let app = server.build_router(Router::new());

        let response = app
            .oneshot(Request::builder().uri("/health/live").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_health_ready_when_ready() {
        let config = HttpServerConfig::default();
        let server = HttpServer::new(config);
        server.set_ready(true);
        let app = server.build_router(Router::new());

        let response = app
            .oneshot(Request::builder().uri("/health/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_health_ready_when_not_ready() {
        let config = HttpServerConfig::default();
        let server = HttpServer::new(config);
        server.set_ready(false);
        let app = server.build_router(Router::new());

        let response = app
            .oneshot(Request::builder().uri("/health/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn test_server_with_handle() {
        // Test the handle API works with an actual server
        let config = HttpServerConfig::new("127.0.0.1:18080");
        let server = HttpServer::new(config);

        let app = Router::new().route("/", get(|| async { "Hello" }));

        // Test the handle API compiles and works
        let (handle, future) = server.serve_with_handle(app).await.unwrap();

        // Shutdown immediately
        handle.shutdown();

        // Wait for server to finish
        future.await.unwrap();
    }

    #[test]
    fn test_ready_flag() {
        let config = HttpServerConfig::default();
        let server = HttpServer::new(config);

        assert!(server.is_ready());
        server.set_ready(false);
        assert!(!server.is_ready());
        server.set_ready(true);
        assert!(server.is_ready());
    }
}
