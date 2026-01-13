// Project:   hs-rustlib
// File:      src/transport/detect.rs
// Purpose:   Stateful payload format detection with auto-locking
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! # Format Detection
//!
//! Stateful payload format detection that auto-locks to the first detected format.
//! Mismatched formats can be sent to DLQ, or the detector can auto-reset after
//! sustained mismatches.
//!
//! ## Modes
//!
//! - **Auto** (default): Detect format from first message, lock it
//! - **ForceJson**: Only accept JSON, reject MessagePack
//! - **ForceMessagePack**: Only accept MessagePack, reject JSON
//!
//! ## Example
//!
//! ```rust
//! use hs_rustlib::transport::{FormatDetector, FormatMode, DetectedFormat};
//!
//! let detector = FormatDetector::new();
//!
//! // First message sets the format
//! let result = detector.check_and_detect(br#"{"event": "login"}"#);
//! assert!(result.is_ok());
//! assert_eq!(detector.format(), DetectedFormat::Json);
//!
//! // Subsequent messages must match
//! let result = detector.check_and_detect(br#"{"event": "logout"}"#);
//! assert!(result.is_ok());
//!
//! // Mismatched format returns Err (send to DLQ)
//! let msgpack = [0x81, 0xa3, b'f', b'o', b'o'];
//! let result = detector.check_and_detect(&msgpack);
//! assert!(result.is_err());
//! ```

use std::sync::atomic::{AtomicU8, Ordering};

/// Detected payload format (for stateful detection).
///
/// Separate from `PayloadFormat` which includes `Auto` for config purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum DetectedFormat {
    /// Format not yet detected
    Unknown = 0,
    /// JSON format
    Json = 1,
    /// MessagePack format
    MessagePack = 2,
}

impl From<u8> for DetectedFormat {
    fn from(v: u8) -> Self {
        match v {
            1 => DetectedFormat::Json,
            2 => DetectedFormat::MessagePack,
            _ => DetectedFormat::Unknown,
        }
    }
}

/// Format detection mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormatMode {
    /// Auto-detect format from first message (default)
    #[default]
    Auto,
    /// Force JSON only - reject MessagePack
    ForceJson,
    /// Force MessagePack only - reject JSON
    ForceMessagePack,
}

impl FormatMode {
    /// Parse from string (for config).
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "auto" => Some(FormatMode::Auto),
            "json" => Some(FormatMode::ForceJson),
            "messagepack" | "msgpack" => Some(FormatMode::ForceMessagePack),
            _ => None,
        }
    }
}

/// Stateful format detector with auto-detection and locking.
///
/// Once a format is detected, the detector locks to that format.
/// Mismatches return `Err` (for DLQ routing). After sustained mismatches
/// (configurable threshold), the detector auto-resets in Auto mode.
pub struct FormatDetector {
    detected_format: AtomicU8,
    mismatch_count: AtomicU8,
    mode: FormatMode,
}

impl FormatDetector {
    /// Threshold of consecutive mismatches before considering format reset (Auto mode only)
    const MISMATCH_THRESHOLD: u8 = 10;

