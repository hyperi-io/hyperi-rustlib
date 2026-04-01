// Project:   hyperi-rustlib
// File:      src/worker/engine/types.rs
// Purpose:   Core message types for the SIMD-optimised batch processing engine
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use bytes::Bytes;
use sonic_rs::JsonValueTrait as _;
use std::sync::Arc;

/// Payload format for a raw message.
///
/// Mirrors [`crate::transport::types::PayloadFormat`] for use when the `transport`
/// feature is not enabled. When `transport` is enabled, conversions between the
/// two types are provided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
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
    /// JSON objects start with `{` (0x7b).
    #[must_use]
    pub fn detect(payload: &[u8]) -> Self {
        if payload.is_empty() {
            return Self::Json;
        }
        if matches!(payload[0], 0x80..=0x8f | 0xde | 0xdf | 0x90..=0x9f | 0xdc | 0xdd) {
            Self::MsgPack
        } else {
            Self::Json
        }
    }
}

#[cfg(feature = "transport")]
impl From<crate::PayloadFormat> for PayloadFormat {
    fn from(f: crate::PayloadFormat) -> Self {
        match f {
            crate::PayloadFormat::Auto => Self::Auto,
            crate::PayloadFormat::Json => Self::Json,
            crate::PayloadFormat::MsgPack => Self::MsgPack,
        }
    }
}

#[cfg(feature = "transport")]
impl From<PayloadFormat> for crate::PayloadFormat {
    fn from(f: PayloadFormat) -> Self {
        match f {
            PayloadFormat::Auto => Self::Auto,
            PayloadFormat::Json => Self::Json,
            PayloadFormat::MsgPack => Self::MsgPack,
        }
    }
}

/// Type-erased commit token for use in [`MessageMetadata`].
///
/// Allows commit tokens from any transport to be stored without
/// introducing a generic parameter on [`RawMessage`].
pub trait CommitTokenErased: Send + Sync + std::fmt::Debug {
    /// Clone this token into a new box.
    fn clone_box(&self) -> Box<dyn CommitTokenErased>;
}

impl Clone for Box<dyn CommitTokenErased> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// Blanket impl: any `CommitToken` type auto-implements `CommitTokenErased`.
#[cfg(feature = "transport")]
impl<T: crate::CommitToken> CommitTokenErased for T {
    fn clone_box(&self) -> Box<dyn CommitTokenErased> {
        Box::new(self.clone())
    }
}

/// Metadata attached to a raw message.
///
/// Carries transport provenance — timestamp, detected format, and an optional
/// type-erased commit token so the engine can acknowledge messages without
/// knowing the concrete transport type.
#[derive(Debug, Clone)]
pub struct MessageMetadata {
    /// Message timestamp from the transport layer (milliseconds since epoch).
    pub timestamp_ms: Option<i64>,
    /// Detected or declared payload format.
    pub format: PayloadFormat,
    /// Type-erased commit token for acknowledgement after processing.
    pub commit_token: Option<Box<dyn CommitTokenErased>>,
}

/// Transport-agnostic raw message.
///
/// The lowest-level type in the engine — holds raw bytes plus just enough
/// metadata to route, parse, and commit. No generic parameters.
#[derive(Debug, Clone)]
pub struct RawMessage {
    /// Raw payload bytes (JSON or MsgPack, unchanged from transport).
    pub payload: Bytes,
    /// Routing key (Kafka topic, gRPC metadata key, etc.).
    pub key: Option<Arc<str>>,
    /// Transport headers (HTTP headers, Kafka headers, etc.).
    pub headers: Vec<(String, Vec<u8>)>,
    /// Transport provenance metadata.
    pub metadata: MessageMetadata,
}

/// Convert a typed `Message<T>` into a `RawMessage`.
///
/// The commit token is type-erased via `CommitTokenErased`. Requires the
/// `transport` feature for `Message<T>` and `CommitToken`.
#[cfg(feature = "transport")]
impl<T: crate::CommitToken> From<crate::Message<T>> for RawMessage {
    fn from(msg: crate::Message<T>) -> Self {
        RawMessage {
            payload: Bytes::from(msg.payload),
            key: msg.key,
            headers: vec![],
            metadata: MessageMetadata {
                timestamp_ms: msg.timestamp_ms,
                format: msg.format.into(),
                commit_token: Some(Box::new(msg.token)),
            },
        }
    }
}

