// Project:   hyperi-rustlib
// File:      src/transport/propagation.rs
// Purpose:   W3C Trace Context propagation helpers for transport layer
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # Trace Context Propagation
//!
//! W3C Trace Context (traceparent) helpers for automatic context propagation
//! across transport boundaries. When the `otel` feature is enabled, transports
//! inject/extract `traceparent` headers transparently.
//!
//! Format: `00-{trace_id}-{span_id}-{flags}`
//! Example: `00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01`

/// W3C traceparent header name.
pub const TRACEPARENT_HEADER: &str = "traceparent";

/// Format a W3C traceparent header value from the current OTel span context.
///
/// Returns `Some("00-{trace_id}-{span_id}-{flags}")` if there is a valid
/// span context active, `None` otherwise.
#[cfg(feature = "otel")]
#[must_use]
pub fn current_traceparent() -> Option<String> {
    use opentelemetry::trace::TraceContextExt;

    let cx = opentelemetry::Context::current();
    let span = cx.span();
    let sc = span.span_context();

    if sc.is_valid() {
        Some(format_traceparent(sc))
    } else {
        None
    }
}

/// Format a `SpanContext` into a W3C traceparent string.
///
/// `TraceId` and `SpanId` implement `Display` as lowercase hex.
/// `TraceFlags::to_u8()` returns the raw flags byte.
#[cfg(feature = "otel")]
fn format_traceparent(sc: &opentelemetry::trace::SpanContext) -> String {
    format!(
        "00-{}-{}-{:02x}",
        sc.trace_id(),
        sc.span_id(),
        sc.trace_flags().to_u8()
    )
}

/// Format a traceparent string from raw components (for testing without OTel).
#[must_use]
pub fn format_traceparent_raw(trace_id: u128, span_id: u64, flags: u8) -> String {
    format!("00-{trace_id:032x}-{span_id:016x}-{flags:02x}")
}

/// Validate that a string looks like a well-formed traceparent header.
///
/// Does basic structural validation (length, separators, hex chars).
/// Does NOT validate that trace_id/span_id are non-zero.
#[must_use]
pub fn is_valid_traceparent(value: &str) -> bool {
    // Expected: "00-<32hex>-<16hex>-<2hex>" = 55 chars
    if value.len() != 55 {
        return false;
    }

    let bytes = value.as_bytes();

    // Version: "00"
    if bytes[0] != b'0' || bytes[1] != b'0' {
        return false;
    }

    // Separators at positions 2, 35, 52
    if bytes[2] != b'-' || bytes[35] != b'-' || bytes[52] != b'-' {
        return false;
    }

    // All other positions must be hex digits
    let hex_ranges = [3..35, 36..52, 53..55];
    for range in &hex_ranges {
        for &b in &bytes[range.clone()] {
            if !b.is_ascii_hexdigit() {
                return false;
            }
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traceparent_format_raw() {
        let tp = format_traceparent_raw(
            0x4bf9_2f35_77b3_4da6_a3ce_929d_0e0e_4736,
            0x00f0_67aa_0ba9_02b7,
            0x01,
        );
        assert_eq!(
            tp,
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        );
        assert_eq!(tp.len(), 55);
    }

    #[test]
    fn traceparent_format_zero_padded() {
        // Low values should be zero-padded to full width
        let tp = format_traceparent_raw(123, 456, 1);
        assert!(tp.starts_with("00-"));
        assert_eq!(tp.len(), 55);
        assert_eq!(
            tp,
            "00-0000000000000000000000000000007b-00000000000001c8-01"
        );
    }

    #[test]
    fn traceparent_format_flags_zero() {
        let tp = format_traceparent_raw(1, 1, 0);
        assert!(tp.ends_with("-00"));
    }

    #[test]
    fn valid_traceparent() {
        assert!(is_valid_traceparent(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        ));
    }

    #[test]
    fn invalid_traceparent_too_short() {
        assert!(!is_valid_traceparent("00-abc-def-01"));
    }

    #[test]
    fn invalid_traceparent_bad_version() {
        assert!(!is_valid_traceparent(
            "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        ));
    }

    #[test]
    fn invalid_traceparent_non_hex() {
        assert!(!is_valid_traceparent(
            "00-4bf92f3577b34da6a3ce929d0e0eXXXX-00f067aa0ba902b7-01"
        ));
    }

    #[test]
    fn invalid_traceparent_wrong_separators() {
        assert!(!is_valid_traceparent(
            "00_4bf92f3577b34da6a3ce929d0e0e4736_00f067aa0ba902b7_01"
        ));
    }
}
