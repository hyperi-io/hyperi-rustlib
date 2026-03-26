// Project:   hyperi-rustlib
// File:      src/transport/factory.rs
// Purpose:   Transport factory — create senders from config
// Language:  Rust
//
// License:   FSL-1.1-ALv2
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
use super::traits::{TransportBase, TransportSender};
use super::types::{SendResult, TransportType};

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
        }
    }
}

impl TransportSender for AnySender {
    async fn send(&self, key: &str, payload: &[u8]) -> SendResult {
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
        let config = super::TransportConfig::default();

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
                let transport = super::memory::MemoryTransport::new(&memory_config);
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
