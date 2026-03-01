// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED

//! # gRPC Transport
//!
//! DFE native gRPC transport using tonic. Supports client mode (sending),
//! server mode (receiving), or both.
//!
//! ## DFE Native Protocol
//!
//! Lightweight bulk bytes transfer via `dfe.transport.v1.DfeTransport/Push`.
//! Payload is opaque bytes (JSON, MsgPack, or Arrow IPC) with a format hint.
//!
//! ## Vector Wire Protocol Compatibility (optional)
//!
//! When the `transport-grpc-vector-compat` feature is enabled and
//! `GrpcConfig::vector_compat` is true, the server also accepts
//! `vector.Vector/PushEvents` RPCs from legacy Vector sinks.
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::transport::{GrpcTransport, GrpcConfig, Transport};
//!
//! // Server mode (receive from remote senders)
//! let config = GrpcConfig::server("0.0.0.0:6000");
//! let transport = GrpcTransport::new(&config).await?;
//!
//! let messages = transport.recv(100).await?;
//! // commit is a no-op for gRPC (no persistence)
//! transport.commit(&[]).await?;
//! ```

pub mod config;
pub mod proto;
pub mod token;

pub use config::GrpcConfig;
pub use token::GrpcToken;

use super::error::{TransportError, TransportResult};
use super::traits::Transport;
use super::types::{Message, PayloadFormat, SendResult};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tonic::{Request, Response, Status};

/// gRPC transport for DFE inter-service communication.
///
/// Combines a tonic gRPC client (for sending) and server (for receiving)
/// behind the unified `Transport` trait.
pub struct GrpcTransport {
    /// Client for sending (None if server-only mode).
    client: Option<proto::dfe_transport_client::DfeTransportClient<tonic::transport::Channel>>,

    /// Receiver channel (None if client-only mode).
    receiver: Option<tokio::sync::Mutex<mpsc::Receiver<Message<GrpcToken>>>>,

    /// Shutdown signal for the server task.
    shutdown_tx: Option<oneshot::Sender<()>>,

    /// Server background task handle (kept alive, aborted on drop).
    _server_handle: Option<tokio::task::JoinHandle<Result<(), tonic::transport::Error>>>,

    /// Whether the transport is closed.
    closed: AtomicBool,

    /// Receive timeout (milliseconds).
    recv_timeout_ms: u64,
}