/// A message that may or may not have been JSON-parsed.
///
/// The `Raw` variant holds the original bytes when parsing has not yet
/// occurred (or is deferred). The `Parsed` variant holds the parsed JSON
/// value alongside extracted fields for fast routing lookups.
#[derive(Debug, Clone)]
pub enum ParsedMessage {
    /// Unparsed message — bytes only.
    Raw(RawMessage),
    /// Successfully JSON-parsed message.
    Parsed {
        /// Full parsed JSON value.
        value: sonic_rs::Value,
        /// Original raw bytes (kept for zero-copy forwarding).
        raw: Bytes,
        /// Detected or declared format.
        format: PayloadFormat,
        /// Routing key.
        key: Option<Arc<str>>,
        /// Transport headers.
        headers: Vec<(String, Vec<u8>)>,
        /// Transport provenance metadata.
        metadata: MessageMetadata,
        /// Pre-extracted fields for fast routing (e.g. `_table`, `_timestamp`).
        ///
        /// Populated by the pre-route extraction step. Keys are interned `Arc<str>`
        /// to avoid repeated allocation during batch routing.
        extracted: std::collections::HashMap<Arc<str>, sonic_rs::Value>,
    },
}

impl ParsedMessage {
    /// Look up a field by name.
    ///
    /// Checks the `extracted` map first (interned fast path), then falls back
    /// to `value.get(name)` for the full parsed tree. Returns `None` for the
    /// `Raw` variant or when the field is absent.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&sonic_rs::Value> {
        match self {
            Self::Parsed {
                value, extracted, ..
            } => {
                // Fast path: check pre-extracted interned keys first.
                let interned = extracted
                    .keys()
                    .find(|k| k.as_ref() == name)
                    .and_then(|k| extracted.get(k));
                if interned.is_some() {
                    return interned;
                }
                // Slow path: walk full JSON value.
                value.get(name)
            }
            Self::Raw(_) => None,
        }
    }

    /// Return a reference to the parsed JSON value, if available.
    #[must_use]
    pub fn value(&self) -> Option<&sonic_rs::Value> {
        match self {
            Self::Parsed { value, .. } => Some(value),
            Self::Raw(_) => None,
        }
    }

    /// Return a mutable reference to the parsed JSON value, if available.
    #[must_use]
    pub fn value_mut(&mut self) -> Option<&mut sonic_rs::Value> {
        match self {
            Self::Parsed { value, .. } => Some(value),
            Self::Raw(_) => None,
        }
    }

    /// Return the raw payload bytes for both variants.
    #[must_use]
    pub fn raw_payload(&self) -> &[u8] {
        match self {
            Self::Parsed { raw, .. } => raw,
            Self::Raw(msg) => &msg.payload,
        }
    }

    /// Return the routing key for both variants.
    #[must_use]
    pub fn key(&self) -> Option<&str> {
        match self {
            Self::Parsed { key, .. } => key.as_deref(),
            Self::Raw(msg) => msg.key.as_deref(),
        }
    }

    /// Return transport provenance metadata for both variants.
    #[must_use]
    pub fn metadata(&self) -> &MessageMetadata {
        match self {
            Self::Parsed { metadata, .. } | Self::Raw(RawMessage { metadata, .. }) => metadata,
        }
    }
}

