// Project:   hyperi-rustlib
// File:      src/transport/codec.rs
// Purpose:   Parse-on-demand WorkBatch codec (native JSON + MsgPack, no bridge)
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # Parse-on-demand codec (Task 0.3a)
//!
//! The data-plane spine frames bytes into a [`WorkBatch`](super::WorkBatch)
//! WITHOUT parsing (Task 0.2). A transform / router that needs to read a field
//! parses on demand -- and it should not care whether the record arrived as
//! JSON or MsgPack. This module is that parse step.
//!
//! ## Native, no JSON bridge
//!
//! Today's scattered serde (engine `parse.rs`, transport `payload.rs`) decodes
//! MsgPack via a BRIDGE: `rmp_serde -> serde_json::Value -> serde_json::to_vec
//! -> sonic_rs`. That is two parses and a re-serialise per MsgPack record, and
//! it defeats the SIMD JSON fast path entirely. This module does NOT do that:
//!
//! - **JSON** is parsed once with [`sonic_rs`] (SIMD, AVX2/NEON).
//! - **MsgPack** is parsed once with [`rmpv`] -- the schema-less `Value` decoder
//!   from the same `3Hren/msgpack-rust` workspace as `rmp-serde`. No
//!   intermediate `serde_json::Value`, no JSON re-serialise.
//!
//! The two scattered call sites are NOT removed here -- this is additive. The
//! rip-out / consolidation lands in Phase 0.7 when the engine migrates onto the
//! WorkBatch spine.
//!
//! ## Unified routing-field accessor
//!
//! A router keys off ONE field (`_table`, `org_id`, ...) and must not branch on
//! wire format. [`ParsedPayload`] exposes a format-agnostic accessor:
//!
//! - [`ParsedPayload::field_str`] -- the common case: a top-level string field.
//! - [`ParsedPayload::field`] -- a [`FieldRef`] covering the scalar routing
//!   cases (string / int / float / bool / null), with everything else
//!   ([`FieldRef::Other`]) deliberately collapsed because routers do not key
//!   off nested containers.
//!
//! **Scope:** top-level object-key lookup only. No deep JSON-path -- routing
//! keys live at the top level, and a deep-path query is a separate concern that
//! YAGNI keeps out of the hot routing path.
//!
//! See `docs/MIGRATIONS.md` (codec consolidation: native rmpv, JSON bridge
//! removed) and `docs/SELF-REGULATION.md` for where this codec sits in the
//! `WorkBatch` data-plane spine. The block contract is in
//! [`work_batch`](super::work_batch).

use super::types::PayloadFormat;
use bytes::Bytes;
use sonic_rs::JsonValueTrait as _;
use thiserror::Error;

