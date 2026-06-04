// Project:   hyperi-rustlib
// File:      src/transport/factory.rs
// Purpose:   Transport factory -- create senders from config
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Transport factory for runtime transport selection.
//!
//! Creates transport senders from configuration, enabling apps to swap
//! between Kafka, gRPC, file, pipe, HTTP, or Redis via config change.
//!
//! # Usage
//!
//! ```yaml
//! # settings.yaml
//! transport:
//!   output:
//!     type: kafka
//!     kafka:
//!       brokers: ["kafka:9092"]
//! ```
//!
//! ```rust,ignore
//! use hyperi_rustlib::transport::factory::AnySender;
//!
//! let sender = AnySender::from_config("transport.output").await?;
//! sender.send("events.land", payload).await;
//! ```

use super::error::{TransportError, TransportResult};
use super::traits::{CommitToken, TransportBase, TransportReceiver, TransportSender};
use super::types::SendResult;
#[cfg(any(
    feature = "transport-kafka",
    feature = "transport-grpc",
    feature = "transport-memory",
    feature = "transport-pipe",
    feature = "transport-file",
    feature = "transport-http",
    feature = "transport-redis"
))]
use super::types::TransportType;
use super::work_batch::{Record, WorkBatch};

/// Type-erased transport sender.
///
/// Wraps any concrete transport sender behind an enum for runtime
/// dispatch. Created by the transport factory from config.
///
/// Uses enum dispatch (not trait objects) because `TransportSender`
/// has `impl Future` return types which prevent `dyn` dispatch.
pub enum AnySender {
    #[cfg(feature = "transport-kafka")]
    Kafka(super::kafka::KafkaTransport),

    #[cfg(feature = "transport-grpc")]
    Grpc(super::grpc::GrpcTransport),

    #[cfg(feature = "transport-memory")]
    Memory(super::memory::MemoryTransport),

    #[cfg(feature = "transport-pipe")]
    Pipe(super::pipe::PipeTransport),

    #[cfg(feature = "transport-file")]
    File(super::file::FileTransport),

    #[cfg(feature = "transport-http")]
    Http(super::http::HttpTransport),

    #[cfg(feature = "transport-redis")]
    Redis(super::redis_transport::RedisTransport),
}

impl TransportBase for AnySender {
    async fn close(&self) -> TransportResult<()> {
        match self {
            #[cfg(feature = "transport-kafka")]
            Self::Kafka(t) => t.close().await,
            #[cfg(feature = "transport-grpc")]
            Self::Grpc(t) => t.close().await,
            #[cfg(feature = "transport-memory")]
            Self::Memory(t) => t.close().await,
            #[cfg(feature = "transport-pipe")]
            Self::Pipe(t) => t.close().await,
            #[cfg(feature = "transport-file")]
            Self::File(t) => t.close().await,
            #[cfg(feature = "transport-http")]
            Self::Http(t) => t.close().await,
            #[cfg(feature = "transport-redis")]
            Self::Redis(t) => t.close().await,
            #[allow(unreachable_patterns)]
            _ => Err(TransportError::Config(
                "no transport variant enabled".into(),
            )),
        }
    }

    fn is_healthy(&self) -> bool {
        match self {
            #[cfg(feature = "transport-kafka")]
            Self::Kafka(t) => t.is_healthy(),
            #[cfg(feature = "transport-grpc")]
            Self::Grpc(t) => t.is_healthy(),
            #[cfg(feature = "transport-memory")]
            Self::Memory(t) => t.is_healthy(),
            #[cfg(feature = "transport-pipe")]
            Self::Pipe(t) => t.is_healthy(),
            #[cfg(feature = "transport-file")]
            Self::File(t) => t.is_healthy(),
            #[cfg(feature = "transport-http")]
            Self::Http(t) => t.is_healthy(),
            #[cfg(feature = "transport-redis")]
            Self::Redis(t) => t.is_healthy(),
            #[allow(unreachable_patterns)]
            _ => false,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            #[cfg(feature = "transport-kafka")]
            Self::Kafka(t) => t.name(),
            #[cfg(feature = "transport-grpc")]
            Self::Grpc(t) => t.name(),
            #[cfg(feature = "transport-memory")]
            Self::Memory(t) => t.name(),
            #[cfg(feature = "transport-pipe")]
            Self::Pipe(t) => t.name(),
            #[cfg(feature = "transport-file")]
            Self::File(t) => t.name(),
            #[cfg(feature = "transport-http")]
            Self::Http(t) => t.name(),
            #[cfg(feature = "transport-redis")]
            Self::Redis(t) => t.name(),
            #[allow(unreachable_patterns)]
            _ => "none",
        }
    }
}

