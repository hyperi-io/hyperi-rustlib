// Project:   hyperi-rustlib
// File:      src/transport/http.rs
// Purpose:   HTTP/HTTPS transport (send via POST, receive via embedded server)
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # HTTP Transport
//!
//! HTTP/HTTPS transport for webhook delivery and REST ingest.
//!
//! ## Send
//!
//! POSTs payload bytes to `{endpoint}/{key}` using reqwest.
//!
//! ## Receive (requires `http-server` feature)
//!
//! Starts an embedded axum HTTP server that accepts POST requests on a
//! configurable path. Incoming payloads are queued into a bounded
//! `tokio::sync::mpsc` channel. `recv()` drains from this channel.
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::transport::http::{HttpTransport, HttpTransportConfig};
//!
//! // Send-only
//! let config = HttpTransportConfig {
//!     endpoint: Some("http://loader:8080/ingest".into()),
//!     ..Default::default()
//! };
//! let transport = HttpTransport::new(&config).await?;
//! transport.send("events", b"{\"msg\":\"hello\"}").await;
//! ```

use super::error::{TransportError, TransportResult};
use super::traits::{CommitToken, TransportBase, TransportReceiver, TransportSender};
#[cfg(feature = "http-server")]
use super::types::PayloadFormat;
use super::types::{Message, SendResult};
use serde::{Deserialize, Serialize};
#[cfg(feature = "http-server")]
use std::sync::Arc;
#[cfg(feature = "http-server")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};

/// Commit token for HTTP transport.
///
/// HTTP is fire-and-forget from the receiver's perspective, so commit
/// is a no-op. The token provides sequence tracking and optional
/// client address for observability.
#[derive(Debug, Clone)]
pub struct HttpToken {
    /// Local sequence number (monotonically increasing per transport instance).
    pub seq: u64,

    /// Source client address (if available from the HTTP request).
    pub source_addr: Option<String>,
}

impl HttpToken {
    /// Create a new token with sequence number.
    #[must_use]
    pub fn new(seq: u64) -> Self {
        Self {
            seq,
            source_addr: None,
        }
    }

    /// Create a new token with sequence number and source address.
    #[must_use]
    pub fn with_source(seq: u64, addr: String) -> Self {
        Self {
            seq,
            source_addr: Some(addr),
        }
    }
}

impl CommitToken for HttpToken {}

impl std::fmt::Display for HttpToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.source_addr {
            Some(addr) => write!(f, "http:{}:{}", addr, self.seq),
            None => write!(f, "http:{}", self.seq),
        }
    }
}

fn default_recv_path() -> String {
    "/ingest".to_string()
}

fn default_buffer_size() -> usize {
    10_000
}

fn default_recv_timeout_ms() -> u64 {
    100
}

/// Configuration for HTTP transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpTransportConfig {
    /// Endpoint URL for sending (e.g., "http://loader:8080/ingest"). None = send disabled.
    #[serde(default)]
    pub endpoint: Option<String>,

    /// Listen address for receiving (e.g., "0.0.0.0:8080"). None = receive disabled.
    /// Requires the `http-server` feature.
    #[serde(default)]
    pub listen: Option<String>,

    /// Path to accept POSTs on for receive mode. Default: "/ingest".
    #[serde(default = "default_recv_path")]
    pub recv_path: String,

    /// Receive buffer size (bounded channel capacity). Default: 10000.
    #[serde(default = "default_buffer_size")]
    pub recv_buffer_size: usize,

    /// Receive timeout in milliseconds. Default: 100.
    #[serde(default = "default_recv_timeout_ms")]
    pub recv_timeout_ms: u64,
}

impl Default for HttpTransportConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            listen: None,
            recv_path: default_recv_path(),
            recv_buffer_size: default_buffer_size(),
            recv_timeout_ms: default_recv_timeout_ms(),
        }
    }
}

impl HttpTransportConfig {
    /// Create a send-only config pointing at the given endpoint URL.
    #[must_use]
    pub fn sender(endpoint: &str) -> Self {
        Self {
            endpoint: Some(endpoint.to_string()),
            ..Default::default()
        }
    }

    /// Create a receive-only config listening on the given address.
    #[must_use]
    pub fn receiver(listen: &str) -> Self {
        Self {
            listen: Some(listen.to_string()),
            ..Default::default()
        }
    }
}

/// HTTP/HTTPS transport.
///
/// Supports send (POST to endpoint) and receive (embedded axum server).
/// The receive side requires the `http-server` feature for axum.
pub struct HttpTransport {
    /// reqwest client for sending (always available when transport-http is enabled).
    client: reqwest::Client,

    /// Base URL for sending (None = send disabled).
    endpoint: Option<String>,