/// The outcome of a pre-route filter evaluation on a single message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreRouteResult {
    /// Allow the message to proceed through the pipeline.
    Continue,
    /// Drop the message silently (counted but not DLQ'd).
    Filtered,
    /// Route to the dead-letter queue with a reason string.
    Dlq(String),
    /// A required field was absent or had an unexpected type.
    FieldError(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonic_rs::JsonValueTrait as _;

    #[test]
    fn raw_message_construction() {
        let msg = RawMessage {
            payload: Bytes::from_static(b"{\"key\":\"value\"}"),
            key: Some(Arc::from("test-key")),
            headers: vec![],
            metadata: MessageMetadata {
                timestamp_ms: Some(1_234_567_890),
                format: PayloadFormat::Json,
                commit_token: None,
            },
        };
        assert_eq!(msg.payload.len(), 15);
        assert_eq!(msg.key.as_deref(), Some("test-key"));
    }

    #[test]
    fn parsed_message_field_access() {
        let mut extracted = std::collections::HashMap::new();
        let table_key: Arc<str> = Arc::from("_table");
        extracted.insert(Arc::clone(&table_key), sonic_rs::Value::from("events"));

        let msg = ParsedMessage::Parsed {
            value: sonic_rs::from_str(r#"{"_table":"events","host":"web1"}"#).unwrap(),
            raw: Bytes::from_static(b"{\"_table\":\"events\",\"host\":\"web1\"}"),
            format: PayloadFormat::Json,
            key: None,
            headers: vec![],
            metadata: MessageMetadata {
                timestamp_ms: None,
                format: PayloadFormat::Json,
                commit_token: None,
            },
            extracted,
        };

        assert_eq!(msg.field("_table").and_then(|v| v.as_str()), Some("events"));
        assert!(msg.value().is_some());
        assert!(!msg.raw_payload().is_empty());
    }

    #[test]
    fn parsed_message_raw_variant() {
        let msg = ParsedMessage::Raw(RawMessage {
            payload: Bytes::from_static(b"raw bytes"),
            key: None,
            headers: vec![],
            metadata: MessageMetadata {
                timestamp_ms: None,
                format: PayloadFormat::Json,
                commit_token: None,
            },
        });
        assert!(msg.value().is_none());
        assert_eq!(msg.raw_payload(), b"raw bytes");
    }

    #[test]
    fn parsed_message_field_falls_back_to_value() {
        let msg = ParsedMessage::Parsed {
            value: sonic_rs::from_str(r#"{"host":"web1"}"#).unwrap(),
            raw: Bytes::from_static(b"{\"host\":\"web1\"}"),
            format: PayloadFormat::Json,
            key: None,
            headers: vec![],
            metadata: MessageMetadata {
                timestamp_ms: None,
                format: PayloadFormat::Json,
                commit_token: None,
            },
            extracted: std::collections::HashMap::new(),
        };
        // field() should fall back to value.get() when extracted is empty.
        assert_eq!(msg.field("host").and_then(|v| v.as_str()), Some("web1"));
    }

    #[test]
    fn payload_format_detect_msgpack() {
        assert_eq!(PayloadFormat::detect(&[0x81, 0xa3]), PayloadFormat::MsgPack);
        assert_eq!(
            PayloadFormat::detect(&[0xde, 0x00, 0x10]),
            PayloadFormat::MsgPack
        );
    }

    #[test]
    fn payload_format_detect_json() {
        assert_eq!(PayloadFormat::detect(b"{\"x\":1}"), PayloadFormat::Json);
        assert_eq!(PayloadFormat::detect(b""), PayloadFormat::Json);
    }

    #[test]
    fn pre_route_result_variants() {
        assert_eq!(PreRouteResult::Continue, PreRouteResult::Continue);
        assert_eq!(PreRouteResult::Filtered, PreRouteResult::Filtered);
        assert!(matches!(
            PreRouteResult::Dlq("bad record".into()),
            PreRouteResult::Dlq(_)
        ));
        assert!(matches!(
            PreRouteResult::FieldError("missing _table".into()),
            PreRouteResult::FieldError(_)
        ));
    }

    #[test]
    fn metadata_accessor_on_raw() {
        let msg = ParsedMessage::Raw(RawMessage {
            payload: Bytes::from_static(b"data"),
            key: Some(Arc::from("k")),
            headers: vec![],
            metadata: MessageMetadata {
                timestamp_ms: Some(42),
                format: PayloadFormat::Auto,
                commit_token: None,
            },
        });
        assert_eq!(msg.metadata().timestamp_ms, Some(42));
        assert_eq!(msg.key(), Some("k"));
    }
}