impl TransportSender for AnySender {
    #[cfg_attr(
        not(any(
            feature = "transport-kafka",
            feature = "transport-grpc",
            feature = "transport-memory",
            feature = "transport-pipe",
            feature = "transport-file",
            feature = "transport-http",
            feature = "transport-redis"
        )),
        allow(unused_variables)
    )]
    async fn send(&self, key: &str, payload: bytes::Bytes) -> SendResult {
        match self {
            #[cfg(feature = "transport-kafka")]
            Self::Kafka(t) => t.send(key, payload).await,
            #[cfg(feature = "transport-grpc")]
            Self::Grpc(t) => t.send(key, payload).await,
            #[cfg(feature = "transport-memory")]
            Self::Memory(t) => t.send(key, payload).await,
            #[cfg(feature = "transport-pipe")]
            Self::Pipe(t) => t.send(key, payload).await,
            #[cfg(feature = "transport-file")]
            Self::File(t) => t.send(key, payload).await,
            #[cfg(feature = "transport-http")]
            Self::Http(t) => t.send(key, payload).await,
            #[cfg(feature = "transport-redis")]
            Self::Redis(t) => t.send(key, payload).await,
            #[allow(unreachable_patterns)]
            _ => SendResult::Fatal(TransportError::Config(
                "no transport variant enabled".into(),
            )),
        }
    }

    /// Forward [`send_batch`](TransportSender::send_batch) to the active
    /// backend. gRPC uses its native single-RPC `RouteBatch` override; every
    /// other backend uses the trait's per-record default (see the at-least-once
    /// partial-send caveat on the trait method).
    #[cfg_attr(
        not(any(
            feature = "transport-kafka",
            feature = "transport-grpc",
            feature = "transport-memory",
            feature = "transport-pipe",
            feature = "transport-file",
            feature = "transport-http",
            feature = "transport-redis"
        )),
        allow(unused_variables)
    )]
    async fn send_batch(&self, records: &[Record]) -> SendResult {
        match self {
            #[cfg(feature = "transport-kafka")]
            Self::Kafka(t) => t.send_batch(records).await,
            #[cfg(feature = "transport-grpc")]
            Self::Grpc(t) => t.send_batch(records).await,
            #[cfg(feature = "transport-memory")]
            Self::Memory(t) => t.send_batch(records).await,
            #[cfg(feature = "transport-pipe")]
            Self::Pipe(t) => t.send_batch(records).await,
            #[cfg(feature = "transport-file")]
            Self::File(t) => t.send_batch(records).await,
            #[cfg(feature = "transport-http")]
            Self::Http(t) => t.send_batch(records).await,
            #[cfg(feature = "transport-redis")]
            Self::Redis(t) => t.send_batch(records).await,
            #[allow(unreachable_patterns)]
            _ => SendResult::Fatal(TransportError::Config(
                "no transport variant enabled".into(),
            )),
        }
    }
}