    /// Create a new detector in Auto mode.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            detected_format: AtomicU8::new(DetectedFormat::Unknown as u8),
            mismatch_count: AtomicU8::new(0),
            mode: FormatMode::Auto,
        }
    }

    /// Create a detector with a specific mode.
    #[must_use]
    pub fn with_mode(mode: FormatMode) -> Self {
        let initial_format = match mode {
            FormatMode::Auto => DetectedFormat::Unknown,
            FormatMode::ForceJson => DetectedFormat::Json,
            FormatMode::ForceMessagePack => DetectedFormat::MessagePack,
        };
        Self {
            detected_format: AtomicU8::new(initial_format as u8),
            mismatch_count: AtomicU8::new(0),
            mode,
        }
    }

    /// Get the current mode.
    #[must_use]
    pub fn mode(&self) -> FormatMode {
        self.mode
    }

    /// Get the currently detected format.
    #[must_use]
    pub fn format(&self) -> DetectedFormat {
        DetectedFormat::from(self.detected_format.load(Ordering::Relaxed))
    }

    /// Check if format matches expected, tracking mismatches.
    ///
    /// Returns `Ok(format)` if message should be processed, `Err(expected)` if it
    /// should go to DLQ (expected format returned for error context).
    #[inline]
    pub fn check_and_detect(&self, payload: &[u8]) -> Result<DetectedFormat, DetectedFormat> {
        let detected = detect_format_bytes(payload);

        // Handle forced modes - no auto-detection, no reset
        match self.mode {
            FormatMode::ForceJson => {
                return match detected {
                    Some(DetectedFormat::Json) => Ok(DetectedFormat::Json),
                    _ => Err(DetectedFormat::Json), // Expected JSON, got something else -> DLQ
                };
            }
            FormatMode::ForceMessagePack => {
                return match detected {
                    Some(DetectedFormat::MessagePack) => Ok(DetectedFormat::MessagePack),
                    _ => Err(DetectedFormat::MessagePack), // Expected MsgPack, got something else -> DLQ
                };
            }
            FormatMode::Auto => {} // Continue with auto-detection logic
        }

        // Auto mode logic
        let current = self.format();

        match (current, detected) {
            // First message - set the format
            (DetectedFormat::Unknown, Some(fmt)) => {
                self.detected_format.store(fmt as u8, Ordering::Relaxed);
                self.mismatch_count.store(0, Ordering::Relaxed);
                Ok(fmt)
            }

            // Unknown format in payload - DLQ
            (_, None) => Err(DetectedFormat::Unknown),

            // Format matches - process
            (expected, Some(actual)) if expected == actual => {
                self.mismatch_count.store(0, Ordering::Relaxed);
                Ok(actual)
            }

            // Format mismatch - check if we should reset
            (expected, Some(actual)) => {
                let count = self.mismatch_count.fetch_add(1, Ordering::Relaxed);
                if count >= Self::MISMATCH_THRESHOLD {
                    // Too many mismatches - assume format changed, reset
                    self.detected_format.store(actual as u8, Ordering::Relaxed);
                    self.mismatch_count.store(0, Ordering::Relaxed);
                    #[cfg(feature = "logger")]
                    tracing::warn!(
                        old = ?expected,
                        new = ?actual,
                        "Format changed after {} mismatches, resetting",
                        count
                    );
                    Ok(actual)
                } else {
                    // Mismatch - send to DLQ
                    Err(expected)
                }
            }
        }
    }

    /// Force reset to unknown (for testing or manual override, Auto mode only).
    pub fn reset(&self) {
        if self.mode == FormatMode::Auto {
            self.detected_format
                .store(DetectedFormat::Unknown as u8, Ordering::Relaxed);
            self.mismatch_count.store(0, Ordering::Relaxed);
        }
    }
}

impl Default for FormatDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Detect payload format from raw bytes (internal).
///
/// Optimized for the common case where JSON starts with '{' at position 0.
#[inline]
fn detect_format_bytes(payload: &[u8]) -> Option<DetectedFormat> {
    // Fast path: check first byte directly (common case - no leading whitespace)
    let first_byte = *payload.first()?;

    // Most common case: JSON object starting with '{'
    if first_byte == b'{' || first_byte == b'[' {
        return Some(DetectedFormat::Json);
    }

    // Check for MessagePack before considering whitespace
    // (MessagePack never starts with whitespace-like bytes)
    match first_byte {
        // MessagePack fixmap (0x80-0x8F)
        0x80..=0x8F => return Some(DetectedFormat::MessagePack),
        // MessagePack map16 (0xDE) or map32 (0xDF)
        0xDE | 0xDF => return Some(DetectedFormat::MessagePack),
        // MessagePack fixarray (0x90-0x9F)
        0x90..=0x9F => return Some(DetectedFormat::MessagePack),
        // MessagePack array16 (0xDC) or array32 (0xDD)
        0xDC | 0xDD => return Some(DetectedFormat::MessagePack),
        _ => {}
    }

    // Slow path: skip leading whitespace for JSON (rare case)
    if first_byte.is_ascii_whitespace() {
        for &b in payload.iter().skip(1) {
            if !b.is_ascii_whitespace() {
                return match b {
                    b'{' | b'[' => Some(DetectedFormat::Json),
                    _ => None,
                };
            }
        }
        return None; // All whitespace
    }

    None
}

