// Project:   hyperi-rustlib
// File:      src/worker/engine/parse.rs
// Purpose:   SIMD-accelerated payload parsing for the batch processing engine
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Parse phase: convert raw bytes into a `sonic_rs::Value` using SIMD
//! acceleration. This is the most CPU-intensive phase (~1-5 µs per message).
//!
//! - JSON: `sonic_rs::from_slice` (AVX2/NEON SIMD, 2-4x faster than serde_json)
//! - MsgPack: `rmpv` native decode -> `sonic_rs::Value` via a direct value walker
//!   (no `rmp_serde -> serde_json` bridge; MsgPack messages are a small minority
//!   in practice)
//! - Auto: byte-sniff via [`PayloadFormat::detect`], then dispatch

use super::types::PayloadFormat;

/// Error produced when a single message fails to parse.
#[derive(Debug)]
pub enum ParseError {
    /// Payload was empty -- nothing to parse.
    Empty,
    /// JSON parse error from sonic_rs.
    Json(sonic_rs::Error),
    /// MsgPack decode error.
    MsgPack(String),
    /// Format not supported (feature gate not enabled).
    UnsupportedFormat(&'static str),
    /// Payload nests deeper than [`crate::parse_guard::MAX_PARSE_DEPTH`].
    /// Rejected BEFORE the recursive parser runs so a hostile deeply-nested
    /// payload cannot exhaust the worker stack (a per-record error, not a crash).
    TooDeep,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "empty payload"),
            Self::Json(e) => write!(f, "json parse error: {e}"),
            Self::MsgPack(msg) => write!(f, "msgpack decode error: {msg}"),
            Self::UnsupportedFormat(msg) => write!(f, "unsupported format: {msg}"),
            Self::TooDeep => write!(
                f,
                "payload nesting exceeds the maximum parse depth of {}",
                crate::parse_guard::MAX_PARSE_DEPTH
            ),
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
/// | `Auto` | byte-sniff -> `Json` or `MsgPack` |
/// | `MsgPack` | `rmpv` native decode -> `sonic_rs::Value` walker (requires `worker-msgpack` feature) |
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
            // Reject pathological nesting with a CHEAP ITERATIVE pre-scan before
            // the recursive SIMD parser runs -- a hostile deeply-nested payload
            // would otherwise exhaust the worker stack and abort the process.
            if !crate::parse_guard::json_depth_within(payload, crate::parse_guard::MAX_PARSE_DEPTH)
            {
                return Err(ParseError::TooDeep);
            }
            sonic_rs::from_slice(payload).map_err(ParseError::Json)
        }
        PayloadFormat::MsgPack => {
            #[cfg(feature = "worker-msgpack")]
            {
                // Native MsgPack: a SINGLE schema-less decode (the same decoder
                // `codec::parse` uses), then `rmpv_to_sonic` walks the value
                // straight into a `sonic_rs::Value` -- see that fn for the
                // bridge-free rationale.
                let mut cursor: &[u8] = payload;
                // Bound nesting depth so a hostile deeply-nested MsgPack payload
                // cannot exhaust the stack here or in the rmpv_to_sonic walk.
                let value = rmpv::decode::read_value_with_max_depth(
                    &mut cursor,
                    crate::parse_guard::MAX_PARSE_DEPTH,
                )
                .map_err(|e| ParseError::MsgPack(e.to_string()))?;
                Ok(rmpv_to_sonic(&value))
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

/// Convert a native `rmpv::Value` into a `sonic_rs::Value` with a direct value
/// walker -- NO `rmp_serde -> serde_json` bridge, no JSON re-serialise. The
/// engine keeps a `sonic_rs::Value`, so `ParsedMessage`, `extract_known`,
/// pre-route, the interner and the transform-closure contract are unchanged.
///
/// ## Mapping
///
/// - Nil -> JSON null; Boolean -> bool; F32/F64 -> JSON number.
/// - Integers fold to `i64` / `u64` (a `u64 > i64::MAX` stays unsigned;
///   otherwise it is surfaced as `f64`, matching the codec's lossy-but-numeric
///   policy for the rare oversized case).
/// - String: valid UTF-8 -> JSON string; otherwise the lossy form (a MsgPack
///   `str` is meant to be UTF-8, so this only bites on malformed input).
/// - Binary / Ext: base-relevant routing never keys off these, so a `bin` maps
///   to its lossy-UTF-8 string and `ext` to JSON null -- neither aborts the
///   parse (the bytes still round-trip via the original `Record.payload`).
/// - Array / Map: recurse. Non-string map keys are stringified so the object is
///   still addressable (JSON object keys must be strings).
#[cfg(feature = "worker-msgpack")]
fn rmpv_to_sonic(value: &rmpv::Value) -> sonic_rs::Value {
    use rmpv::Value as M;
    use sonic_rs::Value as S;

    // A non-finite float (NaN / +/-inf) has no JSON representation, so it folds
    // to null -- the same total-on-bad-input stance the rest of the walker takes.
    let from_f64 = |f: f64| S::new_f64(f).unwrap_or_else(S::new_null);

    match value {
        // Nil -> JSON null. Ext carries an application-defined type tag + bytes
        // with no JSON analogue and routers never key off it, so it folds to
        // null too (the bytes still round-trip via the original Record.payload).
        M::Nil | M::Ext(_, _) => S::new_null(),
        M::Boolean(b) => S::new_bool(*b),
        M::Integer(i) => {
            if let Some(n) = i.as_i64() {
                S::new_i64(n)
            } else if let Some(n) = i.as_u64() {
                S::new_u64(n)
            } else {
                // Cannot happen for a well-formed rmpv integer, but stay total.
                S::new_null()
            }
        }
        M::F32(f) => from_f64(f64::from(*f)),
        M::F64(f) => from_f64(*f),
        M::String(s) => match s.as_str() {
            Some(text) => S::from(text),
            None => S::from(String::from_utf8_lossy(s.as_bytes())),
        },
        M::Binary(bytes) => S::from(String::from_utf8_lossy(bytes)),
        M::Array(items) => {
            let mut arr = sonic_rs::Array::new();
            for item in items {
                arr.push(rmpv_to_sonic(item));
            }
            S::from(arr)
        }
        M::Map(pairs) => {
            let mut obj = sonic_rs::Object::new();
            for (k, v) in pairs {
                let key = msgpack_key_to_string(k);
                obj.insert(&key, rmpv_to_sonic(v));
            }
            S::from(obj)
        }
    }
}

/// Stringify an `rmpv` map key so it can be a JSON object key (which must be a
/// string). String keys pass through; everything else uses its `Display`-ish
/// form so the field stays addressable rather than being dropped.
#[cfg(feature = "worker-msgpack")]
fn msgpack_key_to_string(key: &rmpv::Value) -> String {
    use rmpv::Value as M;
    match key {
        M::String(s) => match s.as_str() {
            Some(text) => text.to_string(),
            None => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        },
        M::Integer(i) => i.to_string(),
        M::Boolean(b) => b.to_string(),
        M::Nil => "null".to_string(),
        other => format!("{other}"),
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

    // ---- Native MsgPack path (rmpv, NO rmp_serde -> serde_json bridge) -------
    //
    // Hand-roll the MsgPack bytes so the test exercises the NATIVE rmpv decoder
    // + value walker, not a serde round-trip.
    #[cfg(feature = "worker-msgpack")]
    mod msgpack_native {
        use super::*;

        /// fixstr: 0xa0 | len, then the UTF-8 bytes (len < 32).
        fn fixstr(s: &str) -> Vec<u8> {
            let bytes = s.as_bytes();
            let mut out = vec![0xa0 | u8::try_from(bytes.len()).expect("len < 32")];
            out.extend_from_slice(bytes);
            out
        }

        /// `{"_table":"events","org_id":42,"live":true,"ratio":1.5,"missing":nil}`
        /// as a MsgPack fixmap -- the same canonical record the codec tests use.
        fn sample() -> Vec<u8> {
            let mut buf = vec![0x80 | 5]; // fixmap(5)
            buf.extend(fixstr("_table"));
            buf.extend(fixstr("events"));
            buf.extend(fixstr("org_id"));
            buf.push(42); // positive fixint
            buf.extend(fixstr("live"));
            buf.push(0xc3); // true
            buf.extend(fixstr("ratio"));
            buf.push(0xcb); // float64
            buf.extend_from_slice(&1.5f64.to_be_bytes());
            buf.extend(fixstr("missing"));
            buf.push(0xc0); // nil
            buf
        }

        #[test]
        fn msgpack_native_decode_extracts_string_field() {
            let value = parse_payload(&sample(), PayloadFormat::MsgPack).unwrap();
            assert_eq!(value.get("_table").and_then(|v| v.as_str()), Some("events"));
        }

        #[test]
        fn msgpack_native_decode_preserves_scalar_types() {
            let value = parse_payload(&sample(), PayloadFormat::MsgPack).unwrap();
            assert_eq!(value.get("org_id").and_then(|v| v.as_i64()), Some(42));
            assert_eq!(value.get("live").and_then(|v| v.as_bool()), Some(true));
            assert_eq!(value.get("ratio").and_then(|v| v.as_f64()), Some(1.5));
            assert!(value.get("missing").is_some_and(|v| v.is_null()));
        }

        #[test]
        fn msgpack_auto_detects_and_decodes_natively() {
            // Leading fixmap byte (0x85) must auto-detect as MsgPack and decode.
            let value = parse_payload(&sample(), PayloadFormat::Auto).unwrap();
            assert_eq!(value.get("_table").and_then(|v| v.as_str()), Some("events"));
        }

        #[test]
        fn msgpack_nested_array_and_map_walk() {
            // {"items":[1,2],"meta":{"k":"v"}}
            let mut buf = vec![0x80 | 2];
            buf.extend(fixstr("items"));
            buf.push(0x90 | 2); // fixarray(2)
            buf.push(1);
            buf.push(2);
            buf.extend(fixstr("meta"));
            buf.push(0x80 | 1); // fixmap(1)
            buf.extend(fixstr("k"));
            buf.extend(fixstr("v"));

            let value = parse_payload(&buf, PayloadFormat::MsgPack).unwrap();
            let items = value.get("items").unwrap();
            assert_eq!(items[0].as_i64(), Some(1));
            assert_eq!(items[1].as_i64(), Some(2));
            assert_eq!(
                value
                    .get("meta")
                    .and_then(|m| m.get("k"))
                    .and_then(|v| v.as_str()),
                Some("v")
            );
        }

        #[test]
        fn malformed_msgpack_returns_msgpack_error() {
            // 0x81 declares a 1-entry fixmap but supplies no key/value.
            let result = parse_payload(&[0x81], PayloadFormat::MsgPack);
            assert!(
                matches!(result, Err(ParseError::MsgPack(_))),
                "expected MsgPack error, got {result:?}"
            );
        }
    }
}
