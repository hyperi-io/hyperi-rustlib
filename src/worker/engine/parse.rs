// Project:   hyperi-rustlib
// File:      src/worker/engine/parse.rs
// Purpose:   SIMD-accelerated payload parsing for the batch processing engine
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Parse phase: convert raw bytes into a `sonic_rs::Value` using SIMD
//! acceleration. This is the most CPU-intensive phase (~1–5 µs per message).
//!
//! - JSON:     `sonic_rs::from_slice` (AVX2/NEON SIMD, 2–4× faster than serde_json)
//! - MsgPack:  `rmp_serde` → `serde_json::Value` → JSON bytes → `sonic_rs::Value`
//!             (slower, but MsgPack messages are a small minority in practice)
//! - Auto:     byte-sniff via [`PayloadFormat::detect`], then dispatch

use super::types::PayloadFormat;

/// Error produced when a single message fails to parse.
#[derive(Debug)]
pub enum ParseError {
    /// Payload was empty — nothing to parse.
    Empty,
    /// JSON parse error from sonic_rs.
    Json(sonic_rs::Error),
    /// MsgPack decode error.
    MsgPack(String),
    /// Format not supported (feature gate not enabled).
    UnsupportedFormat(&'static str),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "empty payload"),
            Self::Json(e) => write!(f, "json parse error: {e}"),
            Self::MsgPack(msg) => write!(f, "msgpack decode error: {msg}"),
            Self::UnsupportedFormat(msg) => write!(f, "unsupported format: {msg}"),
        }
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

/// Parse raw bytes into a `sonic_rs::Value` using SIMD acceleration.
///
/// # Format dispatch
///
/// | Format | Engine |
/// |--------|--------|
/// | `Json` | `sonic_rs::from_slice` (SIMD) |
/// | `Auto` | byte-sniff → `Json` or `MsgPack` |
/// | `MsgPack` | `rmp_serde` → JSON bridge → `sonic_rs` (requires `worker-msgpack` feature) |
///
/// # Errors
///
/// Returns [`ParseError`] for empty payloads, malformed JSON/MsgPack, or when
/// the `worker-msgpack` feature is not enabled and a MsgPack payload is given.
pub fn parse_payload(payload: &[u8], format: PayloadFormat) -> Result<sonic_rs::Value, ParseError> {
    if payload.is_empty() {
        return Err(ParseError::Empty);
    }

    let effective = match format {
        PayloadFormat::Auto => PayloadFormat::detect(payload),
        other => other,
    };

    match effective {
        // Auto resolves to Json or MsgPack; treat residual Auto as Json.
        PayloadFormat::Json | PayloadFormat::Auto => {
            sonic_rs::from_slice(payload).map_err(ParseError::Json)
        }
        PayloadFormat::MsgPack => {
            #[cfg(feature = "worker-msgpack")]
            {
                // MsgPack → serde_json::Value → JSON bytes → sonic_rs::Value.
                // serde_json is used as the intermediate representation because
                // it supports both msgpack deserialization (via rmp_serde) and
                // JSON serialization for the sonic_rs bridge.
                let json_value: serde_json::Value = rmp_serde::from_slice(payload)
                    .map_err(|e| ParseError::MsgPack(e.to_string()))?;
                let json_bytes = serde_json::to_vec(&json_value)
                    .map_err(|e| ParseError::MsgPack(e.to_string()))?;
                sonic_rs::from_slice(&json_bytes).map_err(ParseError::Json)
            }
            #[cfg(not(feature = "worker-msgpack"))]
            {
                Err(ParseError::UnsupportedFormat(
                    "msgpack requires the worker-msgpack feature",
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use sonic_rs::JsonValueTrait as _;

    use super::*;

    #[test]
    fn parse_valid_json() {
        let payload = br#"{"host": "web1", "status": 200}"#;
        let value = parse_payload(payload, PayloadFormat::Json).unwrap();
        assert_eq!(value.get("host").and_then(|v| v.as_str()), Some("web1"));
        assert_eq!(value.get("status").and_then(|v| v.as_u64()), Some(200));
    }

    #[test]
    fn parse_auto_detects_json() {
        let payload = br#"{"_table": "events"}"#;
        let value = parse_payload(payload, PayloadFormat::Auto).unwrap();
        assert_eq!(value.get("_table").and_then(|v| v.as_str()), Some("events"));
    }

    #[test]
    fn parse_invalid_json_returns_error() {
        let payload = b"this is not json {";
        let result = parse_payload(payload, PayloadFormat::Json);
        assert!(
            matches!(result, Err(ParseError::Json(_))),
            "expected Json error, got {result:?}"
        );
    }

    #[test]
    fn parse_empty_payload_returns_empty_error() {
        let result = parse_payload(b"", PayloadFormat::Json);
        assert!(
            matches!(result, Err(ParseError::Empty)),
            "expected Empty error, got {result:?}"
        );
    }

    #[test]
    fn parse_empty_payload_auto_returns_empty_error() {
        let result = parse_payload(b"", PayloadFormat::Auto);
        assert!(matches!(result, Err(ParseError::Empty)));
    }

    #[test]
    fn parse_nested_json() {
        let payload = br#"{"meta": {"source": "kafka", "version": 3}, "data": [1, 2, 3]}"#;
        let value = parse_payload(payload, PayloadFormat::Json).unwrap();
        assert!(value.get("meta").is_some());
        assert!(value.get("data").is_some());
        // Verify nested field access.
        let meta = value.get("meta").unwrap();
        assert_eq!(meta.get("source").and_then(|v| v.as_str()), Some("kafka"));
    }

    #[test]
    fn parse_json_with_unicode() {
        let payload = "{\"name\": \"caf\\u00e9\"}".as_bytes();
        let value = parse_payload(payload, PayloadFormat::Json).unwrap();
        assert!(value.get("name").is_some());
    }

    #[test]
    fn parse_error_display_empty() {
        let e = ParseError::Empty;
        assert_eq!(e.to_string(), "empty payload");
    }

    #[test]
    fn parse_error_display_msgpack_unsupported() {
        // Without the worker-msgpack feature, MsgPack returns UnsupportedFormat.
        #[cfg(not(feature = "worker-msgpack"))]
        {
            // Construct a minimal fixmap: 0x81 = fixmap with 1 entry.
            let payload: &[u8] = &[0x81, 0xa3, b'k', b'e', b'y', 0x01];
            let result = parse_payload(payload, PayloadFormat::MsgPack);
            assert!(
                matches!(result, Err(ParseError::UnsupportedFormat(_))),
                "expected UnsupportedFormat, got {result:?}"
            );
        }
        #[cfg(feature = "worker-msgpack")]
        {
            // Feature is enabled; just verify the UnsupportedFormat variant
            // can still be constructed and displayed.
            let e = ParseError::UnsupportedFormat("test");
            assert!(e.to_string().contains("test"));
        }
    }
}