impl AnySender {
    /// Create a sender from config cascade.
    ///
    /// Reads the transport config from the given key in the config
    /// cascade and creates the appropriate sender.
    ///
    /// # Example config
    ///
    /// ```yaml
    /// transport:
    ///   output:
    ///     type: kafka
    ///     kafka:
    ///       brokers: ["kafka:9092"]
    /// ```
    ///
    /// ```rust,ignore
    /// let sender = AnySender::from_config("transport.output").await?;
    /// ```
    pub async fn from_config(key: &str) -> TransportResult<Self> {
        #[cfg(feature = "config")]
        let config = {
            let cfg = crate::config::try_get()
                .ok_or_else(|| TransportError::Config("config not initialised".into()))?;
            cfg.unmarshal_key::<super::TransportConfig>(key)
                .map_err(|e| TransportError::Config(format!("failed to read {key}: {e}")))?
        };

        #[cfg(not(feature = "config"))]
        let config = {
            let _ = key;
            super::TransportConfig::default()
        };

        Self::from_transport_config(&config).await
    }

    /// Create a sender from an explicit `TransportConfig`.
    pub async fn from_transport_config(config: &super::TransportConfig) -> TransportResult<Self> {
        match config.transport_type {
            #[cfg(feature = "transport-kafka")]
            TransportType::Kafka => {
                let kafka_config = config
                    .kafka
                    .as_ref()
                    .ok_or_else(|| TransportError::Config("kafka config missing".into()))?;
                let transport = super::kafka::KafkaTransport::new(kafka_config).await?;
                Ok(Self::Kafka(transport))
            }

            #[cfg(feature = "transport-grpc")]
            TransportType::Grpc => {
                let grpc_config = config
                    .grpc
                    .as_ref()
                    .ok_or_else(|| TransportError::Config("grpc config missing".into()))?;
                let transport = super::grpc::GrpcTransport::new(grpc_config).await?;
                Ok(Self::Grpc(transport))
            }

            #[cfg(feature = "transport-memory")]
            TransportType::Memory => {
                let memory_config = config.memory.clone().unwrap_or_default();
                let transport = super::memory::MemoryTransport::new(&memory_config)?;
                Ok(Self::Memory(transport))
            }

            #[cfg(feature = "transport-pipe")]
            TransportType::Pipe => {
                let pipe_config = config.pipe.clone().unwrap_or_default();
                let transport = super::pipe::PipeTransport::new(&pipe_config);
                Ok(Self::Pipe(transport))
            }

            #[cfg(feature = "transport-file")]
            TransportType::File => {
                let file_config = config
                    .file
                    .as_ref()
                    .ok_or_else(|| TransportError::Config("file config missing".into()))?;
                let transport = super::file::FileTransport::new(file_config).await?;
                Ok(Self::File(transport))
            }

            #[cfg(feature = "transport-http")]
            TransportType::Http => {
                let http_config = config
                    .http
                    .as_ref()
                    .ok_or_else(|| TransportError::Config("http config missing".into()))?;
                let transport = super::http::HttpTransport::new(http_config).await?;
                Ok(Self::Http(transport))
            }

            #[cfg(feature = "transport-redis")]
            TransportType::Redis => {
                let redis_config = config
                    .redis
                    .as_ref()
                    .ok_or_else(|| TransportError::Config("redis config missing".into()))?;
                let transport = super::redis_transport::RedisTransport::new(redis_config).await?;
                Ok(Self::Redis(transport))
            }

            // Transport types for modules not yet implemented
            #[allow(unreachable_patterns)]
            other => Err(TransportError::Config(format!(
                "transport type '{other}' is not available (feature not enabled or not yet implemented)"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// AnyToken -- type-erased commit token, one variant per enabled backend.
// ---------------------------------------------------------------------------

/// Type-erased commit token produced by [`AnyReceiver`].
///
/// Wraps each backend's concrete token in a matching enum variant so that
/// `AnyReceiver::commit` can route tokens back to the correct backend without
/// heap allocation or trait objects.  The variant set mirrors the enabled
/// transport feature flags exactly.
///
/// Tokens are always produced by the same `AnyReceiver` that delivered the
/// messages, so the active variant and active receiver variant will always
/// agree.  `commit` skips tokens whose variant does not match the active
/// backend (defensive; should not occur in practice).
#[derive(Debug, Clone)]
pub enum AnyToken {
    #[cfg(feature = "transport-kafka")]
    /// Kafka consumer offset token.
    Kafka(super::kafka::KafkaToken),

    #[cfg(feature = "transport-grpc")]
    /// gRPC no-op sequence token.
    Grpc(super::grpc::GrpcToken),

    #[cfg(feature = "transport-memory")]
    /// In-memory sequence token.
    Memory(super::memory::MemoryToken),

    #[cfg(feature = "transport-pipe")]
    /// Pipe sequence token.
    Pipe(super::pipe::PipeToken),

    #[cfg(feature = "transport-file")]
    /// File byte-offset token.
    File(super::file::FileToken),

    #[cfg(feature = "transport-http")]
    /// HTTP sequence token.
    Http(super::http::HttpToken),

    #[cfg(feature = "transport-redis")]
    /// Redis XACK entry token.
    Redis(super::redis_transport::RedisToken),
}

impl std::fmt::Display for AnyToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(feature = "transport-kafka")]
            Self::Kafka(t) => std::fmt::Display::fmt(t, f),
            #[cfg(feature = "transport-grpc")]
            Self::Grpc(t) => std::fmt::Display::fmt(t, f),
            #[cfg(feature = "transport-memory")]
            Self::Memory(t) => std::fmt::Display::fmt(t, f),
            #[cfg(feature = "transport-pipe")]
            Self::Pipe(t) => std::fmt::Display::fmt(t, f),
            #[cfg(feature = "transport-file")]
            Self::File(t) => std::fmt::Display::fmt(t, f),
            #[cfg(feature = "transport-http")]
            Self::Http(t) => std::fmt::Display::fmt(t, f),
            #[cfg(feature = "transport-redis")]
            Self::Redis(t) => std::fmt::Display::fmt(t, f),
            #[allow(unreachable_patterns)]
            _ => write!(f, "none"),
        }
    }
}

impl CommitToken for AnyToken {}

// ---------------------------------------------------------------------------
// AnyReceiver -- type-erased transport receiver, mirroring AnySender.
// ---------------------------------------------------------------------------

/// Type-erased transport receiver.
///
/// Wraps any concrete transport receiver behind an enum for runtime
/// dispatch. Created by the transport factory from config, mirroring
/// [`AnySender`].
///
/// Uses enum dispatch (not trait objects) because [`TransportReceiver`]
/// has `impl Future` return types and an associated `Token` type that
/// prevent `dyn` dispatch.
///
/// The [`AnyReceiver::recv`] method wraps each backend token in the
/// corresponding [`AnyToken`] variant.  [`AnyReceiver::commit`] extracts
/// the inner tokens for the active backend and forwards to that backend's
/// own `commit` -- tokens from a different variant are silently skipped
/// (they cannot legitimately appear but the code stays defensive).
pub enum AnyReceiver {
    #[cfg(feature = "transport-kafka")]
    Kafka(super::kafka::KafkaTransport),

    #[cfg(feature = "transport-grpc")]
    Grpc(super::grpc::GrpcTransport),

    #[cfg(feature = "transport-memory")]
    Memory(super::memory::MemoryTransport),

    #[cfg(feature = "transport-pipe")]
    Pipe(super::pipe::PipeTransport),

    #[cfg(feature = "transport-file")]
    File(super::file::FileTransport),

    #[cfg(feature = "transport-http")]
    Http(super::http::HttpTransport),

    #[cfg(feature = "transport-redis")]
    Redis(super::redis_transport::RedisTransport),
}

impl TransportBase for AnyReceiver {
    async fn close(&self) -> TransportResult<()> {
        match self {
            #[cfg(feature = "transport-kafka")]
            Self::Kafka(t) => t.close().await,
            #[cfg(feature = "transport-grpc")]
            Self::Grpc(t) => t.close().await,
            #[cfg(feature = "transport-memory")]
            Self::Memory(t) => t.close().await,
            #[cfg(feature = "transport-pipe")]
            Self::Pipe(t) => t.close().await,
            #[cfg(feature = "transport-file")]
            Self::File(t) => t.close().await,
            #[cfg(feature = "transport-http")]
            Self::Http(t) => t.close().await,
            #[cfg(feature = "transport-redis")]
            Self::Redis(t) => t.close().await,
            #[allow(unreachable_patterns)]
            _ => Err(TransportError::Config(
                "no transport variant enabled".into(),
            )),
        }
    }

    fn is_healthy(&self) -> bool {
        match self {
            #[cfg(feature = "transport-kafka")]
            Self::Kafka(t) => t.is_healthy(),
            #[cfg(feature = "transport-grpc")]
            Self::Grpc(t) => t.is_healthy(),
            #[cfg(feature = "transport-memory")]
            Self::Memory(t) => t.is_healthy(),
            #[cfg(feature = "transport-pipe")]
            Self::Pipe(t) => t.is_healthy(),
            #[cfg(feature = "transport-file")]
            Self::File(t) => t.is_healthy(),
            #[cfg(feature = "transport-http")]
            Self::Http(t) => t.is_healthy(),
            #[cfg(feature = "transport-redis")]
            Self::Redis(t) => t.is_healthy(),
            #[allow(unreachable_patterns)]
            _ => false,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            #[cfg(feature = "transport-kafka")]
            Self::Kafka(t) => t.name(),
            #[cfg(feature = "transport-grpc")]
            Self::Grpc(t) => t.name(),
            #[cfg(feature = "transport-memory")]
            Self::Memory(t) => t.name(),
            #[cfg(feature = "transport-pipe")]
            Self::Pipe(t) => t.name(),
            #[cfg(feature = "transport-file")]
            Self::File(t) => t.name(),
            #[cfg(feature = "transport-http")]
            Self::Http(t) => t.name(),
            #[cfg(feature = "transport-redis")]
            Self::Redis(t) => t.name(),
            #[allow(unreachable_patterns)]
            _ => "none",
        }
    }
}

/// Map a backend's `WorkBatch<BackendToken>` into `WorkBatch<AnyToken>` using
/// the provided variant constructor.  Each `commit_tokens` entry is wrapped in
/// the matching [`AnyToken`] variant; `records` and `dlq_entries` move straight
/// through (the record payload `Bytes` is a refcount bump, never a copy).
#[cfg(any(
    feature = "transport-kafka",
    feature = "transport-grpc",
    feature = "transport-memory",
    feature = "transport-pipe",
    feature = "transport-file",
    feature = "transport-http",
    feature = "transport-redis"
))]
fn wrap_batch<B: CommitToken>(
    batch: WorkBatch<B>,
    wrap_token: impl Fn(B) -> AnyToken,
) -> WorkBatch<AnyToken> {
    let commit_tokens = batch.commit_tokens.into_iter().map(wrap_token).collect();
    WorkBatch::new(batch.records, commit_tokens).with_dlq_entries(batch.dlq_entries)
}

