// Project:   hyperi-rustlib
// File:      src/transport/mod.rs
// Purpose:   Transport abstraction layer for message delivery
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # Transport Abstraction Layer
//!
//! Pluggable message transport supporting Kafka, Zenoh, and in-memory channels.
//! All transports deliver raw bytes (JSON or MsgPack) without any envelope format.
//!
//! ## Transport Selection
//!
//! | Transport | Use Case | Durability |
//! |-----------|----------|------------|
//! | **Kafka** | Production (default) | At-least-once with broker persistence |
//! | **Zenoh** | Dev/test, low-latency | In-flight only, no persistence |
//! | **Memory** | Unit tests | None, same-process only |
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::transport::{Transport, TransportConfig, TransportType};
//!
//! // Create transport from config
//! let config = TransportConfig {
//!     transport_type: TransportType::Kafka,
//!     kafka: Some(KafkaConfig { /* ... */ }),
//!     ..Default::default()
//! };
//! let transport = create_transport(&config).await?;
//!
//! // Receive messages
//! let messages = transport.recv(100).await?;
//! for msg in &messages {
//!     println!("Received: {} bytes", msg.payload.len());
//! }
//!
//! // Commit after processing
//! let tokens: Vec<_> = messages.iter().map(|m| m.token.clone()).collect();
//! transport.commit(&tokens).await?;
//! ```

mod detect;
mod error;
mod payload;
mod traits;
mod types;

// Re-export payload utilities
pub use payload::{
    extract_field, extract_nested_field, parse_payload, parse_payload_typed,
    parse_payload_with_format, serialize_json, serialize_msgpack, serialize_payload, PayloadValue,
};
pub use types::PayloadFormat;

// Re-export stateful format detection
pub use detect::{detect_format, DetectedFormat, FormatDetector, FormatMode};

#[cfg(feature = "transport-kafka")]
pub mod kafka;

#[cfg(feature = "transport-zenoh")]
pub mod zenoh;

#[cfg(feature = "transport-memory")]
pub mod memory;

// Re-exports
pub use error::{TransportError, TransportResult};
pub use traits::{CommitToken, Transport};
pub use types::{Message, SendResult, TransportConfig, TransportType};

#[cfg(feature = "transport-kafka")]
pub use kafka::{KafkaConfig, KafkaToken, KafkaTransport};

#[cfg(feature = "transport-zenoh")]
pub use zenoh::{ZenohConfig, ZenohToken, ZenohTransport};

#[cfg(feature = "transport-memory")]
pub use memory::{MemoryConfig, MemoryToken, MemoryTransport};

// Note: Transport instances are created directly via their constructors
// (e.g., KafkaTransport::new(), ZenohTransport::new(), MemoryTransport::new())
// rather than through a factory function, because each transport has a
// different Token associated type that can't be erased without losing type safety.
