// Project:   hyperi-rustlib
// File:      src/worker/engine/types.rs
// Purpose:   Core message types for the SIMD-optimised batch processing engine
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

use bytes::Bytes;
use sonic_rs::JsonValueTrait as _;
use std::sync::Arc;

/// Payload format for a raw message.
///
/// Mirrors `crate::transport::types::PayloadFormat` for use when the `transport`
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

/// Metadata attached to a parsed message.
///
/// Carries transport provenance -- timestamp and detected format. Commit
/// tokens are NOT carried here: they live on [`crate::transport::WorkBatch`]'s
/// `commit_tokens` and are decoupled from individual records.
#[derive(Debug, Clone)]
pub struct MessageMetadata {
    /// Message timestamp from the transport layer (milliseconds since epoch).
    pub timestamp_ms: Option<i64>,
    /// Detected or declared payload format.
    pub format: PayloadFormat,
}

/// A JSON-parsed message.
///
/// Holds the parsed JSON value alongside extracted fields for fast routing
/// lookups. Built by the engine's parse step from a [`crate::transport::Record`].
#[derive(Debug, Clone)]
pub enum ParsedMessage {
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
        let Self::Parsed {
            value, extracted, ..
        } = self;
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

    /// Return a reference to the parsed JSON value.
    #[must_use]
    pub fn value(&self) -> Option<&sonic_rs::Value> {
        let Self::Parsed { value, .. } = self;
        Some(value)
    }

    /// Return a mutable reference to the parsed JSON value.
    #[must_use]
    pub fn value_mut(&mut self) -> Option<&mut sonic_rs::Value> {
        let Self::Parsed { value, .. } = self;
        Some(value)
    }

    /// Return the raw payload bytes.
    #[must_use]
    pub fn raw_payload(&self) -> &[u8] {
        let Self::Parsed { raw, .. } = self;
        raw
    }

    /// Return the routing key.
    #[must_use]
    pub fn key(&self) -> Option<&str> {
        let Self::Parsed { key, .. } = self;
        key.as_deref()
    }

    /// Return transport provenance metadata.
    #[must_use]
    pub fn metadata(&self) -> &MessageMetadata {
        let Self::Parsed { metadata, .. } = self;
        metadata
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
            },
            extracted,
        };

        assert_eq!(msg.field("_table").and_then(|v| v.as_str()), Some("events"));
        assert!(msg.value().is_some());
        assert!(!msg.raw_payload().is_empty());
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
    fn metadata_accessor() {
        let msg = ParsedMessage::Parsed {
            value: sonic_rs::from_str(r#"{"x":1}"#).unwrap(),
            raw: Bytes::from_static(b"{\"x\":1}"),
            format: PayloadFormat::Auto,
            key: Some(Arc::from("k")),
            headers: vec![],
            metadata: MessageMetadata {
                timestamp_ms: Some(42),
                format: PayloadFormat::Auto,
            },
            extracted: std::collections::HashMap::new(),
        };
        assert_eq!(msg.metadata().timestamp_ms, Some(42));
        assert_eq!(msg.key(), Some("k"));
    }
}
