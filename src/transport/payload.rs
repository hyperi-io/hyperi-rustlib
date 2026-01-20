// Project:   hs-rustlib
// File:      src/transport/payload.rs
// Purpose:   Payload parsing and serialization (JSON/MsgPack)
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! # Payload Handling
//!
//! Unified JSON and MsgPack parsing with auto-detection.
//! This module provides a common pattern for all HyperSec projects.
//!
//! ## Performance Notes
//!
//! This implementation uses `serde_json` for JSON parsing. Projects requiring
//! maximum JSON performance (like dfe-loader-clickhouse) should use `sonic-rs`
//! directly for SIMD-accelerated parsing.
//!
//! ## Example
//!
//! ```rust,ignore
//! use hs_rustlib::transport::payload::{parse_payload, PayloadValue};
//!
//! let json_bytes = br#"{"event": "login", "user_id": 123}"#;
//! let value = parse_payload(json_bytes)?;
//!
//! if let PayloadValue::Json(obj) = value {
//!     println!("Parsed JSON: {:?}", obj);
//! }
//! ```

use super::error::{TransportError, TransportResult};
use super::types::PayloadFormat;
use serde::{de::DeserializeOwned, Serialize};

/// Parsed payload value.
#[derive(Debug, Clone)]
pub enum PayloadValue {
    /// JSON value (serde_json::Value).
    Json(serde_json::Value),
    /// MsgPack was converted to JSON value for uniform handling.
    MsgPack(serde_json::Value),
}

impl PayloadValue {
    /// Get the inner JSON value regardless of original format.
    #[must_use]
    pub fn as_json(&self) -> &serde_json::Value {
        match self {
            Self::Json(v) | Self::MsgPack(v) => v,
        }
    }

    /// Take ownership of the inner JSON value.
    #[must_use]
    pub fn into_json(self) -> serde_json::Value {
        match self {
            Self::Json(v) | Self::MsgPack(v) => v,
        }
    }

    /// Returns true if originally JSON.
    #[must_use]
    pub fn is_json(&self) -> bool {
        matches!(self, Self::Json(_))
    }

    /// Returns true if originally MsgPack.
    #[must_use]
    pub fn is_msgpack(&self) -> bool {
        matches!(self, Self::MsgPack(_))
    }
}

/// Parse payload bytes into a JSON value.
///
/// Auto-detects format (JSON or MsgPack) and converts to serde_json::Value.
/// MsgPack is converted to JSON for uniform downstream processing.
///
/// # Errors
///
/// Returns error if parsing fails for the detected format.
pub fn parse_payload(bytes: &[u8]) -> TransportResult<PayloadValue> {
    let format = PayloadFormat::detect(bytes);
    parse_payload_with_format(bytes, format)
}

/// Parse payload bytes with explicit format.
///
/// # Errors
///
/// Returns error if parsing fails.
pub fn parse_payload_with_format(
    bytes: &[u8],
    format: PayloadFormat,
) -> TransportResult<PayloadValue> {
    match format {
        PayloadFormat::Auto => parse_payload(bytes),
        PayloadFormat::Json => {
            let value: serde_json::Value = serde_json::from_slice(bytes)
                .map_err(|e| TransportError::Internal(format!("JSON parse error: {e}")))?;
            Ok(PayloadValue::Json(value))
        }
        PayloadFormat::MsgPack => {
            let value: serde_json::Value = rmp_serde::from_slice(bytes)
                .map_err(|e| TransportError::Internal(format!("MsgPack parse error: {e}")))?;
            Ok(PayloadValue::MsgPack(value))
        }
    }
}

/// Parse payload bytes into a typed struct.
///
/// Auto-detects format and deserializes directly to the target type.
///
/// # Errors
///
/// Returns error if parsing or deserialization fails.
pub fn parse_payload_typed<T: DeserializeOwned>(bytes: &[u8]) -> TransportResult<T> {
    let format = PayloadFormat::detect(bytes);
    match format {
        PayloadFormat::Json | PayloadFormat::Auto => serde_json::from_slice(bytes)
            .map_err(|e| TransportError::Internal(format!("JSON deserialize error: {e}"))),
        PayloadFormat::MsgPack => rmp_serde::from_slice(bytes)
            .map_err(|e| TransportError::Internal(format!("MsgPack deserialize error: {e}"))),
    }
}