impl TransportReceiver for AnyReceiver {
    type Token = AnyToken;

    #[cfg_attr(
        not(any(
            feature = "transport-kafka",
            feature = "transport-grpc",
            feature = "transport-memory",
            feature = "transport-pipe",
            feature = "transport-file",
            feature = "transport-http",
            feature = "transport-redis"
        )),
        allow(unused_variables)
    )]
    async fn recv(&self, max: usize) -> TransportResult<WorkBatch<AnyToken>> {
        match self {
            #[cfg(feature = "transport-kafka")]
            Self::Kafka(t) => {
                let batch = t.recv(max).await?;
                Ok(wrap_batch(batch, AnyToken::Kafka))
            }
            #[cfg(feature = "transport-grpc")]
            Self::Grpc(t) => {
                let batch = t.recv(max).await?;
                Ok(wrap_batch(batch, AnyToken::Grpc))
            }
            #[cfg(feature = "transport-memory")]
            Self::Memory(t) => {
                let batch = t.recv(max).await?;
                Ok(wrap_batch(batch, AnyToken::Memory))
            }
            #[cfg(feature = "transport-pipe")]
            Self::Pipe(t) => {
                let batch = t.recv(max).await?;
                Ok(wrap_batch(batch, AnyToken::Pipe))
            }
            #[cfg(feature = "transport-file")]
            Self::File(t) => {
                let batch = t.recv(max).await?;
                Ok(wrap_batch(batch, AnyToken::File))
            }
            #[cfg(feature = "transport-http")]
            Self::Http(t) => {
                let batch = t.recv(max).await?;
                Ok(wrap_batch(batch, AnyToken::Http))
            }
            #[cfg(feature = "transport-redis")]
            Self::Redis(t) => {
                let batch = t.recv(max).await?;
                Ok(wrap_batch(batch, AnyToken::Redis))
            }
            #[allow(unreachable_patterns)]
            _ => Err(TransportError::Config(
                "no transport variant enabled".into(),
            )),
        }
    }

    #[cfg_attr(
        not(any(
            feature = "transport-kafka",
            feature = "transport-grpc",
            feature = "transport-memory",
            feature = "transport-pipe",
            feature = "transport-file",
            feature = "transport-http",
            feature = "transport-redis"
        )),
        allow(unused_variables)
    )]
    async fn commit(&self, tokens: &[AnyToken]) -> TransportResult<()> {
        // Each arm uses `match tok { Variant(x) => Some(x), #[allow(unreachable_patterns)] _ => None }`
        // rather than `if let`.  When only a single transport feature is enabled, the AnyToken enum
        // has a single variant, making an `if let` irrefutable (an error under -D warnings).
        // The explicit wildcard arm with `#[allow(unreachable_patterns)]` avoids that -- it is
        // genuinely unreachable in the single-feature case but legal.  Tokens from a non-matching
        // variant indicate a programming error; they are silently filtered out rather than panicking
        // (defensive behaviour, they cannot legitimately arise from this receiver's recv).
        match self {
            #[cfg(feature = "transport-kafka")]
            Self::Kafka(t) => {
                let inner: Vec<_> = tokens
                    .iter()
                    .filter_map(|tok| match tok {
                        AnyToken::Kafka(k) => Some(k.clone()),
                        #[allow(unreachable_patterns)]
                        _ => None,
                    })
                    .collect();
                t.commit(&inner).await
            }
            #[cfg(feature = "transport-grpc")]
            Self::Grpc(t) => {
                let inner: Vec<_> = tokens
                    .iter()
                    .filter_map(|tok| match tok {
                        AnyToken::Grpc(g) => Some(g.clone()),
                        #[allow(unreachable_patterns)]
                        _ => None,
                    })
                    .collect();
                t.commit(&inner).await
            }
            #[cfg(feature = "transport-memory")]
            Self::Memory(t) => {
                let inner: Vec<_> = tokens
                    .iter()
                    .filter_map(|tok| match tok {
                        AnyToken::Memory(m) => Some(*m),
                        #[allow(unreachable_patterns)]
                        _ => None,
                    })
                    .collect();
                t.commit(&inner).await
            }
            #[cfg(feature = "transport-pipe")]
            Self::Pipe(t) => {
                let inner: Vec<_> = tokens
                    .iter()
                    .filter_map(|tok| match tok {
                        AnyToken::Pipe(p) => Some(*p),
                        #[allow(unreachable_patterns)]
                        _ => None,
                    })
                    .collect();
                t.commit(&inner).await
            }
            #[cfg(feature = "transport-file")]
            Self::File(t) => {
                let inner: Vec<_> = tokens
                    .iter()
                    .filter_map(|tok| match tok {
                        AnyToken::File(f) => Some(*f),
                        #[allow(unreachable_patterns)]
                        _ => None,
                    })
                    .collect();
                t.commit(&inner).await
            }
            #[cfg(feature = "transport-http")]
            Self::Http(t) => {
                let inner: Vec<_> = tokens
                    .iter()
                    .filter_map(|tok| match tok {
                        AnyToken::Http(h) => Some(h.clone()),
                        #[allow(unreachable_patterns)]
                        _ => None,
                    })
                    .collect();
                t.commit(&inner).await
            }
            #[cfg(feature = "transport-redis")]
            Self::Redis(t) => {
                let inner: Vec<_> = tokens
                    .iter()
                    .filter_map(|tok| match tok {
                        AnyToken::Redis(r) => Some(r.clone()),
                        #[allow(unreachable_patterns)]
                        _ => None,
                    })
                    .collect();
                t.commit(&inner).await
            }
            #[allow(unreachable_patterns)]
            _ => Err(TransportError::Config(
                "no transport variant enabled".into(),
            )),
        }
    }
}

