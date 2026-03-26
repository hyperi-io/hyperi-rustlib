// Project:   hyperi-rustlib
// File:      src/transport/types.rs
// Purpose:   Transport data types and configuration
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use super::error::TransportError;
use super::traits::CommitToken;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Transport type selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportType {
    /// Apache Kafka (production default).
    #[default]
    Kafka,
    /// DFE native gRPC (tonic, inter-service mesh).
    Grpc,
    /// In-memory tokio channels (unit tests).
    Memory,
    /// NDJSON file (debugging, audit trails, replay).
    File,
    /// Unix pipe (stdin/stdout, sidecar pattern).
    Pipe,
    /// HTTP/HTTPS (webhook delivery, REST ingest).
    Http,
    /// Redis/Valkey Streams (lightweight pub/sub).
    Redis,
}

impl std::fmt::Display for TransportType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Kafka => write!(f, "kafka"),
            Self::Grpc => write!(f, "grpc"),
            Self::Memory => write!(f, "memory"),
            Self::File => write!(f, "file"),
            Self::Pipe => write!(f, "pipe"),
            Self::Http => write!(f, "http"),
            Self::Redis => write!(f, "redis"),
        }
    }
}

/// Payload format (auto-detected or explicit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PayloadFormat {
    /// Auto-detect from payload bytes.
    #[default]
    Auto,
    /// JSON format.
    Json,
    /// MessagePack format.
    MsgPack,
}

impl PayloadFormat {
    /// Detect format from payload bytes.
    ///
    /// MsgPack maps start with 0x80-0x8f (fixmap) or 0xde/0xdf (map16/map32).
    /// JSON objects start with '{' (0x7b).
    /// JSON arrays start with '[' (0x5b).
    #[must_use]
    pub fn detect(payload: &[u8]) -> Self {
        if payload.is_empty() {
            return Self::Json; // Default to JSON for empty
        }

        // MsgPack: fixmap (0x80-0x8f), map16/32 (0xde/0xdf), fixarray (0x90-0x9f), array16/32 (0xdc/0xdd)
        if matches!(payload[0], 0x80..=0x8f | 0xde | 0xdf | 0x90..=0x9f | 0xdc | 0xdd) {
            Self::MsgPack
        } else {
            // JSON object/array or whitespace-prefixed JSON - default to JSON
            Self::Json
        }
    }
}

/// A received message with transport metadata.
#[derive(Debug, Clone)]
pub struct Message<T: CommitToken> {
    /// Routing key (Kafka topic, gRPC metadata topic).
    pub key: Option<Arc<str>>,

    /// Raw payload bytes - JSON or MsgPack, unchanged.
    pub payload: Vec<u8>,

    /// Transport-specific commit token.
    pub token: T,

    /// Message timestamp from transport layer (milliseconds since epoch).
    pub timestamp_ms: Option<i64>,

    /// Detected payload format.
    pub format: PayloadFormat,
}

impl<T: CommitToken> Message<T> {
    /// Create a new message with auto-detected format.
    #[must_use]
    pub fn new(
        key: Option<Arc<str>>,
        payload: Vec<u8>,
        token: T,
        timestamp_ms: Option<i64>,
    ) -> Self {
        let format = PayloadFormat::detect(&payload);
        Self {
            key,
            payload,
            token,
            timestamp_ms,
            format,
        }
    }

    /// Returns true if payload is JSON.
    #[must_use]
    pub fn is_json(&self) -> bool {
        self.format == PayloadFormat::Json
    }

    /// Returns true if payload is MsgPack.
    #[must_use]
    pub fn is_msgpack(&self) -> bool {
        self.format == PayloadFormat::MsgPack
    }
}

/// Result of a send operation.
#[derive(Debug)]
pub enum SendResult {
    /// Message accepted.
    Ok,
    /// Transport is backpressured, retry later.
    Backpressured,
    /// Fatal error, cannot continue.
    Fatal(TransportError),
}