    /// Receiver channel populated by the embedded HTTP server.
    /// Only available when `http-server` feature is enabled AND `listen` is configured.
    #[cfg(feature = "http-server")]
    receiver: Option<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<Message<HttpToken>>>>,

    /// Shutdown signal for the server task.
    #[cfg(feature = "http-server")]
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,

    /// Server background task handle.
    #[cfg(feature = "http-server")]
    _server_handle: Option<tokio::task::JoinHandle<()>>,

    /// Whether the transport is closed.
    closed: AtomicBool,

    /// Receive timeout in milliseconds (used by receive side).
    #[cfg(feature = "http-server")]
    recv_timeout_ms: u64,
}

impl HttpTransport {
    /// Create a new HTTP transport.
    ///
    /// - Set `config.endpoint` to enable sending (POST to URL).
    /// - Set `config.listen` to enable receiving (embedded HTTP server, requires `http-server` feature).
    ///
    /// # Errors
    ///
    /// Returns error if the listen address is invalid or the server fails to bind.
    pub async fn new(config: &HttpTransportConfig) -> TransportResult<Self> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| TransportError::Config(format!("failed to create HTTP client: {e}")))?;

        #[cfg(feature = "http-server")]
        let (receiver, shutdown_tx, server_handle) = if let Some(listen) = &config.listen {
            let addr: std::net::SocketAddr = listen
                .parse()
                .map_err(|e| TransportError::Config(format!("invalid listen address: {e}")))?;

            let (tx, rx) = tokio::sync::mpsc::channel(config.recv_buffer_size);
            let (sd_tx, sd_rx) = tokio::sync::oneshot::channel::<()>();

            let sequence = Arc::new(AtomicU64::new(0));
            let recv_path = config.recv_path.clone();

            let app = build_receiver_router(tx, sequence, &recv_path);

            let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
                TransportError::Connection(format!("failed to bind to {addr}: {e}"))
            })?;

            let handle = tokio::spawn(async move {
                axum::serve(
                    listener,
                    app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                )
                .with_graceful_shutdown(async {
                    sd_rx.await.ok();
                })
                .await
                .ok();
            });

            (Some(tokio::sync::Mutex::new(rx)), Some(sd_tx), Some(handle))
        } else {
            (None, None, None)
        };

        Ok(Self {
            client,
            endpoint: config.endpoint.clone(),
            #[cfg(feature = "http-server")]
            receiver,
            #[cfg(feature = "http-server")]
            shutdown_tx,
            #[cfg(feature = "http-server")]
            _server_handle: server_handle,
            closed: AtomicBool::new(false),
            #[cfg(feature = "http-server")]
            recv_timeout_ms: config.recv_timeout_ms,
        })
    }
}

/// Build the axum router for the receive side.
#[cfg(feature = "http-server")]
fn build_receiver_router(
    sender: tokio::sync::mpsc::Sender<Message<HttpToken>>,
    sequence: Arc<AtomicU64>,
    recv_path: &str,
) -> axum::Router {
    use axum::routing::post;

    let state = ReceiverState { sender, sequence };

    axum::Router::new()
        .route(recv_path, post(ingest_handler))
        .with_state(state)
}

/// Shared state for the receive handler.
#[cfg(feature = "http-server")]
#[derive(Clone)]
struct ReceiverState {
    sender: tokio::sync::mpsc::Sender<Message<HttpToken>>,
    sequence: Arc<AtomicU64>,
}

/// POST handler that accepts raw bytes and queues them into the mpsc channel.
#[cfg(feature = "http-server")]
async fn ingest_handler(
    axum::extract::State(state): axum::extract::State<ReceiverState>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    body: axum::body::Bytes,
) -> axum::http::StatusCode {
    if body.is_empty() {
        return axum::http::StatusCode::BAD_REQUEST;
    }

    let seq = state.sequence.fetch_add(1, Ordering::Relaxed);
    let format = PayloadFormat::detect(&body);
    let timestamp_ms = chrono::Utc::now().timestamp_millis();

    let msg = Message {
        key: None,
        payload: body.to_vec(),
        token: HttpToken::with_source(seq, addr.to_string()),
        timestamp_ms: Some(timestamp_ms),
        format,
    };

    match state.sender.try_send(msg) {
        Ok(()) => {
            #[cfg(feature = "metrics")]
            metrics::counter!("dfe_transport_sent_total", "transport" => "http").increment(1);
            axum::http::StatusCode::OK
        }
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            #[cfg(feature = "metrics")]
            metrics::counter!("dfe_transport_backpressured_total", "transport" => "http")
                .increment(1);
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            #[cfg(feature = "metrics")]
            metrics::counter!("dfe_transport_refused_total", "transport" => "http").increment(1);
            axum::http::StatusCode::GONE
        }
    }
}

