// Project:   hyperi-rustlib
// File:      src/transport/grpc/mod.rs
// Purpose:   gRPC transport backend
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

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
//! use hyperi_rustlib::transport::{GrpcTransport, GrpcConfig, TransportReceiver};
//!
//! // Server mode (receive from remote senders)
//! let config = GrpcConfig::server("0.0.0.0:6000");
//! let transport = GrpcTransport::new(&config).await?;
//!
//! let records = transport.recv(100).await?.records;
//! // commit is a no-op for gRPC (no persistence)
//! transport.commit(&[]).await?;
//! ```

pub mod batch;
pub mod config;
pub mod proto;
pub mod token;

pub use config::GrpcConfig;
pub use token::GrpcToken;

use super::error::{TransportError, TransportResult};
use super::traits::{RecvBatch, TransportBase, TransportReceiver, TransportSender};
use super::types::{Message, PayloadFormat, SendResult};
use super::work_batch::{Record, WorkBatch};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::{mpsc, oneshot};
use tonic::{Request, Response, Status};

/// gRPC transport for DFE inter-service communication.
///
/// Implements both `TransportSender` and `TransportReceiver`, so it also
/// satisfies the unified `Transport` trait via blanket impl.
pub struct GrpcTransport {
    /// Client for sending (None if server-only mode).
    client: Option<proto::dfe_transport_client::DfeTransportClient<tonic::transport::Channel>>,

    /// Receiver channel (None if client-only mode).
    receiver: Option<tokio::sync::Mutex<mpsc::Receiver<Message<GrpcToken>>>>,

    /// Shutdown signal for the server task. Behind a `Mutex<Option<..>>` so
    /// `close(&self)` (not just `Drop`) can take and fire it.
    shutdown_tx: parking_lot::Mutex<Option<oneshot::Sender<()>>>,

    /// Server background task handle (kept alive, aborted on drop).
    _server_handle: Option<tokio::task::JoinHandle<Result<(), tonic::transport::Error>>>,

    /// Whether the transport is closed.
    closed: AtomicBool,

    /// Shared healthy flag -- read by health registry closure, written by close().
    healthy: Arc<AtomicBool>,

    /// Receive timeout (milliseconds).
    recv_timeout_ms: u64,

    /// Per-RPC send deadline (milliseconds, 0 = none).
    send_timeout_ms: u64,

    /// In-flight send count (for metrics).
    #[cfg(feature = "metrics")]
    inflight: AtomicU64,

    /// Transport-level message filter engine.
    filter_engine: super::filter::TransportFilterEngine,
}