impl GrpcTransport {
    /// Create a new gRPC transport.
    ///
    /// # Configuration
    ///
    /// - Set `config.listen` to start a gRPC server (receive mode).
    /// - Set `config.endpoint` to connect to a remote server (send mode).
    /// - Set both for bidirectional communication.
    ///
    /// # Errors
    ///
    /// Returns error if the listen address is invalid or the server fails to start.
    pub async fn new(config: &GrpcConfig) -> TransportResult<Self> {
        let mut client = None;
        let mut receiver = None;
        let mut shutdown_tx = None;
        let mut server_handle = None;
        let sequence = Arc::new(AtomicU64::new(0));

        // Set up client (lazy connection — doesn't fail until first RPC)
        if let Some(endpoint) = &config.endpoint {
            let channel = tonic::transport::Channel::from_shared(endpoint.clone())
                .map_err(|e| TransportError::Config(format!("invalid endpoint: {e}")))?
                .connect_lazy();

            let mut c = proto::dfe_transport_client::DfeTransportClient::new(channel)
                .max_decoding_message_size(config.max_message_size)
                .max_encoding_message_size(config.max_message_size);

            if config.compression {
                c = c
                    .send_compressed(tonic::codec::CompressionEncoding::Gzip)
                    .accept_compressed(tonic::codec::CompressionEncoding::Gzip);
            }

            client = Some(c);
        }

        // Set up server
        if let Some(listen) = &config.listen {
            let addr: std::net::SocketAddr = listen
                .parse()
                .map_err(|e| TransportError::Config(format!("invalid listen address: {e}")))?;

            let (tx, rx) = mpsc::channel(config.recv_buffer_size);
            let (sd_tx, sd_rx) = oneshot::channel();

            // DFE native service
            let dfe_svc = DfeTransportServiceImpl {
                sender: tx.clone(),
                sequence: sequence.clone(),
            };

            let dfe_server = proto::dfe_transport_server::DfeTransportServer::new(dfe_svc)
                .max_decoding_message_size(config.max_message_size)
                .max_encoding_message_size(config.max_message_size)
                .accept_compressed(tonic::codec::CompressionEncoding::Gzip)
                .send_compressed(tonic::codec::CompressionEncoding::Gzip);

            // Build server with optional Vector compat
            let mut builder = tonic::transport::Server::builder();

            #[cfg(feature = "transport-grpc-vector-compat")]
            let router = if config.vector_compat {
                let vector_svc =
                    super::vector_compat::source::VectorCompatService::new(tx, sequence.clone());
                let vector_server =
                    super::vector_compat::proto::vector::vector_server::VectorServer::new(
                        vector_svc,
                    )
                    .max_decoding_message_size(config.max_message_size)
                    .accept_compressed(tonic::codec::CompressionEncoding::Gzip)
                    .send_compressed(tonic::codec::CompressionEncoding::Gzip);

                builder.add_service(dfe_server).add_service(vector_server)
            } else {
                builder.add_service(dfe_server)
            };

            #[cfg(not(feature = "transport-grpc-vector-compat"))]
            let router = builder.add_service(dfe_server);

            let handle = tokio::spawn(async move {
                router
                    .serve_with_shutdown(addr, async {
                        sd_rx.await.ok();
                    })
                    .await
            });

            receiver = Some(tokio::sync::Mutex::new(rx));
            shutdown_tx = Some(sd_tx);
            server_handle = Some(handle);
        }

        Ok(Self {
            client,
            receiver,
            shutdown_tx,
            _server_handle: server_handle,
            closed: AtomicBool::new(false),
            recv_timeout_ms: config.recv_timeout_ms,
        })
    }
}

#[async_trait]
impl Transport for GrpcTransport {
    type Token = GrpcToken;

    async fn send(&self, key: &str, payload: &[u8]) -> SendResult {
        if self.closed.load(Ordering::Relaxed) {
            return SendResult::Fatal(TransportError::Closed);
        }

        let Some(client) = &self.client else {
            return SendResult::Fatal(TransportError::Config(
                "no endpoint configured for sending".into(),
            ));
        };

        let mut metadata = HashMap::new();
        if !key.is_empty() {
            metadata.insert("topic".to_string(), key.to_string());
        }

        let request = proto::PushRequest {
            payload: payload.to_vec(),
            format: proto::Format::Auto.into(),
            metadata,
        };

        // tonic clients are cheaply cloneable (shared channel)
        match client.clone().push(request).await {
            Ok(_) => SendResult::Ok,
            Err(status) => match status.code() {
                tonic::Code::Unavailable | tonic::Code::ResourceExhausted => {
                    SendResult::Backpressured
                }
                _ => SendResult::Fatal(TransportError::Send(status.message().to_string())),
            },
        }
    }