/// Serialize a value to JSON bytes.
///
/// # Errors
///
/// Returns error if serialization fails.
pub fn serialize_json<T: Serialize>(value: &T) -> TransportResult<Vec<u8>> {
    serde_json::to_vec(value)
        .map_err(|e| TransportError::Internal(format!("JSON serialize error: {e}")))
}

/// Serialize a value to MsgPack bytes.
///
/// # Errors
///
/// Returns error if serialization fails.
pub fn serialize_msgpack<T: Serialize>(value: &T) -> TransportResult<Vec<u8>> {
    rmp_serde::to_vec(value)
        .map_err(|e| TransportError::Internal(format!("MsgPack serialize error: {e}")))
}

/// Serialize a value to the specified format.
///
/// # Errors
///
/// Returns error if serialization fails.
pub fn serialize_payload<T: Serialize>(
    value: &T,
    format: PayloadFormat,
) -> TransportResult<Vec<u8>> {
    match format {
        PayloadFormat::Json | PayloadFormat::Auto => serialize_json(value),
        PayloadFormat::MsgPack => serialize_msgpack(value),
    }
}

/// Extract a field from JSON bytes without full parsing.
///
/// This is a simple implementation. For high-performance field extraction,
/// use sonic-rs `get_from_slice()` in performance-critical code.
///
/// # Errors
///
/// Returns error if the bytes are not valid JSON or field is not found.
pub fn extract_field(bytes: &[u8], field: &str) -> TransportResult<Option<serde_json::Value>> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| TransportError::Internal(format!("JSON parse error: {e}")))?;

    Ok(value.get(field).cloned())
}

/// Extract a nested field using dot notation (e.g., "tags.event.org_id").
///
/// # Errors
///
/// Returns error if the bytes are not valid JSON.
pub fn extract_nested_field(
    bytes: &[u8],
    path: &str,
) -> TransportResult<Option<serde_json::Value>> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| TransportError::Internal(format!("JSON parse error: {e}")))?;

    let mut current = &value;
    for part in path.split('.') {
        match current.get(part) {
            Some(v) => current = v,
            None => return Ok(None),
        }
    }

    Ok(Some(current.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json_object() {
        let bytes = br#"{"foo": "bar", "num": 42}"#;
        let value = parse_payload(bytes).unwrap();
        assert!(value.is_json());

        let json = value.as_json();
        assert_eq!(json["foo"], "bar");
        assert_eq!(json["num"], 42);
    }

    #[test]
    fn parse_msgpack() {
        // MsgPack for {"foo": "bar"}
        // fixmap(1) + fixstr(3) "foo" + fixstr(3) "bar"
        let bytes = [
            0x81, // fixmap with 1 element
            0xa3, b'f', b'o', b'o', // fixstr(3) "foo"
            0xa3, b'b', b'a', b'r', // fixstr(3) "bar"
        ];
        let value = parse_payload(&bytes).unwrap();
        assert!(value.is_msgpack());

        let json = value.as_json();
        assert_eq!(json["foo"], "bar");
    }

    #[test]
    fn extract_simple_field() {
        let bytes = br#"{"event": "login", "user_id": 123}"#;
        let field = extract_field(bytes, "event").unwrap();
        assert_eq!(field, Some(serde_json::json!("login")));
    }

    #[test]
    fn extract_nested_field_path() {
        let bytes = br#"{"tags": {"event": {"org_id": "acme"}}}"#;
        let field = extract_nested_field(bytes, "tags.event.org_id").unwrap();
        assert_eq!(field, Some(serde_json::json!("acme")));
    }

    #[test]
    fn extract_missing_field() {
        let bytes = br#"{"foo": "bar"}"#;
        let field = extract_field(bytes, "missing").unwrap();
        assert_eq!(field, None);
    }

    #[test]
    fn serialize_roundtrip() {
        #[derive(Debug, PartialEq, Serialize, serde::Deserialize)]
        struct Event {
            name: String,
            value: i32,
        }

        let event = Event {
            name: "test".to_string(),
            value: 42,
        };

        // JSON roundtrip
        let json_bytes = serialize_json(&event).unwrap();
        let parsed: Event = parse_payload_typed(&json_bytes).unwrap();
        assert_eq!(event, parsed);

        // MsgPack roundtrip
        let msgpack_bytes = serialize_msgpack(&event).unwrap();
        let parsed: Event = parse_payload_typed(&msgpack_bytes).unwrap();
        assert_eq!(event, parsed);
    }
}