/// Build a tonic `ClientTlsConfig` from the unified TLS fields on `GrpcConfig`.
///
/// tonic owns its TLS stack (like librdkafka), so this maps the unified
/// `TlsTrust` vocabulary onto `ClientTlsConfig`: private-CA PEM (else OS native
/// roots), optional SNI domain override, and optional mTLS client identity.
fn build_grpc_client_tls(
    config: &GrpcConfig,
) -> TransportResult<tonic::transport::ClientTlsConfig> {
    use tonic::transport::{Certificate, ClientTlsConfig, Identity};

    let mut tls = ClientTlsConfig::new();

    if let Some(ref ca) = config.tls_ca_path {
        let pem = std::fs::read(ca)
            .map_err(|e| TransportError::Config(format!("gRPC TLS: cannot read ca {ca}: {e}")))?;
        tls = tls.ca_certificate(Certificate::from_pem(pem));
    } else {
        // No private CA -> trust the OS native roots.
        tls = tls.with_native_roots();
    }

    if let Some(ref domain) = config.tls_domain {
        tls = tls.domain_name(domain.clone());
    }

    // mTLS identity -- both cert and key, or neither.
    match (&config.tls_client_cert_path, &config.tls_client_key_path) {
        (Some(cert), Some(key)) => {
            let cert_pem = std::fs::read(cert).map_err(|e| {
                TransportError::Config(format!("gRPC TLS: cannot read client cert {cert}: {e}"))
            })?;
            let key_pem = std::fs::read(key).map_err(|e| {
                TransportError::Config(format!("gRPC TLS: cannot read client key {key}: {e}"))
            })?;
            tls = tls.identity(Identity::from_pem(cert_pem, key_pem));
        }
        (None, None) => {}
        _ => {
            return Err(TransportError::Config(
                "gRPC TLS: mTLS requires BOTH tls_client_cert_path and tls_client_key_path"
                    .to_string(),
            ));
        }
    }

    Ok(tls)
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
        Self::new_inner(
            config,
            #[cfg(feature = "governor")]
            None,
        )
        .await
    }

    /// Create a gRPC transport bound to a pressure governor (G3, `governor`
    /// feature).
    ///
    /// Identical to [`new`](Self::new) except the receive server consults
    /// `pressure` BEFORE enqueuing each inbound Push / batch record: while
    /// [`UnifiedPressure::should_hold`](crate::governor::UnifiedPressure::should_hold)
    /// holds, the RPC is rejected with `Status::unavailable` (the gRPC analogue
    /// of HTTP 503, matching the existing channel-full backpressure mapping).
    /// Passing `None` is exactly equivalent to [`new`](Self::new).
    ///
    /// # Errors
    ///
    /// Same as [`new`](Self::new).
    #[cfg(feature = "governor")]
    pub async fn with_pressure(
        config: &GrpcConfig,
        pressure: Option<Arc<crate::governor::UnifiedPressure>>,
    ) -> TransportResult<Self> {
        Self::new_inner(config, pressure).await
    }

    async fn new_inner(
        config: &GrpcConfig,
        #[cfg(feature = "governor")] pressure: Option<Arc<crate::governor::UnifiedPressure>>,
    ) -> TransportResult<Self> {
        let mut client = None;
        let mut receiver = None;
        let mut shutdown_tx = None;
        let mut server_handle = None;
        let sequence = Arc::new(AtomicU64::new(0));

        // Set up client (lazy connection -- doesn't fail until first RPC)
        if let Some(endpoint) = &config.endpoint {
            let mut ep = tonic::transport::Channel::from_shared(endpoint.clone())
                .map_err(|e| TransportError::Config(format!("invalid endpoint: {e}")))?;

            // Client TLS. tonic owns its TLS stack, so we map the unified
            // vocabulary onto ClientTlsConfig (private CA, mTLS identity, SNI).
            if config.tls_enabled {
                ep = ep
                    .tls_config(build_grpc_client_tls(config)?)
                    .map_err(|e| TransportError::Config(format!("gRPC TLS config: {e}")))?;
            }

            let channel = ep.connect_lazy();

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
                #[cfg(feature = "governor")]
                pressure: pressure.clone(),
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

            // Bind the listener synchronously BEFORE spawning the serve task.
            // Once `TcpListener::bind` returns the OS socket is listening and
            // queues incoming connections, so `new()` returning is a true
            // readiness signal -- callers (and their tests) can connect
            // immediately with no polling. `serve_with_shutdown(addr, ..)`
            // bound inside the spawned task, which made `new()` return before
            // the socket existed and forced every consumer to poll the port.
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .map_err(|e| TransportError::Config(format!("failed to bind {addr}: {e}")))?;
            let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

            let handle = tokio::spawn(async move {
                router
                    .serve_with_incoming_shutdown(incoming, async {
                        sd_rx.await.ok();
                    })
                    .await
            });

            receiver = Some(tokio::sync::Mutex::new(rx));
            shutdown_tx = Some(sd_tx);
            server_handle = Some(handle);
        } else {
            // No receive server -> nothing to attach the governor to. Consume
            // it so the param stays uniform with no unused-variable warning.
            #[cfg(feature = "governor")]
            let _ = pressure;
        }

        let healthy = Arc::new(AtomicBool::new(true));

        let filter_engine = super::filter::TransportFilterEngine::new(
            &config.filters_in,
            &config.filters_out,
            &crate::transport::filter::TransportFilterTierConfig::from_cascade(),
        )?;

        #[cfg(feature = "health")]
        {
            let h = Arc::clone(&healthy);
            crate::health::HealthRegistry::register("transport:grpc", move || {
                if h.load(Ordering::Relaxed) {
                    crate::health::HealthStatus::Healthy
                } else {
                    crate::health::HealthStatus::Unhealthy
                }
            });
        }

        Ok(Self {
            client,
            receiver,
            shutdown_tx: parking_lot::Mutex::new(shutdown_tx),
            _server_handle: server_handle,
            closed: AtomicBool::new(false),
            healthy,
            recv_timeout_ms: config.recv_timeout_ms,
            send_timeout_ms: config.send_timeout_ms,
            #[cfg(feature = "metrics")]
            inflight: AtomicU64::new(0),
            filter_engine,
        })
    }
}

