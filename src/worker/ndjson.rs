// Project:   hyperi-rustlib
// File:      src/worker/ndjson.rs
// Purpose:   NDJSON batch splitting and parallel parsing utilities
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! NDJSON (newline-delimited JSON) batch processing utilities.
//!
//! Splits NDJSON byte payloads into individual lines and optionally parses
//! them in parallel via [`AdaptiveWorkerPool::process_batch`](crate::worker::AdaptiveWorkerPool::process_batch).
//!
//! Does NOT depend on a specific JSON parser — the parse function is a closure.
//! Use with `sonic-rs`, `serde_json`, or any other parser.
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::worker::ndjson;
//!
//! let payload = b"{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n";
//! let lines = ndjson::split_lines(payload);
//! assert_eq!(lines.len(), 3);
//!
//! // Parallel parse (with worker pool)
//! let parsed = pool.process_batch(&lines, |line| {
//!     sonic_rs::from_slice::<Value>(line).map_err(|e| e.to_string())
//! });
//! ```

/// Split an NDJSON payload into individual line slices.
///
/// Handles `\n`, `\r\n`, trailing newlines, and blank lines (skipped).
/// Zero-copy — returns slices into the original payload.
#[must_use]
pub fn split_lines(payload: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;

    for (i, &byte) in payload.iter().enumerate() {
        if byte == b'\n' {
            let mut end = i;
            // Handle \r\n
            if end > start && payload[end - 1] == b'\r' {
                end -= 1;
            }
            if end > start {
                lines.push(&payload[start..end]);
            }
            start = i + 1;
        }
    }

    // Handle last line without trailing newline
    if start < payload.len() {
        let end = if payload[payload.len() - 1] == b'\r' {
            payload.len() - 1
        } else {
            payload.len()
        };
        if end > start {
            lines.push(&payload[start..end]);
        }
    }

    lines
}

/// Count the number of NDJSON lines in a payload without allocating.
///
/// Useful for pre-sizing buffers before splitting.
#[must_use]
pub fn count_lines(payload: &[u8]) -> usize {
    if payload.is_empty() {
        return 0;
    }

    // Count newline bytes — bytecount crate would be marginally faster but is
    // not worth a dependency for a non-hot-path utility function.
    #[allow(clippy::naive_bytecount)]
    let newlines = payload.iter().filter(|&&b| b == b'\n').count();
    // If the payload doesn't end with \n, there's one more line
    let trailing = usize::from(payload.last() != Some(&b'\n'));
    newlines + trailing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_simple() {
        let payload = b"{\"a\":1}\n{\"b\":2}\n{\"c\":3}\n";
        let lines = split_lines(payload);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], b"{\"a\":1}");
        assert_eq!(lines[1], b"{\"b\":2}");
        assert_eq!(lines[2], b"{\"c\":3}");
    }

    #[test]
    fn test_split_no_trailing_newline() {
        let payload = b"{\"a\":1}\n{\"b\":2}";
        let lines = split_lines(payload);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], b"{\"a\":1}");
        assert_eq!(lines[1], b"{\"b\":2}");
    }

    #[test]
    fn test_split_single_line() {
        let payload = b"{\"x\":42}";
        let lines = split_lines(payload);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], b"{\"x\":42}");
    }

    #[test]
    fn test_split_empty() {
        let lines = split_lines(b"");
        assert!(lines.is_empty());
    }

    #[test]
    fn test_split_blank_lines_skipped() {
        let payload = b"{\"a\":1}\n\n{\"b\":2}\n\n";
        let lines = split_lines(payload);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_split_crlf() {
        let payload = b"{\"a\":1}\r\n{\"b\":2}\r\n";
        let lines = split_lines(payload);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], b"{\"a\":1}");
        assert_eq!(lines[1], b"{\"b\":2}");
    }

    #[test]
    fn test_split_large_payload() {
        let mut payload = Vec::new();
        for i in 0..1000 {
            payload.extend_from_slice(format!("{{\"id\":{i}}}\n").as_bytes());
        }
        let lines = split_lines(&payload);
        assert_eq!(lines.len(), 1000);
    }

    #[test]
    fn test_count_lines_simple() {
        assert_eq!(count_lines(b"{}\n{}\n{}\n"), 3);
    }

    #[test]
    fn test_count_lines_no_trailing() {
        assert_eq!(count_lines(b"{}\n{}"), 2);
    }

    #[test]
    fn test_count_lines_empty() {
        assert_eq!(count_lines(b""), 0);
    }

    #[test]
    fn test_count_lines_single() {
        assert_eq!(count_lines(b"{}"), 1);
    }
}