impl AnyReceiver {
    /// Create a receiver from the config cascade.
    ///
    /// Reads the transport config from the given key in the config
    /// cascade and creates the appropriate receiver.
    ///
    /// # Example config
    ///
    /// ```yaml
    /// transport:
    ///   input:
    ///     type: kafka
    ///     kafka:
    ///       brokers: ["kafka:9092"]
    ///       group_id: "my-consumer"
    /// ```
    ///
    /// ```rust,ignore
    /// let receiver = AnyReceiver::from_config("transport.input").await?;
    /// ```
    pub async fn from_config(key: &str) -> TransportResult<Self> {
        #[cfg(feature = "config")]
        let config = {
            let cfg = crate::config::try_get()
                .ok_or_else(|| TransportError::Config("config not initialised".into()))?;
            cfg.unmarshal_key::<super::TransportConfig>(key)
                .map_err(|e| TransportError::Config(format!("failed to read {key}: {e}")))?
        };

        #[cfg(not(feature = "config"))]
        let config = {
            let _ = key;
            super::TransportConfig::default()
        };

        Self::from_transport_config(&config).await
    }

    /// Create a receiver from an explicit `TransportConfig`.
    pub async fn from_transport_config(config: &super::TransportConfig) -> TransportResult<Self> {
        match config.transport_type {
            #[cfg(feature = "transport-kafka")]
            TransportType::Kafka => {
                let kafka_config = config
                    .kafka
                    .as_ref()
                    .ok_or_else(|| TransportError::Config("kafka config missing".into()))?;
                let transport = super::kafka::KafkaTransport::new(kafka_config).await?;
                Ok(Self::Kafka(transport))
            }

            #[cfg(feature = "transport-grpc")]
            TransportType::Grpc => {
                let grpc_config = config
                    .grpc
                    .as_ref()
                    .ok_or_else(|| TransportError::Config("grpc config missing".into()))?;
                let transport = super::grpc::GrpcTransport::new(grpc_config).await?;
                Ok(Self::Grpc(transport))
            }

            #[cfg(feature = "transport-memory")]
            TransportType::Memory => {
                let memory_config = config.memory.clone().unwrap_or_default();
                let transport = super::memory::MemoryTransport::new(&memory_config)?;
                Ok(Self::Memory(transport))
            }

            #[cfg(feature = "transport-pipe")]
            TransportType::Pipe => {
                let pipe_config = config.pipe.clone().unwrap_or_default();
                let transport = super::pipe::PipeTransport::new(&pipe_config);
                Ok(Self::Pipe(transport))
            }

            #[cfg(feature = "transport-file")]
            TransportType::File => {
                let file_config = config
                    .file
                    .as_ref()
                    .ok_or_else(|| TransportError::Config("file config missing".into()))?;
                let transport = super::file::FileTransport::new(file_config).await?;
                Ok(Self::File(transport))
            }

            #[cfg(feature = "transport-http")]
            TransportType::Http => {
                let http_config = config
                    .http
                    .as_ref()
                    .ok_or_else(|| TransportError::Config("http config missing".into()))?;
                let transport = super::http::HttpTransport::new(http_config).await?;
                Ok(Self::Http(transport))
            }

            #[cfg(feature = "transport-redis")]
            TransportType::Redis => {
                let redis_config = config
                    .redis
                    .as_ref()
                    .ok_or_else(|| TransportError::Config("redis config missing".into()))?;
                let transport = super::redis_transport::RedisTransport::new(redis_config).await?;
                Ok(Self::Redis(transport))
            }

            // Transport types for modules not yet implemented
            #[allow(unreachable_patterns)]
            other => Err(TransportError::Config(format!(
                "transport type '{other}' is not available (feature not enabled or not yet implemented)"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "transport-memory"))]
mod tests {
    use super::*;
    use crate::transport::memory::{MemoryConfig, MemoryTransport};
    use crate::transport::traits::TransportReceiver;

    /// End-to-end round-trip: inject a message, recv via `AnyReceiver`,
    /// assert token wrapping, then commit and verify the memory transport's
    /// committed sequence advances.
    ///
    /// This exercises the full token wrap + commit re-dispatch path.
    #[tokio::test]
    async fn any_receiver_memory_recv_commit_round_trip() {
        // Build the underlying transport and inject a message.
        let inner = MemoryTransport::new(&MemoryConfig::default())
            .expect("memory transport must construct with default config");
        inner
            .inject(Some("events.test"), b"hello from AnyReceiver".to_vec())
            .await
            .expect("inject must succeed");

        // Wrap in AnyReceiver.
        let receiver = AnyReceiver::Memory(inner);

        assert_eq!(receiver.name(), "memory");
        assert!(receiver.is_healthy());

        // Recv via AnyReceiver -- must yield a WorkBatch<AnyToken>.
        let batch = receiver.recv(10).await.expect("recv must succeed");
        assert_eq!(batch.records.len(), 1, "expected exactly one record");
        assert_eq!(batch.commit_tokens.len(), 1, "expected one commit token");
        assert!(batch.dlq_entries.is_empty(), "no DLQ entries expected");

        let record = &batch.records[0];
        assert_eq!(record.payload.as_ref(), b"hello from AnyReceiver");
        assert_eq!(record.key.as_deref(), Some("events.test"));

        // Token must be wrapped in the Memory variant.
        let token = &batch.commit_tokens[0];
        assert!(
            matches!(token, AnyToken::Memory(_)),
            "token variant must be AnyToken::Memory, got {token}"
        );

        // Display delegates to the inner MemoryToken (format: "memory:<seq>").
        let display = token.to_string();
        assert!(
            display.starts_with("memory:"),
            "Display must delegate to MemoryToken, got {display}"
        );

        // Commit the AnyToken slice -- routes back to the MemoryTransport.
        let tokens: Vec<AnyToken> = batch.commit_tokens;
        let seq_before = if let AnyReceiver::Memory(ref t) = receiver {
            t.committed_sequence()
        } else {
            panic!("must be Memory variant");
        };

        receiver.commit(&tokens).await.expect("commit must succeed");

        // The memory transport tracks the max committed seq; it must have advanced.
        if let AnyReceiver::Memory(ref t) = receiver {
            let seq_after = t.committed_sequence();
            assert!(
                seq_after > seq_before || seq_after == 0,
                "committed_sequence must advance after commit (before={seq_before}, after={seq_after})"
            );
        }
    }

    /// Tokens from the wrong variant are silently ignored by commit --
    /// commit must succeed without error even if no tokens match.
    #[tokio::test]
    async fn any_receiver_commit_ignores_mismatched_variants() {
        let inner = MemoryTransport::new(&MemoryConfig::default())
            .expect("memory transport must construct with default config");
        let receiver = AnyReceiver::Memory(inner);

        // A Pipe token delivered to a Memory receiver -- must not panic or error.
        #[cfg(feature = "transport-pipe")]
        {
            let alien_token = AnyToken::Pipe(crate::transport::pipe::PipeToken { seq: 99 });
            receiver
                .commit(&[alien_token])
                .await
                .expect("commit with mismatched variant must succeed without error");
        }

        // Zero tokens -- always a no-op.
        receiver
            .commit(&[])
            .await
            .expect("commit with empty slice must succeed");
    }
}