impl TransportSender for GrpcTransport {
    async fn send(&self, key: &str, payload: bytes::Bytes) -> SendResult {
        if self.closed.load(Ordering::Relaxed) {
            return SendResult::Fatal(TransportError::Closed);
        }

        // Outbound filter check
        if self.filter_engine.has_outbound_filters() {
            match self.filter_engine.apply_outbound(&payload) {
                super::filter::FilterDisposition::Pass => {}
                super::filter::FilterDisposition::Drop => return SendResult::Ok,
                super::filter::FilterDisposition::Dlq => return SendResult::FilteredDlq,
            }
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

        // Inject W3C traceparent into gRPC metadata for distributed tracing
        #[cfg(feature = "transport-trace")]
        if let Some(tp) = super::propagation::current_traceparent() {
            metadata.insert(super::propagation::TRACEPARENT_HEADER.to_string(), tp);
        }

        let mut request = tonic::Request::new(proto::PushRequest {
            // `payload` is already `bytes::Bytes` and the proto field is now
            // `Bytes` too (`.bytes(".")` in build.rs) -- move the handle, no copy.
            payload,
            format: proto::Format::Auto.into(),
            metadata,
        });

        // Bound the RPC so a hung/black-holing server cannot wedge the sender
        // task forever. Sent as the grpc-timeout header; the server aborts and
        // the client surfaces Code::DeadlineExceeded when it elapses.
        if self.send_timeout_ms > 0 {
            request.set_timeout(std::time::Duration::from_millis(self.send_timeout_ms));
        }

        #[cfg(feature = "metrics")]
        let start = std::time::Instant::now();

        #[cfg(feature = "metrics")]
        self.inflight.fetch_add(1, Ordering::Relaxed);

        // tonic clients are cheaply cloneable (shared channel)
        let result = match client.clone().push(request).await {
            Ok(_) => {
                #[cfg(feature = "metrics")]
                metrics::counter!("dfe_transport_sent_total", "transport" => "grpc").increment(1);
                SendResult::Ok
            }
            Err(status) => match status.code() {
                // DeadlineExceeded = our send_timeout_ms fired (slow/hung server).
                // Transient -- treat as backpressure so the caller retries rather
                // than dropping the message.
                tonic::Code::Unavailable
                | tonic::Code::ResourceExhausted
                | tonic::Code::DeadlineExceeded => {
                    #[cfg(feature = "metrics")]
                    metrics::counter!(
                        "dfe_transport_backpressured_total",
                        "transport" => "grpc"
                    )
                    .increment(1);
                    SendResult::Backpressured
                }
                _ => {
                    #[cfg(feature = "metrics")]
                    metrics::counter!(
                        "dfe_transport_send_errors_total",
                        "transport" => "grpc"
                    )
                    .increment(1);
                    SendResult::Fatal(TransportError::Send(status.message().to_string()))
                }
            },
        };

        #[cfg(feature = "metrics")]
        {
            self.inflight.fetch_sub(1, Ordering::Relaxed);
            metrics::gauge!("dfe_transport_inflight", "transport" => "grpc")
                .set(self.inflight.load(Ordering::Relaxed) as f64);
            metrics::histogram!(
                "dfe_transport_send_duration_seconds",
                "transport" => "grpc"
            )
            .record(start.elapsed().as_secs_f64());
        }

        result
    }

    /// Send a whole batch of records in ONE `RouteBatch` RPC (Task 0.6).
    ///
    /// The native batch override of [`TransportSender::send_batch`]: serde-less
    /// rustlib<->rustlib transfer. The records map to a proto
    /// [`Batch`](proto::Batch) via [`batch::records_to_proto`] -- payloads travel
    /// as OPAQUE `bytes` and the JSON / MsgPack codec is NEVER invoked in
    /// transit. The whole batch goes in a single call (batch-at-a-time, NOT
    /// record-by-record streaming), so unlike the trait's per-record default
    /// there is no partial-send window: the block is accepted or not as a unit.
    ///
    /// Commit tokens and inline-DLQ entries are NOT sent -- they are the
    /// SENDER's local concern. Pass the records (e.g. `&workbatch.records`); the
    /// caller fires its commit tokens locally after this returns `Ok`.
    ///
    /// # Errors / result
    ///
    /// Returns a [`SendResult`]. `Backpressured` maps the same transient gRPC
    /// codes as [`send`](TransportSender::send) so the caller retries the whole
    /// block rather than dropping it (at-least-once).
    async fn send_batch(&self, records: &[Record]) -> SendResult {
        if self.closed.load(Ordering::Relaxed) {
            return SendResult::Fatal(TransportError::Closed);
        }

        let Some(client) = &self.client else {
            return SendResult::Fatal(TransportError::Config(
                "no endpoint configured for sending".into(),
            ));
        };

        // Map records -> proto Batch. Payloads are MOVED (Bytes handle), opaque.
        let proto_batch = batch::records_to_proto(records.to_vec());

        let mut request = tonic::Request::new(proto_batch);

        // Inject W3C traceparent into gRPC metadata for distributed tracing.
        #[cfg(feature = "transport-trace")]
        if let Some(tp) = super::propagation::current_traceparent()
            && let Ok(val) = tp.parse()
        {
            request
                .metadata_mut()
                .insert(super::propagation::TRACEPARENT_HEADER, val);
        }

        if self.send_timeout_ms > 0 {
            request.set_timeout(std::time::Duration::from_millis(self.send_timeout_ms));
        }

        #[cfg(feature = "metrics")]
        let start = std::time::Instant::now();
        #[cfg(feature = "metrics")]
        self.inflight.fetch_add(1, Ordering::Relaxed);

        let result = match client.clone().route_batch(request).await {
            Ok(_) => {
                #[cfg(feature = "metrics")]
                metrics::counter!(
                    "dfe_transport_sent_total",
                    "transport" => "grpc",
                    "path" => "batch"
                )
                .increment(records.len() as u64);
                SendResult::Ok
            }
            Err(status) => match status.code() {
                tonic::Code::Unavailable
                | tonic::Code::ResourceExhausted
                | tonic::Code::DeadlineExceeded => {
                    #[cfg(feature = "metrics")]
                    metrics::counter!(
                        "dfe_transport_backpressured_total",
                        "transport" => "grpc"
                    )
                    .increment(1);
                    SendResult::Backpressured
                }
                _ => {
                    #[cfg(feature = "metrics")]
                    metrics::counter!(
                        "dfe_transport_send_errors_total",
                        "transport" => "grpc"
                    )
                    .increment(1);
                    SendResult::Fatal(TransportError::Send(status.message().to_string()))
                }
            },
        };

        #[cfg(feature = "metrics")]
        {
            self.inflight.fetch_sub(1, Ordering::Relaxed);
            metrics::histogram!(
                "dfe_transport_send_duration_seconds",
                "transport" => "grpc"
            )
            .record(start.elapsed().as_secs_f64());
        }

        result
    }
}

impl TransportBase for GrpcTransport {
    async fn close(&self) -> TransportResult<()> {
        self.closed.store(true, Ordering::Relaxed);
        self.healthy.store(false, Ordering::Relaxed);

        // Actually stop the server: fire the shutdown oneshot so
        // serve_with_incoming_shutdown completes and the listener is freed.
        // Idempotent -- a second close() (or Drop) finds None.
        if let Some(tx) = self.shutdown_tx.lock().take() {
            let _ = tx.send(());
        }
        Ok(())
    }