impl TransportBase for HttpTransport {
    async fn close(&self) -> TransportResult<()> {
        self.closed.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn is_healthy(&self) -> bool {
        !self.closed.load(Ordering::Relaxed)
    }

    fn name(&self) -> &'static str {
        "http"
    }
}

impl TransportSender for HttpTransport {
    async fn send(&self, key: &str, payload: &[u8]) -> SendResult {
        if self.closed.load(Ordering::Relaxed) {
            return SendResult::Fatal(TransportError::Closed);
        }

        let Some(base_url) = &self.endpoint else {
            return SendResult::Fatal(TransportError::Config(
                "no endpoint configured for sending".into(),
            ));
        };

        // Build URL: {base_url}/{key} if key is non-empty, otherwise just {base_url}
        let url = if key.is_empty() {
            base_url.clone()
        } else {
            let base = base_url.trim_end_matches('/');
            let suffix = key.trim_start_matches('/');
            format!("{base}/{suffix}")
        };

        #[cfg(feature = "metrics")]
        let start = std::time::Instant::now();

        let result = match self
            .client
            .post(&url)
            .header("content-type", "application/octet-stream")
            .body(payload.to_vec())
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                #[cfg(feature = "metrics")]
                metrics::counter!("dfe_transport_sent_total", "transport" => "http").increment(1);
                SendResult::Ok
            }
            Ok(resp)
                if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
                    || resp.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE =>
            {
                #[cfg(feature = "metrics")]
                metrics::counter!("dfe_transport_backpressured_total", "transport" => "http")
                    .increment(1);
                SendResult::Backpressured
            }
            Ok(resp) => {
                #[cfg(feature = "metrics")]
                metrics::counter!("dfe_transport_send_errors_total", "transport" => "http")
                    .increment(1);
                SendResult::Fatal(TransportError::Send(format!(
                    "HTTP {} from {}",
                    resp.status(),
                    url
                )))
            }
            Err(e) => {
                #[cfg(feature = "metrics")]
                metrics::counter!("dfe_transport_send_errors_total", "transport" => "http")
                    .increment(1);
                SendResult::Fatal(TransportError::Send(format!("HTTP request failed: {e}")))
            }
        };

        #[cfg(feature = "metrics")]
        metrics::histogram!("dfe_transport_send_duration_seconds", "transport" => "http")
            .record(start.elapsed().as_secs_f64());

        result
    }
}

impl TransportReceiver for HttpTransport {
    type Token = HttpToken;

    async fn recv(&self, max: usize) -> TransportResult<Vec<Message<Self::Token>>> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(TransportError::Closed);
        }

        #[cfg(feature = "http-server")]
        {
            let Some(receiver) = &self.receiver else {
                return Err(TransportError::Config(
                    "no listen address configured for receiving".into(),
                ));
            };

            let mut rx = receiver.lock().await;
            let mut messages = Vec::with_capacity(max.min(100));

            for _ in 0..max {
                let result = if self.recv_timeout_ms == 0 {
                    match rx.try_recv() {
                        Ok(msg) => Some(msg),
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                            return Err(TransportError::Closed);
                        }
                    }
                } else if messages.is_empty() {
                    // First message: wait with timeout
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(self.recv_timeout_ms),
                        rx.recv(),
                    )
                    .await
                    {
                        Ok(Some(msg)) => Some(msg),
                        Ok(None) => return Err(TransportError::Closed),
                        Err(_) => break, // Timeout
                    }
                } else {
                    // Subsequent: non-blocking drain
                    match rx.try_recv() {
                        Ok(msg) => Some(msg),
                        Err(_) => break,
                    }
                };

                if let Some(msg) = result {
                    messages.push(msg);
                }
            }

            Ok(messages)
        }

        #[cfg(not(feature = "http-server"))]
        {
            let _ = max;
            Err(TransportError::Config(
                "HTTP receive requires the 'http-server' feature".into(),
            ))
        }
    }

    async fn commit(&self, _tokens: &[Self::Token]) -> TransportResult<()> {
        // HTTP is fire-and-forget — commit is a no-op.
        Ok(())
    }
}