/// A parse failure, tagged by the format that failed.
#[derive(Debug, Error)]
pub enum CodecError {
    /// JSON parse failed (sonic_rs SIMD parser).
    #[error("json parse error: {0}")]
    Json(#[from] sonic_rs::Error),

    /// MsgPack parse failed (native rmpv decoder).
    #[error("msgpack parse error: {0}")]
    MsgPack(#[from] rmpv::decode::Error),

    /// MsgPack serialise failed (native rmpv encoder).
    ///
    /// `rmpv::encode::Error` is `rmp::encode::ValueWriteError` -- an I/O write
    /// failure from the underlying writer. Serialising into an in-memory `Vec`
    /// effectively never fails, but the encoder is fallible so we surface it
    /// rather than panic. JSON serialise reuses [`CodecError::Json`]
    /// (`sonic_rs::Error` already covers both parse and serialise).
    #[error("msgpack encode error: {0}")]
    Encode(#[from] rmpv::encode::Error),
}

/// A parsed payload, retaining its native value representation.
///
/// JSON stays a [`sonic_rs::Value`] (so the SIMD parse is not thrown away) and
/// MsgPack stays an [`rmpv::Value`] (native, no JSON bridge). A consumer that
/// only needs a routing field should reach for [`ParsedPayload::field_str`] /
/// [`ParsedPayload::field`] rather than matching the variant -- that is the
/// whole point of the unified accessor.
#[derive(Debug, Clone)]
pub enum ParsedPayload {
    /// JSON value parsed by sonic_rs (SIMD).
    Json(sonic_rs::Value),
    /// MsgPack value parsed natively by rmpv (no serde_json bridge).
    MsgPack(rmpv::Value),
}

/// A borrowed view of one routing field, format-agnostic.
///
/// This is the shared currency the unified accessor returns so a router need
/// not know whether the record was JSON or MsgPack. It covers the scalar cases
/// a router actually keys off; nested objects / arrays / binary / ext collapse
/// to [`FieldRef::Other`] because routing never branches on a container.
///
/// `Str` borrows from the parsed value (zero-copy); the numeric / bool variants
/// are `Copy` scalars.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FieldRef<'a> {
    /// A string field (borrowed from the parsed value).
    Str(&'a str),
    /// An integer field (MsgPack ints and JSON integers fold to `i64`).
    Int(i64),
    /// A floating-point field.
    Float(f64),
    /// A boolean field.
    Bool(bool),
    /// An explicit null / nil field.
    Null,
    /// Present but not a routing scalar (object, array, binary, ext, ...).
    Other,
}

/// Parse a framed payload into a native [`ParsedPayload`].
///
/// - [`PayloadFormat::Json`] -> sonic_rs (SIMD).
/// - [`PayloadFormat::MsgPack`] -> rmpv (native, no JSON bridge).
/// - [`PayloadFormat::Auto`] -> [`PayloadFormat::detect`] then dispatch. An empty
///   blob detects as JSON (matching `detect`'s contract) and surfaces a
///   [`CodecError::Json`] -- empty input is not valid JSON.
///
/// # Errors
///
/// Returns [`CodecError::Json`] or [`CodecError::MsgPack`] when the bytes are
/// malformed for the (detected or declared) format.
pub fn parse(payload: &Bytes, format: PayloadFormat) -> Result<ParsedPayload, CodecError> {
    let effective = match format {
        PayloadFormat::Auto => PayloadFormat::detect(payload),
        other => other,
    };

    match effective {
        // detect() never yields Auto, but treat a residual Auto as JSON.
        PayloadFormat::Json | PayloadFormat::Auto => {
            let value: sonic_rs::Value = sonic_rs::from_slice(payload)?;
            Ok(ParsedPayload::Json(value))
        }
        PayloadFormat::MsgPack => {
            // rmpv::decode::read_value reads from any `io::Read`; a byte slice
            // is one. `&mut &[u8]` advances the cursor as it decodes. This is a
            // SINGLE native decode -- no rmp_serde, no serde_json, no re-encode.
            let mut cursor: &[u8] = payload.as_ref();
            let value = rmpv::decode::read_value(&mut cursor)?;
            Ok(ParsedPayload::MsgPack(value))
        }
    }
}

/// Serialise a JSON value to bytes via [`sonic_rs`] (SIMD), no bridge.
///
/// The inverse of the JSON arm of [`parse`]. Reuses sonic_rs end-to-end so a
/// transform that mutates a parsed JSON value re-emits it without ever touching
/// `serde_json`.
///
/// # Errors
///
/// Returns [`CodecError::Json`] if sonic_rs fails to serialise the value.
pub fn to_json_bytes(value: &sonic_rs::Value) -> Result<Bytes, CodecError> {
    let buf = sonic_rs::to_vec(value)?;
    Ok(Bytes::from(buf))
}

/// Serialise a MsgPack value to bytes via NATIVE [`rmpv::encode::write_value`].
///
/// The inverse of the MsgPack arm of [`parse`]. This is the native rmpv encoder
/// -- NOT `rmp_serde`, NOT a JSON bridge. A transform that mutates a parsed
/// `rmpv::Value` re-emits MsgPack wire bytes with a single native encode, no
/// intermediate `serde_json::Value`, no re-parse.
///
/// # Errors
///
/// Returns [`CodecError::Encode`] if the encoder fails to write the value. For
/// an in-memory `Vec` writer this is effectively unreachable, but the encoder
/// is fallible so the error is surfaced rather than unwrapped.
pub fn to_msgpack_bytes(value: &rmpv::Value) -> Result<Bytes, CodecError> {
    // write_value writes into any `io::Write`; a Vec<u8> is one and never
    // returns a short write, so the only failure path is the encoder's own.
    let mut buf: Vec<u8> = Vec::new();
    rmpv::encode::write_value(&mut buf, value)?;
    Ok(Bytes::from(buf))
}

impl ParsedPayload {
    /// Whether the payload was decoded from JSON.
    #[must_use]
    pub fn is_json(&self) -> bool {
        matches!(self, Self::Json(_))
    }

    /// Whether the payload was decoded from MsgPack.
    #[must_use]
    pub fn is_msgpack(&self) -> bool {
        matches!(self, Self::MsgPack(_))
    }

    /// Read a top-level string field, format-agnostic.
    ///
    /// The common routing case: a router keys off one string field and does not
    /// care about wire format. Returns `None` if the value is not a top-level
    /// object, the key is absent, or the field is not a string. Borrows from the
    /// parsed value (zero-copy).
    ///
    /// Top-level lookup only -- see the module docs.
    #[must_use]
    pub fn field_str(&self, name: &str) -> Option<&str> {
        match self {
            Self::Json(v) => v.get(name).and_then(|f| f.as_str()),
            Self::MsgPack(v) => msgpack_field(v, name).and_then(rmpv::Value::as_str),
        }
    }

    /// Read a top-level field as a format-agnostic [`FieldRef`].
    ///
    /// Returns `None` only when the value is not a top-level object or the key
    /// is absent. A present-but-non-scalar field yields [`FieldRef::Other`]
    /// (routers never key off containers). Borrows from the parsed value.
    ///
    /// Top-level lookup only -- see the module docs.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<FieldRef<'_>> {
        match self {
            Self::Json(v) => v.get(name).map(json_field_ref),
            Self::MsgPack(v) => msgpack_field(v, name).map(msgpack_field_ref),
        }
    }

    /// Serialise back to the payload's OWN wire format (Task 0.3b).
    ///
    /// `Json` -> JSON bytes (via [`to_json_bytes`]), `MsgPack` -> MsgPack bytes
    /// (via [`to_msgpack_bytes`]). Same format in, same format out -- no
    /// cross-format conversion, no bridge.
    ///
    /// ## Pass-through contract -- DO NOT round-trip untouched records
    ///
    /// This is the egress face of a *parse-on-demand* spine. The governing
    /// principle is "serde is the enemy / zero re-representation": a record that
    /// a transform did NOT change must re-use its original `Record.payload`
    /// (the `Bytes` it arrived as) directly on egress. `to_bytes` is ONLY for a
    /// record a transform actually mutated.
    ///
    /// Calling `to_bytes` on an unmodified record is a correctness *and*
    /// performance bug: it pays a full parse + re-serialise for nothing AND can
    /// alter the wire bytes (key order, number formatting, whitespace) even
    /// though the logical value is identical. Reuse the original `Bytes`; only
    /// reach for `to_bytes` once the value has been edited.
    ///
    /// There is deliberately NO `to_bytes_as` cross-format egress. JSON and
    /// MsgPack have distinct value models (`sonic_rs::Value` vs `rmpv::Value`)
    /// with no native conversion between them; bridging would mean either a
    /// hand-rolled recursive value walker or a `serde_json` hop -- the exact
    /// double-representation this spine exists to avoid. Cross-format egress, if
    /// a consumer ever needs it, is a separate, explicit concern (YAGNI).
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::Json`] (JSON serialise) or [`CodecError::Encode`]
    /// (MsgPack serialise) on encoder failure.
    pub fn to_bytes(&self) -> Result<Bytes, CodecError> {
        match self {
            Self::Json(v) => to_json_bytes(v),
            Self::MsgPack(v) => to_msgpack_bytes(v),
        }
    }
}

/// Classify a sonic_rs JSON value into a [`FieldRef`] (borrows from `v`).
///
/// Order matters: probe the scalar accessors in turn. `as_i64` is tried before
/// `as_f64` so integers stay [`FieldRef::Int`]; a JSON number with a fractional
/// part falls through to [`FieldRef::Float`].
fn json_field_ref(v: &sonic_rs::Value) -> FieldRef<'_> {
    if let Some(s) = v.as_str() {
        FieldRef::Str(s)
    } else if v.is_null() {
        FieldRef::Null
    } else if let Some(b) = v.as_bool() {
        FieldRef::Bool(b)
    } else if let Some(i) = v.as_i64() {
        FieldRef::Int(i)
    } else if let Some(f) = v.as_f64() {
        FieldRef::Float(f)
    } else {
        FieldRef::Other
    }
}

/// Find a top-level value for `name` in an rmpv MsgPack value.
///
/// Only a [`rmpv::Value::Map`] has named fields. The map is a `Vec<(Value,
/// Value)>`, so this is a linear scan -- routing maps are small (a handful of
/// keys), so a linear scan beats building an index. Only string keys match.
fn msgpack_field<'a>(v: &'a rmpv::Value, name: &str) -> Option<&'a rmpv::Value> {
    match v {
        rmpv::Value::Map(pairs) => pairs
            .iter()
            .find(|(k, _)| k.as_str() == Some(name))
            .map(|(_, val)| val),
        _ => None,
    }
}

/// Classify an rmpv MsgPack value into a [`FieldRef`].
///
/// MsgPack integers split into signed/unsigned at the wire level; both fold to
/// `i64` here when they fit. An unsigned value above `i64::MAX` cannot fit `i64`
/// and is surfaced as [`FieldRef::Float`] via `as_f64` (lossy but it keeps a
/// numeric field numeric for routing) rather than dropped to `Other`.
fn msgpack_field_ref(v: &rmpv::Value) -> FieldRef<'_> {
    match v {
        rmpv::Value::String(s) => s.as_str().map_or(FieldRef::Other, FieldRef::Str),
        rmpv::Value::Nil => FieldRef::Null,
        rmpv::Value::Boolean(b) => FieldRef::Bool(*b),
        rmpv::Value::Integer(_) => v
            .as_i64()
            .map(FieldRef::Int)
            .or_else(|| v.as_f64().map(FieldRef::Float))
            .unwrap_or(FieldRef::Other),
        rmpv::Value::F32(f) => FieldRef::Float(f64::from(*f)),
        rmpv::Value::F64(f) => FieldRef::Float(*f),
        // Map / Array / Binary / Ext: routers do not key off containers.
        _ => FieldRef::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Helpers: build real MsgPack blobs by hand (no serde encode) -------
    //
    // We hand-roll the MsgPack bytes so the test exercises the NATIVE rmpv
    // decoder against the real wire format, not a serde round-trip.

    /// fixstr: 0xa0 | len, then the UTF-8 bytes (len < 32).
    fn fixstr(s: &str) -> Vec<u8> {
        let bytes = s.as_bytes();
        assert!(bytes.len() < 32, "fixstr helper only handles len < 32");
        let len = u8::try_from(bytes.len()).expect("len < 32 fits u8");
        let mut out = vec![0xa0 | len];
        out.extend_from_slice(bytes);
        out
    }

    /// fixmap header: 0x80 | n (n < 16 entries).
    fn fixmap_header(n: u8) -> u8 {
        assert!(n < 16, "fixmap helper only handles < 16 entries");
        0x80 | n
    }

    /// Build a logical record `{"_table": "events", "org_id": 42, "live":
    /// true, "ratio": <f64>, "missing": nil}` as a MsgPack fixmap.
    fn sample_msgpack() -> Bytes {
        let mut buf = vec![fixmap_header(5)];
        // "_table": "events"
        buf.extend(fixstr("_table"));
        buf.extend(fixstr("events"));
        // "org_id": 42  (positive fixint -- the byte is its own value)
        buf.extend(fixstr("org_id"));
        buf.push(42);
        // "live": true (0xc3)
        buf.extend(fixstr("live"));
        buf.push(0xc3);
        // "ratio": 1.5 (float64 0xcb + 8 bytes big-endian)
        buf.extend(fixstr("ratio"));
        buf.push(0xcb);
        buf.extend_from_slice(&1.5f64.to_be_bytes());
        // "missing": nil (0xc0)
        buf.extend(fixstr("missing"));
        buf.push(0xc0);
        Bytes::from(buf)
    }

    /// The same logical record as JSON.
    fn sample_json() -> Bytes {
        Bytes::from_static(
            br#"{"_table":"events","org_id":42,"live":true,"ratio":1.5,"missing":null}"#,
        )
    }

    // ---- parse(): JSON -----------------------------------------------------

    #[test]
    fn parse_json_object() {
        let parsed = parse(&sample_json(), PayloadFormat::Json).unwrap();
        assert!(parsed.is_json());
        assert!(!parsed.is_msgpack());
        assert_eq!(parsed.field_str("_table"), Some("events"));
    }

    #[test]
    fn parse_json_array_is_ok() {
        // A top-level array is valid JSON; it simply has no named fields.
        let parsed = parse(&Bytes::from_static(b"[1,2,3]"), PayloadFormat::Json).unwrap();
        assert!(parsed.is_json());
        assert_eq!(parsed.field_str("anything"), None);
    }

    // ---- parse(): MsgPack (native rmpv, hand-rolled bytes) -----------------

    #[test]
    fn parse_msgpack_map() {
        let parsed = parse(&sample_msgpack(), PayloadFormat::MsgPack).unwrap();
        assert!(parsed.is_msgpack());
        assert!(!parsed.is_json());
        assert_eq!(parsed.field_str("_table"), Some("events"));
    }

    #[test]
    fn parse_minimal_fixmap() {
        // {"k": "v"} -- the smallest interesting map.
        let mut buf = vec![fixmap_header(1)];
        buf.extend(fixstr("k"));
        buf.extend(fixstr("v"));
        let parsed = parse(&Bytes::from(buf), PayloadFormat::MsgPack).unwrap();
        assert_eq!(parsed.field_str("k"), Some("v"));
    }

    // ---- Auto detection dispatch ------------------------------------------

    #[test]
    fn parse_auto_dispatches_to_json() {
        let parsed = parse(&sample_json(), PayloadFormat::Auto).unwrap();
        assert!(parsed.is_json(), "object byte '{{' must detect as JSON");
        assert_eq!(parsed.field_str("_table"), Some("events"));
    }

    #[test]
    fn parse_auto_dispatches_to_msgpack() {
        let parsed = parse(&sample_msgpack(), PayloadFormat::Auto).unwrap();
        assert!(
            parsed.is_msgpack(),
            "fixmap byte 0x85 must detect as MsgPack"
        );
        assert_eq!(parsed.field_str("_table"), Some("events"));
    }

    // ---- Unified accessor: SAME field value from BOTH formats --------------

    #[test]
    fn field_str_identical_across_formats() {
        let j = parse(&sample_json(), PayloadFormat::Json).unwrap();
        let m = parse(&sample_msgpack(), PayloadFormat::MsgPack).unwrap();
        // The whole point: same logical record, same routing field, regardless
        // of wire format.
        assert_eq!(j.field_str("_table"), m.field_str("_table"));
        assert_eq!(j.field_str("_table"), Some("events"));
    }

    #[test]
    fn field_str_returns_none_for_non_string() {
        // org_id is an int -- field_str only returns strings.
        let j = parse(&sample_json(), PayloadFormat::Json).unwrap();
        let m = parse(&sample_msgpack(), PayloadFormat::MsgPack).unwrap();
        assert_eq!(j.field_str("org_id"), None);
        assert_eq!(m.field_str("org_id"), None);
    }

    #[test]
    fn field_str_returns_none_for_missing_key() {
        let j = parse(&sample_json(), PayloadFormat::Json).unwrap();
        let m = parse(&sample_msgpack(), PayloadFormat::MsgPack).unwrap();
        assert_eq!(j.field_str("nope"), None);
        assert_eq!(m.field_str("nope"), None);
    }

    #[test]
    fn field_str_value_is_present_via_field_too() {
        let j = parse(&sample_json(), PayloadFormat::Json).unwrap();
        assert_eq!(j.field("_table"), Some(FieldRef::Str("events")));
    }

    #[test]
    fn field_int_identical_across_formats() {
        let j = parse(&sample_json(), PayloadFormat::Json).unwrap();
        let m = parse(&sample_msgpack(), PayloadFormat::MsgPack).unwrap();
        assert_eq!(j.field("org_id"), Some(FieldRef::Int(42)));
        assert_eq!(m.field("org_id"), Some(FieldRef::Int(42)));
    }

    #[test]
    fn field_bool_identical_across_formats() {
        let j = parse(&sample_json(), PayloadFormat::Json).unwrap();
        let m = parse(&sample_msgpack(), PayloadFormat::MsgPack).unwrap();
        assert_eq!(j.field("live"), Some(FieldRef::Bool(true)));
        assert_eq!(m.field("live"), Some(FieldRef::Bool(true)));
    }

    #[test]
    fn field_float_identical_across_formats() {
        let j = parse(&sample_json(), PayloadFormat::Json).unwrap();
        let m = parse(&sample_msgpack(), PayloadFormat::MsgPack).unwrap();
        assert_eq!(j.field("ratio"), Some(FieldRef::Float(1.5)));
        assert_eq!(m.field("ratio"), Some(FieldRef::Float(1.5)));
    }

    #[test]
    fn field_null_identical_across_formats() {
        let j = parse(&sample_json(), PayloadFormat::Json).unwrap();
        let m = parse(&sample_msgpack(), PayloadFormat::MsgPack).unwrap();
        assert_eq!(j.field("missing"), Some(FieldRef::Null));
        assert_eq!(m.field("missing"), Some(FieldRef::Null));
    }

    #[test]
    fn field_missing_key_is_none_for_both() {
        let j = parse(&sample_json(), PayloadFormat::Json).unwrap();
        let m = parse(&sample_msgpack(), PayloadFormat::MsgPack).unwrap();
        assert_eq!(j.field("nope"), None);
        assert_eq!(m.field("nope"), None);
    }

    #[test]
    fn field_nested_object_is_other() {
        // A field whose value is a container collapses to Other, not None.
        let j = parse(
            &Bytes::from_static(br#"{"k":{"nested":1}}"#),
            PayloadFormat::Json,
        )
        .unwrap();
        assert_eq!(j.field("k"), Some(FieldRef::Other));
        // ...but it is not a routing scalar, so field_str is None.
        assert_eq!(j.field_str("k"), None);

        // MsgPack: {"k": [1]} -- an array value also collapses to Other.
        // fixmap(1) "k" -> fixarray(1) [positive fixint 1]
        let mut buf = vec![fixmap_header(1)];
        buf.extend(fixstr("k"));
        buf.push(0x91); // fixarray with 1 element
        buf.push(0x01); // positive fixint 1
        let m = parse(&Bytes::from(buf), PayloadFormat::MsgPack).unwrap();
        assert_eq!(m.field("k"), Some(FieldRef::Other));
    }

    #[test]
    fn field_on_non_object_top_level_is_none() {
        // A top-level JSON array has no named fields.
        let j = parse(&Bytes::from_static(b"[1,2,3]"), PayloadFormat::Json).unwrap();
        assert_eq!(j.field("0"), None);

        // A top-level MsgPack array (fixarray) likewise.
        // fixarray(2) [1, 2]
        let m = parse(&Bytes::from(vec![0x92, 0x01, 0x02]), PayloadFormat::MsgPack).unwrap();
        assert_eq!(m.field("0"), None);
    }

    // ---- Error cases -------------------------------------------------------

    #[test]
    fn malformed_json_errors() {
        let err = parse(&Bytes::from_static(b"{not valid json"), PayloadFormat::Json).unwrap_err();
        assert!(matches!(err, CodecError::Json(_)), "got {err:?}");
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn empty_blob_auto_errors_as_json() {
        // detect() maps empty -> Json; empty is not valid JSON.
        let err = parse(&Bytes::new(), PayloadFormat::Auto).unwrap_err();
        assert!(matches!(err, CodecError::Json(_)), "got {err:?}");
    }

    #[test]
    fn malformed_msgpack_errors() {
        // 0x81 declares a fixmap with one entry but supplies no key/value.
        let err = parse(&Bytes::from_static(&[0x81]), PayloadFormat::MsgPack).unwrap_err();
        assert!(matches!(err, CodecError::MsgPack(_)), "got {err:?}");
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn msgpack_truncated_float_errors() {
        // 0xcb declares a float64 but supplies only 3 of the 8 payload bytes.
        let mut buf = vec![fixmap_header(1)];
        buf.extend(fixstr("ratio"));
        buf.push(0xcb);
        buf.extend_from_slice(&[0x00, 0x01, 0x02]); // short
        let err = parse(&Bytes::from(buf), PayloadFormat::MsgPack).unwrap_err();
        assert!(matches!(err, CodecError::MsgPack(_)), "got {err:?}");
    }

    // ---- Task 0.3b: serialise-out (round-trips through real bytes) ---------
    //
    // The contract is "parse -> (mutate) -> serialise -> parse again preserves
    // the logical value". We assert on re-parsed VALUES (via the unified
    // accessor), NOT on raw bytes: re-serialise may reorder keys or reformat
    // numbers, so a byte-for-byte equality assertion would be wrong.

    /// Compare every routing field of the canonical sample across two payloads.
    fn assert_sample_fields_eq(a: &ParsedPayload, b: &ParsedPayload) {
        assert_eq!(a.field("_table"), b.field("_table"));
        assert_eq!(a.field("org_id"), b.field("org_id"));
        assert_eq!(a.field("live"), b.field("live"));
        assert_eq!(a.field("ratio"), b.field("ratio"));
        assert_eq!(a.field("missing"), b.field("missing"));
    }

    #[test]
    fn json_to_bytes_round_trips() {
        // parse JSON -> to_bytes -> parse again -> values equal.
        let original = parse(&sample_json(), PayloadFormat::Json).unwrap();
        assert!(original.is_json());

        let bytes = original.to_bytes().unwrap();
        assert!(!bytes.is_empty());

        let reparsed = parse(&bytes, PayloadFormat::Json).unwrap();
        assert!(reparsed.is_json(), "JSON must round-trip as JSON");
        assert_sample_fields_eq(&original, &reparsed);
    }

    #[test]
    fn msgpack_to_bytes_round_trips_via_native_bytes() {
        // parse MsgPack (hand-rolled bytes) -> to_bytes (native rmpv encode)
        // -> parse again -> values equal. No serde, no JSON bridge anywhere.
        let original = parse(&sample_msgpack(), PayloadFormat::MsgPack).unwrap();
        assert!(original.is_msgpack());

        let bytes = original.to_bytes().unwrap();
        assert!(!bytes.is_empty());
        // First byte must be a MsgPack map marker (fixmap 0x80..=0x8f for 5
        // entries -> 0x85), proving native MsgPack came out, not JSON.
        assert_eq!(bytes[0], fixmap_header(5), "expected fixmap(5) wire marker");

        let reparsed = parse(&bytes, PayloadFormat::MsgPack).unwrap();
        assert!(reparsed.is_msgpack(), "MsgPack must round-trip as MsgPack");
        assert_sample_fields_eq(&original, &reparsed);
    }

    #[test]
    fn to_json_bytes_reparses_to_same_value() {
        // Free function: serialise a sonic_rs::Value, re-parse, compare.
        let ParsedPayload::Json(value) = parse(&sample_json(), PayloadFormat::Json).unwrap() else {
            panic!("expected JSON");
        };
        let bytes = to_json_bytes(&value).unwrap();
        let reparsed = parse(&bytes, PayloadFormat::Json).unwrap();
        assert_eq!(reparsed.field("_table"), Some(FieldRef::Str("events")));
        assert_eq!(reparsed.field("org_id"), Some(FieldRef::Int(42)));
        assert_eq!(reparsed.field("ratio"), Some(FieldRef::Float(1.5)));
    }

    #[test]
    fn to_msgpack_bytes_reparses_to_same_value() {
        // Free function: serialise an rmpv::Value via native write_value,
        // re-parse, compare.
        let ParsedPayload::MsgPack(value) =
            parse(&sample_msgpack(), PayloadFormat::MsgPack).unwrap()
        else {
            panic!("expected MsgPack");
        };
        let bytes = to_msgpack_bytes(&value).unwrap();
        let reparsed = parse(&bytes, PayloadFormat::MsgPack).unwrap();
        assert_eq!(reparsed.field("_table"), Some(FieldRef::Str("events")));
        assert_eq!(reparsed.field("org_id"), Some(FieldRef::Int(42)));
        assert_eq!(reparsed.field("live"), Some(FieldRef::Bool(true)));
        assert_eq!(reparsed.field("ratio"), Some(FieldRef::Float(1.5)));
        assert_eq!(reparsed.field("missing"), Some(FieldRef::Null));
    }

    #[test]
    fn to_bytes_preserves_a_mutated_json_field() {
        // The realistic case: a transform CHANGED a field, then re-serialises.
        // Only then is to_bytes the right tool (unmodified records pass through
        // the original Bytes -- see the to_bytes doc).
        let ParsedPayload::Json(mut value) = parse(&sample_json(), PayloadFormat::Json).unwrap()
        else {
            panic!("expected JSON");
        };
        // Mutate _table in place via sonic_rs's object insert (overwrites the
        // existing key) -- the value model is what gets re-serialised.
        value.insert("_table", sonic_rs::Value::from("audit"));
        let bytes = to_json_bytes(&value).unwrap();
        let reparsed = parse(&bytes, PayloadFormat::Json).unwrap();
        assert_eq!(reparsed.field_str("_table"), Some("audit"));
        // Untouched siblings survive the round-trip.
        assert_eq!(reparsed.field("org_id"), Some(FieldRef::Int(42)));
    }

    #[test]
    fn json_to_bytes_handles_top_level_array() {
        // Egress is not object-only: a top-level array must round-trip too.
        let parsed = parse(&Bytes::from_static(b"[1,2,3]"), PayloadFormat::Json).unwrap();
        let bytes = parsed.to_bytes().unwrap();
        let reparsed = parse(&bytes, PayloadFormat::Json).unwrap();
        assert!(reparsed.is_json());
        // No named fields either way; the value re-parses without error.
        assert_eq!(reparsed.field_str("anything"), None);
    }

    #[test]
    fn msgpack_to_bytes_handles_top_level_scalar() {
        // A bare MsgPack integer (positive fixint 42) round-trips as itself,
        // not wrapped in a map.
        let parsed = parse(&Bytes::from(vec![42u8]), PayloadFormat::MsgPack).unwrap();
        let bytes = parsed.to_bytes().unwrap();
        assert_eq!(
            bytes.as_ref(),
            &[42u8],
            "fixint must re-emit byte-identical"
        );
        let reparsed = parse(&bytes, PayloadFormat::MsgPack).unwrap();
        assert!(reparsed.is_msgpack());
    }

    #[test]
    fn double_round_trip_is_stable() {
        // parse -> to_bytes -> parse -> to_bytes: the SECOND serialise must
        // equal the first (the value model is the fixed point, even if it
        // differs from the original hand-rolled bytes).
        let first = parse(&sample_msgpack(), PayloadFormat::MsgPack).unwrap();
        let b1 = first.to_bytes().unwrap();
        let second = parse(&b1, PayloadFormat::MsgPack).unwrap();
        let b2 = second.to_bytes().unwrap();
        assert_eq!(b1, b2, "re-serialising a re-parsed value must be stable");
    }

    #[test]
    fn json_parsed_as_msgpack_errors() {
        // Force the wrong decoder: JSON bytes through the MsgPack path. '{' is
        // 0x7b, which rmpv reads as a positive fixint -- a single value, not a
        // map -- so field lookups miss but parse itself may succeed. The robust
        // assertion is that it does NOT yield a usable _table field.
        let parsed = parse(&sample_json(), PayloadFormat::MsgPack);
        // Either it errors, or it decodes to a non-map with no _table field.
        match parsed {
            Err(CodecError::MsgPack(_)) => {}
            Ok(p) => assert_eq!(p.field_str("_table"), None),
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }
}