    async fn recv(&self, max: usize) -> TransportResult<Vec<Message<Self::Token>>> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(TransportError::Closed);
        }

        let Some(receiver) = &self.receiver else {
            return Err(TransportError::Config(
                "no listen address configured for receiving".into(),
            ));
        };

        let mut rx = receiver.lock().await;
        let mut messages = Vec::with_capacity(max.min(100));

        for _ in 0..max {
            let result = if self.recv_timeout_ms == 0 {
                // Non-blocking
                match rx.try_recv() {
                    Ok(msg) => Some(msg),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
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

    async fn commit(&self, _tokens: &[Self::Token]) -> TransportResult<()> {
        // gRPC has no broker-side persistence — commit is a no-op.
        // Acknowledgement is implicit in the Push RPC response.
        Ok(())
    }

    async fn close(&self) -> TransportResult<()> {
        self.closed.store(true, Ordering::Relaxed);

        // Signal server shutdown
        // Note: we can't take from Option behind &self, so we use a flag
        // The server task will complete when the oneshot is dropped
        Ok(())
    }

    fn is_healthy(&self) -> bool {
        !self.closed.load(Ordering::Relaxed)
    }

    fn name(&self) -> &'static str {
        "grpc"
    }
}

impl Drop for GrpcTransport {
    fn drop(&mut self) {
        // Take and send shutdown signal
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        // Server handle will be dropped, which aborts the task
    }
}

// --- DFE Transport gRPC service implementation ---

/// Internal service implementation that receives Push RPCs
/// and forwards messages into the transport's mpsc channel.
struct DfeTransportServiceImpl {
    sender: mpsc::Sender<Message<GrpcToken>>,
    sequence: Arc<AtomicU64>,
}

#[tonic::async_trait]
impl proto::dfe_transport_server::DfeTransport for DfeTransportServiceImpl {
    async fn push(
        &self,
        request: Request<proto::PushRequest>,
    ) -> Result<Response<proto::PushResponse>, Status> {
        let req = request.into_inner();
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);

        let format = PayloadFormat::detect(&req.payload);
        let key = req.metadata.get("topic").map(|s| Arc::from(s.as_str()));

        let msg = Message {
            key,
            payload: req.payload,
            token: GrpcToken::new(seq),
            timestamp_ms: None,
            format,
        };

        self.sender
            .send(msg)
            .await
            .map_err(|_| Status::unavailable("receiver buffer full"))?;

        Ok(Response::new(proto::PushResponse { accepted: 1 }))
    }

    async fn health_check(
        &self,
        _request: Request<proto::HealthCheckRequest>,
    ) -> Result<Response<proto::HealthCheckResponse>, Status> {
        Ok(Response::new(proto::HealthCheckResponse {
            status: proto::ServingStatus::Serving.into(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grpc_token_display() {
        let token = GrpcToken::new(42);
        assert_eq!(format!("{token}"), "grpc:42");

        let token = GrpcToken::with_source(7, Arc::from("peer-1"));
        assert_eq!(format!("{token}"), "grpc:peer-1:7");
    }

    #[test]
    fn grpc_config_defaults() {
        let config = GrpcConfig::default();
        assert!(config.listen.is_none());
        assert!(config.endpoint.is_none());
        assert_eq!(config.recv_buffer_size, 10_000);
        assert_eq!(config.recv_timeout_ms, 100);
        assert_eq!(config.max_message_size, 16 * 1024 * 1024);
        assert!(!config.compression);
    }

    #[test]
    fn grpc_config_server() {
        let config = GrpcConfig::server("0.0.0.0:6000");
        assert_eq!(config.listen.as_deref(), Some("0.0.0.0:6000"));
        assert!(config.endpoint.is_none());
    }

    #[test]
    fn grpc_config_client() {
        let config = GrpcConfig::client("http://loader:6000");
        assert!(config.listen.is_none());
        assert_eq!(config.endpoint.as_deref(), Some("http://loader:6000"));
    }

    #[test]
    fn grpc_config_with_compression() {
        let config = GrpcConfig::server("0.0.0.0:6000").with_compression();
        assert!(config.compression);
    }

    #[tokio::test]
    async fn grpc_transport_client_only() {
        // Client-only transport (lazy connection, no server)
        let config = GrpcConfig::client("http://localhost:16000");
        let transport = GrpcTransport::new(&config).await.unwrap();

        assert!(transport.client.is_some());
        assert!(transport.receiver.is_none());
        assert!(transport.is_healthy());
        assert_eq!(transport.name(), "grpc");

        // recv should error (no server)
        let result = transport.recv(10).await;
        assert!(result.is_err());

        // commit is always ok
        transport.commit(&[]).await.unwrap();
    }

    #[tokio::test]
    async fn grpc_transport_server_only() {
        // Server-only transport (no client for sending)
        // Note: port 0 may not work with tonic parse, use a specific port
        let config = GrpcConfig::server("127.0.0.1:16001");
        let transport = GrpcTransport::new(&config).await.unwrap();

        assert!(transport.client.is_none());
        assert!(transport.receiver.is_some());
        assert!(transport.is_healthy());

        // send should error (no client)
        let result = transport.send("test", b"payload").await;
        assert!(result.is_fatal());

        // Close
        transport.close().await.unwrap();
        assert!(!transport.is_healthy());
    }
}