impl Drop for HttpTransport {
    fn drop(&mut self) {
        #[cfg(feature = "http-server")]
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_token_display() {
        let token = HttpToken::new(42);
        assert_eq!(format!("{token}"), "http:42");
    }

    #[test]
    fn http_token_display_with_source() {
        let token = HttpToken::with_source(7, "192.168.1.1:54321".to_string());
        assert_eq!(format!("{token}"), "http:192.168.1.1:54321:7");
    }

    #[test]
    fn config_defaults() {
        let config = HttpTransportConfig::default();
        assert!(config.endpoint.is_none());
        assert!(config.listen.is_none());
        assert_eq!(config.recv_path, "/ingest");
        assert_eq!(config.recv_buffer_size, 10_000);
        assert_eq!(config.recv_timeout_ms, 100);
    }

    #[test]
    fn config_sender_helper() {
        let config = HttpTransportConfig::sender("http://localhost:8080/ingest");
        assert_eq!(
            config.endpoint.as_deref(),
            Some("http://localhost:8080/ingest")
        );
        assert!(config.listen.is_none());
    }

    #[test]
    fn config_receiver_helper() {
        let config = HttpTransportConfig::receiver("0.0.0.0:8080");
        assert!(config.endpoint.is_none());
        assert_eq!(config.listen.as_deref(), Some("0.0.0.0:8080"));
    }

    #[tokio::test]
    async fn send_only_transport() {
        // Send-only config (no endpoint = send disabled, but transport creates fine)
        let config = HttpTransportConfig::default();
        let transport = HttpTransport::new(&config).await.unwrap();

        assert!(transport.is_healthy());
        assert_eq!(transport.name(), "http");

        // Send without endpoint should fail
        let result = transport.send("test", b"payload").await;
        assert!(result.is_fatal());

        // Commit is always ok
        transport.commit(&[]).await.unwrap();
    }

    #[tokio::test]
    async fn close_prevents_send() {
        let config = HttpTransportConfig::sender("http://localhost:19999/test");
        let transport = HttpTransport::new(&config).await.unwrap();

        transport.close().await.unwrap();
        assert!(!transport.is_healthy());

        let result = transport.send("test", b"data").await;
        assert!(result.is_fatal());
    }

    #[tokio::test]
    async fn close_prevents_recv() {
        let config = HttpTransportConfig::default();
        let transport = HttpTransport::new(&config).await.unwrap();

        transport.close().await.unwrap();
        let result = transport.recv(1).await;
        assert!(result.is_err());
    }

    /// Full send + receive round-trip test.
    /// Requires both `transport-http` and `http-server` features.
    #[cfg(feature = "http-server")]
    #[tokio::test]
    async fn send_and_receive_roundtrip() {
        // Start receiver on a random available port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener); // Free the port for the transport to bind

        let recv_config = HttpTransportConfig {
            listen: Some(addr.to_string()),
            recv_path: "/ingest".to_string(),
            recv_buffer_size: 100,
            recv_timeout_ms: 1000,
            ..Default::default()
        };
        let receiver = HttpTransport::new(&recv_config).await.unwrap();

        // Give the server a moment to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send a message via a separate sender transport
        let send_config =
            HttpTransportConfig::sender(&format!("http://127.0.0.1:{}/ingest", addr.port()));
        let sender = HttpTransport::new(&send_config).await.unwrap();

        let result = sender.send("", b"{\"msg\":\"hello\"}").await;
        assert!(result.is_ok(), "send failed: {result:?}");

        // Receive it
        let messages = receiver.recv(10).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].payload, b"{\"msg\":\"hello\"}");
        assert!(messages[0].token.source_addr.is_some());

        // Cleanup
        sender.close().await.unwrap();
        receiver.close().await.unwrap();
    }

    /// Test that the receiver rejects empty bodies.
    #[cfg(feature = "http-server")]
    #[tokio::test]
    async fn receive_rejects_empty_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let recv_config = HttpTransportConfig {
            listen: Some(addr.to_string()),
            recv_timeout_ms: 200,
            ..Default::default()
        };
        let receiver = HttpTransport::new(&recv_config).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Send empty body
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://127.0.0.1:{}/ingest", addr.port()))
            .body(Vec::<u8>::new())
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);

        // recv should timeout with no messages
        let messages = receiver.recv(10).await.unwrap();
        assert!(messages.is_empty());

        receiver.close().await.unwrap();
    }

    /// Test recv without listen returns config error.
    #[cfg(feature = "http-server")]
    #[tokio::test]
    async fn recv_without_listen_returns_error() {
        let config = HttpTransportConfig::sender("http://localhost:9999");
        let transport = HttpTransport::new(&config).await.unwrap();

        let result = transport.recv(10).await;
        assert!(result.is_err());
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = HttpTransportConfig {
            endpoint: Some("http://example.com/ingest".into()),
            listen: Some("0.0.0.0:8080".into()),
            recv_path: "/custom".into(),
            recv_buffer_size: 5000,
            recv_timeout_ms: 250,
        };

        let json = serde_json::to_string(&config).unwrap();
        let parsed: HttpTransportConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.endpoint, config.endpoint);
        assert_eq!(parsed.listen, config.listen);
        assert_eq!(parsed.recv_path, config.recv_path);
        assert_eq!(parsed.recv_buffer_size, config.recv_buffer_size);
        assert_eq!(parsed.recv_timeout_ms, config.recv_timeout_ms);
    }
}