    fn is_healthy(&self) -> bool {
        let healthy = self.healthy.load(Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::gauge!("dfe_transport_healthy", "transport" => "grpc").set(if healthy {
            1.0
        } else {
            0.0
        });
        healthy
    }

    fn name(&self) -> &'static str {
        "grpc"
    }
}

impl TransportReceiver for GrpcTransport {
    type Token = GrpcToken;

    async fn recv(&self, max: usize) -> TransportResult<WorkBatch<Self::Token>> {
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

        // Apply inbound filters via the shared partition helper; DLQ entries
        // are returned in the RecvBatch for the caller to route onward.
        let batch =
            self.filter_engine
                .partition_batch(messages, |m| m.payload.as_ref(), |m| m.key.clone());
        let messages = batch.messages;
        let dlq_entries = batch.dlq_entries;

        Ok(RecvBatch {
            messages,
            dlq_entries,
        }
        .into())
    }

    async fn commit(&self, _tokens: &[Self::Token]) -> TransportResult<()> {
        // gRPC has no broker-side persistence -- commit is a no-op.
        // Acknowledgement is implicit in the Push RPC response.
        Ok(())
    }
}

impl Drop for GrpcTransport {
    fn drop(&mut self) {
        // Fire the shutdown signal if close() didn't already (idempotent).
        if let Some(tx) = self.shutdown_tx.lock().take() {
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
    /// Optional pressure governor (G3, `governor` feature). `None` by default
    /// -> the handlers never consult it and behaviour is byte-identical. When
    /// `Some`, an inbound Push / batch record is rejected with
    /// `Status::unavailable` while [`UnifiedPressure::should_hold`] holds --
    /// pressure-driven shedding ON TOP of the existing channel-full rejection.
    #[cfg(feature = "governor")]
    pressure: Option<Arc<crate::governor::UnifiedPressure>>,
}

#[tonic::async_trait]
impl proto::dfe_transport_server::DfeTransport for DfeTransportServiceImpl {
    async fn push(
        &self,
        request: Request<proto::PushRequest>,
    ) -> Result<Response<proto::PushResponse>, Status> {
        // G3 pressure-driven shedding (governor feature, opt-in). BEFORE doing
        // any work, if a governor is wired and it says hold, reject with
        // `unavailable` -- the gRPC analogue of HTTP 503, mirroring the
        // channel-full rejection below. Default `None` -> skipped, unchanged.
        #[cfg(feature = "governor")]
        if let Some(pressure) = &self.pressure
            && pressure.should_hold()
        {
            #[cfg(feature = "metrics")]
            metrics::counter!(
                "dfe_transport_backpressured_total",
                "transport" => "grpc",
                "reason" => "pressure"
            )
            .increment(1);
            return Err(Status::unavailable("under pressure -- inbound held"));
        }

        let req = request.into_inner();
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);

        // Extract W3C traceparent from incoming gRPC metadata for distributed tracing
        #[cfg(feature = "transport-trace")]
        if let Some(tp) = req.metadata.get(super::propagation::TRACEPARENT_HEADER)
            && super::propagation::is_valid_traceparent(tp)
        {
            tracing::Span::current().record("traceparent", tp.as_str());
        }

        let format = PayloadFormat::detect(&req.payload);
        let key = req.metadata.get("topic").map(|s| Arc::from(s.as_str()));

        // `req.payload` is already prost `Bytes` (`.bytes(".")` in build.rs) --
        // the decode was zero-copy, so this is a move, not a copy.
        let msg = Message {
            key,
            payload: req.payload,
            token: GrpcToken::new(seq),
            timestamp_ms: None,
            format,
        };

        match self.sender.try_send(msg) {
            Ok(()) => {
                #[cfg(feature = "metrics")]
                {
                    metrics::counter!("dfe_transport_sent_total", "transport" => "grpc")
                        .increment(1);
                    metrics::gauge!("dfe_transport_queue_size", "transport" => "grpc").set(
                        self.sender
                            .max_capacity()
                            .saturating_sub(self.sender.capacity()) as f64,
                    );
                }
                Ok(Response::new(proto::PushResponse { accepted: 1 }))
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                #[cfg(feature = "metrics")]
                metrics::counter!(
                    "dfe_transport_backpressured_total",
                    "transport" => "grpc"
                )
                .increment(1);
                Err(Status::resource_exhausted("receiver buffer full"))
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                #[cfg(feature = "metrics")]
                metrics::counter!(
                    "dfe_transport_refused_total",
                    "transport" => "grpc"
                )
                .increment(1);
                Err(Status::unavailable("receiver closed"))
            }
        }
    }

    async fn route_batch(
        &self,
        request: Request<proto::Batch>,
    ) -> Result<Response<proto::BatchAck>, Status> {
        // G3 pressure-driven shedding (governor feature, opt-in): reject the
        // whole batch with `unavailable` while pressure holds. Default `None`
        // -> skipped, byte-identical.
        #[cfg(feature = "governor")]
        if let Some(pressure) = &self.pressure
            && pressure.should_hold()
        {
            #[cfg(feature = "metrics")]
            metrics::counter!(
                "dfe_transport_backpressured_total",
                "transport" => "grpc",
                "reason" => "pressure"
            )
            .increment(1);
            return Err(Status::unavailable("under pressure -- inbound held"));
        }

        // Extract W3C traceparent from incoming gRPC metadata for distributed
        // tracing, BEFORE consuming the request body.
        #[cfg(feature = "transport-trace")]
        if let Some(tp) = request
            .metadata()
            .get(super::propagation::TRACEPARENT_HEADER)
            .and_then(|v| v.to_str().ok())
            && super::propagation::is_valid_traceparent(tp)
        {
            tracing::Span::current().record("traceparent", tp);
        }

        let proto_batch = request.into_inner();

        // Decode the proto Batch back into rustlib Records (payloads are
        // zero-copy `Bytes`; the codec is NOT invoked here). Each record fans
        // into the SAME mpsc channel the single-message Push path uses, so the
        // existing recv() path delivers them unchanged.
        let records = batch::proto_batch_to_records(proto_batch);
        let accepted = records.len() as u64;

        for record in records {
            let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
            let format = record.metadata.format;
            // A record carrying Auto means the sender did not pin a format
            // (e.g. it framed but did not classify). Detect from the bytes so
            // the receiver still gets a concrete hint -- this inspects the lead
            // byte only, it does NOT parse/decode the payload.
            let format = if format == PayloadFormat::Auto {
                PayloadFormat::detect(&record.payload)
            } else {
                format
            };

            let msg = Message {
                key: record.key,
                payload: record.payload,
                token: GrpcToken::new(seq),
                timestamp_ms: record.metadata.timestamp_ms,
                format,
            };

            match self.sender.try_send(msg) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    #[cfg(feature = "metrics")]
                    metrics::counter!(
                        "dfe_transport_backpressured_total",
                        "transport" => "grpc"
                    )
                    .increment(1);
                    return Err(Status::resource_exhausted("receiver buffer full"));
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    #[cfg(feature = "metrics")]
                    metrics::counter!(
                        "dfe_transport_refused_total",
                        "transport" => "grpc"
                    )
                    .increment(1);
                    return Err(Status::unavailable("receiver closed"));
                }
            }
        }

