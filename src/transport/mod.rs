// Project:   hyperi-rustlib
// File:      src/transport/mod.rs
// Purpose:   Transport abstraction layer for message delivery
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # Transport Abstraction Layer
//!
//! Pluggable message transport with split sender/receiver traits for
//! type-safe factory construction and runtime transport selection.
//!
//! ## Architecture
//!
//! ```text
//! TransportSender (object-safe)     TransportReceiver<Token> (generic)
//!   send(key, payload)                recv(max) -> Vec<Message<Token>>
//!   close()                           commit(tokens)
//!   is_healthy()                      close()
//!   name()                            is_healthy(), name()
//!         |                                    |
//!         +-------- Transport (blanket) -------+
//! ```
//!
//! - **Output stages** (DLQ, forwarding, archiving): use `Box<dyn TransportSender>`
//! - **Input stages** (receiver, fetcher): use concrete `impl TransportReceiver`
//! - **Factory**: `sender_from_config()` returns `Box<dyn TransportSender>`
//!
//! ## Transport Selection
//!
//! | Transport | Send | Recv | Use Case |
//! |-----------|------|------|----------|
//! | **Kafka** | Yes | Yes | Production default, PB/day, persistence |
//! | **gRPC** | Yes | Yes | Low-latency direct, DFE mesh |
//! | **Memory** | Yes | Yes | Unit tests, same-process |
//! | **File** | Yes | Yes | Debugging, audit trails, replay |
//! | **Pipe** | Yes | Yes | Unix pipelines, sidecar pattern |
//! | **HTTP** | Yes | Yes | Webhook delivery, REST ingest |
//! | **Redis** | Yes | Yes | Edge deployments, lightweight pub/sub |
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::transport::{TransportSender, TransportConfig};
//!
//! // Factory creates the right backend from config
//! let sender: Box<dyn TransportSender> = transport::sender_from_config("transport.output").await?;
//! sender.send("events.land", payload).await;
//! ```

mod detect;
mod error;
pub mod factory;
mod payload;
mod traits;
mod types;

// Re-export payload utilities
pub use payload::{
    PayloadValue, extract_field, extract_nested_field, parse_payload, parse_payload_typed,
    parse_payload_with_format, serialize_json, serialize_msgpack, serialize_payload,
};
pub use types::PayloadFormat;

// Re-export stateful format detection
pub use detect::{DetectedFormat, FormatDetector, FormatMode, detect_format};

#[cfg(feature = "transport-kafka")]
pub mod kafka;

#[cfg(feature = "transport-grpc")]
pub mod grpc;

#[cfg(feature = "transport-grpc-vector-compat")]
pub mod vector_compat;

#[cfg(feature = "transport-memory")]
pub mod memory;

#[cfg(feature = "transport-pipe")]
pub mod pipe;

#[cfg(feature = "transport-file")]
pub mod file;

#[cfg(feature = "transport-http")]
pub mod http;

#[cfg(feature = "transport-redis")]
pub mod redis_transport;

pub mod routed;

// Re-exports — traits and factory
pub use error::{TransportError, TransportResult};
pub use factory::AnySender;
pub use routed::RoutedSender;
pub use traits::{CommitToken, Transport, TransportBase, TransportReceiver, TransportSender};
pub use types::{Message, SendResult, TransportConfig, TransportType};

#[cfg(feature = "transport-kafka")]
pub use kafka::{KafkaConfig, KafkaToken, KafkaTransport};

#[cfg(feature = "transport-grpc")]
pub use grpc::{GrpcConfig, GrpcToken, GrpcTransport};

#[cfg(feature = "transport-grpc-vector-compat")]
pub use vector_compat::{VectorCompatClient, VectorCompatService};

#[cfg(feature = "transport-memory")]
pub use memory::{MemoryConfig, MemoryToken, MemoryTransport};

#[cfg(feature = "transport-pipe")]
pub use pipe::{PipeToken, PipeTransport, PipeTransportConfig};

#[cfg(feature = "transport-file")]
pub use file::{FileToken, FileTransport, FileTransportConfig};

#[cfg(feature = "transport-http")]
pub use http::{HttpToken, HttpTransport, HttpTransportConfig};

#[cfg(feature = "transport-redis")]
pub use redis_transport::{RedisToken, RedisTransport, RedisTransportConfig};