impl SendResult {
    /// Returns true if send was successful.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok)
    }

    /// Returns true if backpressured (should retry).
    #[must_use]
    pub fn is_backpressured(&self) -> bool {
        matches!(self, Self::Backpressured)
    }

    /// Returns true if fatal error.
    #[must_use]
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::Fatal(_))
    }
}

/// Top-level transport configuration.
///
/// Used by the transport factory to create the right backend from config.
/// Each transport type has its own optional config section — only the one
/// matching `transport_type` is read.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TransportConfig {
    /// Transport type (kafka, grpc, memory, file, pipe, http, redis).
    #[serde(rename = "type", default)]
    pub transport_type: TransportType,

    /// Expected payload format (auto-detect by default).
    #[serde(default)]
    pub payload_format: PayloadFormat,

    /// Kafka-specific configuration.
    #[cfg(feature = "transport-kafka")]
    #[serde(default)]
    pub kafka: Option<super::kafka::KafkaConfig>,

    /// gRPC transport configuration.
    #[cfg(feature = "transport-grpc")]
    #[serde(default)]
    pub grpc: Option<super::grpc::GrpcConfig>,

    /// Memory transport configuration (for tests).
    #[cfg(feature = "transport-memory")]
    #[serde(default)]
    pub memory: Option<super::memory::MemoryConfig>,

    /// Pipe transport configuration (stdin/stdout).
    #[cfg(feature = "transport-pipe")]
    #[serde(default)]
    pub pipe: Option<super::pipe::PipeTransportConfig>,

    /// File transport configuration (NDJSON file I/O).
    #[cfg(feature = "transport-file")]
    #[serde(default)]
    pub file: Option<super::file::FileTransportConfig>,

    /// HTTP transport configuration (webhook delivery, REST ingest).
    #[cfg(feature = "transport-http")]
    #[serde(default)]
    pub http: Option<super::http::HttpTransportConfig>,

    // Placeholder fields when features are disabled
    #[cfg(not(feature = "transport-kafka"))]
    #[serde(default, skip)]
    pub kafka: Option<()>,

    #[cfg(not(feature = "transport-grpc"))]
    #[serde(default, skip)]
    pub grpc: Option<()>,

    #[cfg(not(feature = "transport-memory"))]
    #[serde(default, skip)]
    pub memory: Option<()>,

    #[cfg(not(feature = "transport-pipe"))]
    #[serde(default, skip)]
    pub pipe: Option<()>,

    #[cfg(not(feature = "transport-file"))]
    #[serde(default, skip)]
    pub file: Option<()>,

    #[cfg(not(feature = "transport-http"))]
    #[serde(default, skip)]
    pub http: Option<()>,

    /// Redis/Valkey Streams transport configuration.
    #[cfg(feature = "transport-redis")]
    #[serde(default)]
    pub redis: Option<super::redis_transport::RedisTransportConfig>,

    #[cfg(not(feature = "transport-redis"))]
    #[serde(default, skip)]
    pub redis: Option<()>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_json_object() {
        assert_eq!(PayloadFormat::detect(b"{\"foo\":1}"), PayloadFormat::Json);
    }

    #[test]
    fn detect_json_array() {
        assert_eq!(PayloadFormat::detect(b"[1,2,3]"), PayloadFormat::Json);
    }

    #[test]
    fn detect_msgpack_fixmap() {
        // fixmap with 1 element: 0x81
        assert_eq!(PayloadFormat::detect(&[0x81, 0xa3]), PayloadFormat::MsgPack);
    }

    #[test]
    fn detect_msgpack_map16() {
        assert_eq!(
            PayloadFormat::detect(&[0xde, 0x00, 0x10]),
            PayloadFormat::MsgPack
        );
    }

    #[test]
    fn detect_empty_defaults_json() {
        assert_eq!(PayloadFormat::detect(&[]), PayloadFormat::Json);
    }
}