/// Stateless format detection (convenience function).
///
/// For stateful detection with locking, use `FormatDetector`.
#[inline]
#[must_use]
pub fn detect_format(payload: &[u8]) -> Option<DetectedFormat> {
    detect_format_bytes(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_json_object() {
        assert_eq!(detect_format(b"{\"key\": \"value\"}"), Some(DetectedFormat::Json));
    }

    #[test]
    fn test_detect_json_array() {
        assert_eq!(detect_format(b"[1, 2, 3]"), Some(DetectedFormat::Json));
    }

    #[test]
    fn test_detect_json_with_whitespace() {
        assert_eq!(
            detect_format(b"  \n\t{\"key\": 1}"),
            Some(DetectedFormat::Json)
        );
    }

    #[test]
    fn test_detect_msgpack_fixmap() {
        assert_eq!(
            detect_format(&[0x81, 0xA3, b'k', b'e', b'y']),
            Some(DetectedFormat::MessagePack)
        );
    }

    #[test]
    fn test_detect_msgpack_map16() {
        assert_eq!(
            detect_format(&[0xDE, 0x00, 0x01]),
            Some(DetectedFormat::MessagePack)
        );
    }

    #[test]
    fn test_detect_empty() {
        assert_eq!(detect_format(b""), None);
    }

    #[test]
    fn test_detect_whitespace_only() {
        assert_eq!(detect_format(b"   \n\t  "), None);
    }

    #[test]
    fn test_detect_unknown() {
        assert_eq!(detect_format(b"hello"), None);
    }

    #[test]
    fn test_format_detector_auto_detect() {
        let detector = FormatDetector::new();
        assert_eq!(detector.format(), DetectedFormat::Unknown);

        // First JSON message sets format
        let result = detector.check_and_detect(b"{\"key\": 1}");
        assert_eq!(result, Ok(DetectedFormat::Json));
        assert_eq!(detector.format(), DetectedFormat::Json);

        // Subsequent JSON messages pass
        assert_eq!(
            detector.check_and_detect(b"{\"key\": 2}"),
            Ok(DetectedFormat::Json)
        );

        // MessagePack mismatch goes to DLQ
        assert_eq!(
            detector.check_and_detect(&[0x81, 0xA1, b'k']),
            Err(DetectedFormat::Json)
        );
    }

    #[test]
    fn test_format_detector_mismatch_reset() {
        let detector = FormatDetector::new();

        // Set to JSON
        detector.check_and_detect(b"{\"key\": 1}").unwrap();

        // Send 11 MessagePack messages (> threshold of 10)
        for _ in 0..11 {
            let _ = detector.check_and_detect(&[0x81, 0xA1, b'k']);
        }

        // Format should have switched to MessagePack
        assert_eq!(detector.format(), DetectedFormat::MessagePack);
    }

    #[test]
    fn test_force_json_mode() {
        let detector = FormatDetector::with_mode(FormatMode::ForceJson);
        assert_eq!(detector.mode(), FormatMode::ForceJson);
        assert_eq!(detector.format(), DetectedFormat::Json);

        // JSON passes
        assert_eq!(
            detector.check_and_detect(b"{\"key\": 1}"),
            Ok(DetectedFormat::Json)
        );

        // MessagePack fails immediately (no mismatch counting)
        assert_eq!(
            detector.check_and_detect(&[0x81, 0xA1, b'k']),
            Err(DetectedFormat::Json)
        );

        // Unknown format also fails
        assert_eq!(
            detector.check_and_detect(b"hello"),
            Err(DetectedFormat::Json)
        );

        // Format stays locked
        assert_eq!(detector.format(), DetectedFormat::Json);
    }

    #[test]
    fn test_force_msgpack_mode() {
        let detector = FormatDetector::with_mode(FormatMode::ForceMessagePack);
        assert_eq!(detector.mode(), FormatMode::ForceMessagePack);
        assert_eq!(detector.format(), DetectedFormat::MessagePack);

        // MessagePack passes
        assert_eq!(
            detector.check_and_detect(&[0x81, 0xA1, b'k']),
            Ok(DetectedFormat::MessagePack)
        );

        // JSON fails immediately
        assert_eq!(
            detector.check_and_detect(b"{\"key\": 1}"),
            Err(DetectedFormat::MessagePack)
        );

        // Format stays locked
        assert_eq!(detector.format(), DetectedFormat::MessagePack);
    }

    #[test]
    fn test_force_mode_no_reset() {
        let detector = FormatDetector::with_mode(FormatMode::ForceJson);

        // Send many MessagePack messages - should NOT reset
        for _ in 0..20 {
            let _ = detector.check_and_detect(&[0x81, 0xA1, b'k']);
        }

        // Format should still be JSON (no auto-reset in force mode)
        assert_eq!(detector.format(), DetectedFormat::Json);
    }

    #[test]
    fn test_format_mode_from_str() {
        assert_eq!(FormatMode::from_str("auto"), Some(FormatMode::Auto));
        assert_eq!(FormatMode::from_str("AUTO"), Some(FormatMode::Auto));
        assert_eq!(FormatMode::from_str("json"), Some(FormatMode::ForceJson));
        assert_eq!(FormatMode::from_str("JSON"), Some(FormatMode::ForceJson));
        assert_eq!(
            FormatMode::from_str("messagepack"),
            Some(FormatMode::ForceMessagePack)
        );
        assert_eq!(
            FormatMode::from_str("msgpack"),
            Some(FormatMode::ForceMessagePack)
        );
        assert_eq!(FormatMode::from_str("invalid"), None);
    }
}