        #[cfg(feature = "metrics")]
        metrics::counter!(
            "dfe_transport_sent_total",
            "transport" => "grpc",
            "path" => "batch"
        )
        .increment(accepted);

        Ok(Response::new(proto::BatchAck { accepted }))
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
        assert_eq!(config.send_timeout_ms, 30_000);
        assert_eq!(config.max_message_size, 16 * 1024 * 1024);
        assert!(!config.compression);
        assert!(!config.tls_enabled);
        assert!(config.tls_ca_path.is_none());
    }

    #[test]
    fn grpc_client_tls_builds_with_private_ca_and_rejects_half_mtls() {
        use std::io::Write;
        let cert = rcgen::generate_simple_self_signed(vec!["grpc.test".to_string()]).unwrap();
        let mut ca = tempfile::NamedTempFile::new().unwrap();
        ca.write_all(cert.cert.pem().as_bytes()).unwrap();
        ca.flush().unwrap();

        // Private CA + SNI -> builds.
        let cfg = GrpcConfig {
            endpoint: Some("https://peer:6000".to_string()),
            tls_enabled: true,
            tls_ca_path: Some(ca.path().to_string_lossy().into_owned()),
            tls_domain: Some("grpc.test".to_string()),
            ..Default::default()
        };
        assert!(build_grpc_client_tls(&cfg).is_ok());

        // Half-configured mTLS (cert without key) -> error.
        let cfg = GrpcConfig {
            tls_enabled: true,
            tls_client_cert_path: Some(ca.path().to_string_lossy().into_owned()),
            tls_client_key_path: None,
            ..Default::default()
        };
        assert!(build_grpc_client_tls(&cfg).is_err());
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

    /// G3: with a pressure governor pinned HIGH, the gRPC Push handler rejects
    /// with `Status::unavailable` (the gRPC analogue of 503). The default `new`
    /// (no governor) accepts as before.
    #[cfg(feature = "governor")]
    #[tokio::test]
    async fn grpc_pressure_high_rejects_unavailable() {
        use crate::governor::{Hysteresis, MemoryPressureSource, PressureSource, UnifiedPressure};
        use crate::memory::{MemoryGuard, MemoryGuardConfig};

        let guard = Arc::new(MemoryGuard::new(MemoryGuardConfig {
            limit_bytes: 1000,
            pressure_threshold: 0.80,
            ..Default::default()
        }));
        guard.add_bytes(950); // 95%
        let pressure = Arc::new(UnifiedPressure::new(
            vec![Arc::new(MemoryPressureSource::new(Arc::clone(&guard))) as Arc<dyn PressureSource>],
            Hysteresis::new(0.80, 0.65).expect("valid band"),
        ));
        assert!(pressure.should_hold(), "pinned-high governor must hold");

        // Server bound to the governor.
        let server_cfg = GrpcConfig::server("127.0.0.1:16077");
        let server = GrpcTransport::with_pressure(&server_cfg, Some(Arc::clone(&pressure)))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Client pushes -> rejected as backpressure (maps to Backpressured).
        let client_cfg = GrpcConfig::client("http://127.0.0.1:16077");
        let client = GrpcTransport::new(&client_cfg).await.unwrap();
        let result = client
            .send("events", bytes::Bytes::from_static(b"{\"x\":1}"))
            .await;
        assert!(
            matches!(result, SendResult::Backpressured),
            "push under pressure must surface as backpressure, got {result:?}"
        );

        client.close().await.unwrap();
        server.close().await.unwrap();
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
        let result = transport
            .send("test", bytes::Bytes::from_static(b"payload"))
            .await;
        assert!(result.is_fatal());

        // Close
        transport.close().await.unwrap();
        assert!(!transport.is_healthy());
    }
}
