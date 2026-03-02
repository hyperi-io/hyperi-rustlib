// Project:   hyperi-rustlib
// File:      tests/logger_output.rs
// Purpose:   Integration tests for logger output capturing and masking
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Tests that verify actual log output content for both JSON and text formats,
//! including sensitive data masking via the `MaskingWriter`.

use std::collections::HashSet;
use std::io;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::EnvFilter;

use hyperi_rustlib::logger::MaskingWriter;

// ---------------------------------------------------------------------------
// Test writer infrastructure
// ---------------------------------------------------------------------------

/// Shared buffer that survives writer drop (for capturing log output).
#[derive(Clone)]
struct TestBuf(Arc<Mutex<Vec<u8>>>);

impl TestBuf {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    fn output(&self) -> String {
        let guard = self.0.lock().unwrap();
        String::from_utf8_lossy(&guard).to_string()
    }
}

/// Writer handle that writes to the shared buffer.
struct BufWriter(Arc<Mutex<Vec<u8>>>);

impl io::Write for BufWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// JSON format tests
// ---------------------------------------------------------------------------

#[test]
fn test_json_format_output() {
    let buf = TestBuf::new();
    let shared = buf.0.clone();
    let writer = move || BufWriter(Arc::clone(&shared));

    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new("info"))
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_timer(UtcTime::rfc_3339())
                .with_file(false)
                .with_line_number(false)
                .with_target(true)
                .with_writer(writer),
        );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(user_id = 123, "User logged in");
    });

    let output = buf.output();
    assert!(output.contains("\"level\""), "should contain level field");
    assert!(output.contains("\"INFO\""), "level should be INFO");
    assert!(
        output.contains("User logged in"),
        "should contain the message"
    );
    assert!(
        output.contains("\"user_id\":123"),
        "should contain custom field"
    );
    assert!(
        output.contains("\"timestamp\""),
        "should contain timestamp field"
    );
}

#[test]
fn test_json_timestamp_is_rfc3339() {
    let buf = TestBuf::new();
    let shared = buf.0.clone();
    let writer = move || BufWriter(Arc::clone(&shared));

    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new("info"))
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_timer(UtcTime::rfc_3339())
                .with_file(false)
                .with_line_number(false)
                .with_writer(writer),
        );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("timestamp check");
    });

    let output = buf.output();
    let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    let ts = parsed["timestamp"].as_str().unwrap();

    // RFC 3339 timestamps end with Z or contain a +/- offset
    assert!(
        ts.ends_with('Z') || ts.contains('+') || ts.contains('-'),
        "timestamp should be RFC 3339: {ts}"
    );
    assert!(
        ts.contains('T'),
        "timestamp should contain T separator: {ts}"
    );
}

// ---------------------------------------------------------------------------
// Text format tests
// ---------------------------------------------------------------------------

#[test]
fn test_text_format_output() {
    let buf = TestBuf::new();
    let shared = buf.0.clone();
    let writer = move || BufWriter(Arc::clone(&shared));

    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new("info"))
        .with(
            tracing_subscriber::fmt::layer()
                .with_timer(UtcTime::rfc_3339())
                .with_file(false)
                .with_line_number(false)
                .with_target(true)
                .with_ansi(false)
                .with_writer(writer),
        );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(host = "db.example.com", "Connected to database");
    });

    let output = buf.output();
    assert!(output.contains("INFO"), "should contain level");
    assert!(
        output.contains("Connected to database"),
        "should contain message"
    );
    assert!(
        output.contains("db.example.com"),
        "should contain field value"
    );
}

// ---------------------------------------------------------------------------
// Level filtering tests
// ---------------------------------------------------------------------------

#[test]
fn test_log_level_filtering() {
    let buf = TestBuf::new();
    let shared = buf.0.clone();
    let writer = move || BufWriter(Arc::clone(&shared));

    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new("warn"))
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_timer(UtcTime::rfc_3339())
                .with_file(false)
                .with_line_number(false)
                .with_ansi(false)
                .with_writer(writer),
        );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("should be filtered");
        tracing::warn!("should appear");
    });

    let output = buf.output();
    assert!(
        !output.contains("should be filtered"),
        "info should be filtered at warn level"
    );
    assert!(output.contains("should appear"), "warn should pass through");
}

// ---------------------------------------------------------------------------
// Masking integration tests (with MaskingWriter)
// ---------------------------------------------------------------------------

#[test]
fn test_json_masking_redacts_sensitive_fields() {
    let buf = TestBuf::new();
    let sensitive: HashSet<String> = ["password".to_string(), "token".to_string()]
        .into_iter()
        .collect();
    let fields = Arc::new(sensitive);
    let shared = buf.0.clone();

    let make_writer = move || {
        let inner = BufWriter(Arc::clone(&shared));
        MaskingWriter::new(inner, Arc::clone(&fields), true)
    };

    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new("info"))
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_timer(UtcTime::rfc_3339())
                .with_file(false)
                .with_line_number(false)
                .with_writer(make_writer),
        );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            password = "super_secret_123",
            token = "tok_abc",
            username = "john",
            "Login attempt"
        );
    });

    let output = buf.output();
    assert!(
        !output.contains("super_secret_123"),
        "password value should be redacted"
    );
    assert!(
        !output.contains("tok_abc"),
        "token value should be redacted"
    );
    assert!(
        output.contains("[REDACTED]"),
        "should contain redacted placeholder"
    );
    assert!(
        output.contains("john"),
        "non-sensitive field should be preserved"
    );
    assert!(
        output.contains("Login attempt"),
        "message should be preserved"
    );
}

#[test]
fn test_text_masking_redacts_sensitive_fields() {
    let buf = TestBuf::new();
    let sensitive: HashSet<String> = ["password".to_string()].into_iter().collect();
    let fields = Arc::new(sensitive);
    let shared = buf.0.clone();

    let make_writer = move || {
        let inner = BufWriter(Arc::clone(&shared));
        MaskingWriter::new(inner, Arc::clone(&fields), false)
    };

    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new("info"))
        .with(
            tracing_subscriber::fmt::layer()
                .with_timer(UtcTime::rfc_3339())
                .with_file(false)
                .with_line_number(false)
                .with_ansi(false)
                .with_writer(make_writer),
        );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(password = "my_secret", host = "localhost", "DB connect");
    });

    let output = buf.output();
    assert!(
        !output.contains("my_secret"),
        "password value should be redacted"
    );
    assert!(
        output.contains("[REDACTED]"),
        "should contain redacted placeholder"
    );
    assert!(
        output.contains("localhost"),
        "non-sensitive field should be preserved"
    );
}

#[test]
fn test_masking_preserves_all_normal_fields() {
    let buf = TestBuf::new();
    let sensitive: HashSet<String> = ["password".to_string()].into_iter().collect();
    let fields = Arc::new(sensitive);
    let shared = buf.0.clone();

    let make_writer = move || {
        let inner = BufWriter(Arc::clone(&shared));
        MaskingWriter::new(inner, Arc::clone(&fields), true)
    };

    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new("info"))
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_timer(UtcTime::rfc_3339())
                .with_file(false)
                .with_line_number(false)
                .with_writer(make_writer),
        );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(
            host = "db.example.com",
            port = 5432,
            username = "admin",
            "Connection established"
        );
    });

    let output = buf.output();
    let parsed: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(parsed["fields"]["host"], "db.example.com");
    assert_eq!(parsed["fields"]["port"], 5432);
    assert_eq!(parsed["fields"]["username"], "admin");
}

// ---------------------------------------------------------------------------
// Coloured formatter tests
// ---------------------------------------------------------------------------

#[test]
fn test_coloured_output_contains_ansi_escapes() {
    use hyperi_rustlib::logger::format::ColouredFormatter;

    let buf = TestBuf::new();
    let shared = buf.0.clone();
    let writer = move || BufWriter(Arc::clone(&shared));

    let formatter = ColouredFormatter::new(true)
        .with_file(false)
        .with_line_number(false);

    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new("info"))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(true)
                .event_format(formatter)
                .with_writer(writer),
        );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("coloured output test");
    });

    let output = buf.output();
    // ANSI escape sequences start with ESC (0x1B) followed by [
    assert!(
        output.contains('\x1b'),
        "output should contain ANSI escape codes when ansi=true: {output}"
    );
    assert!(
        output.contains("coloured output test"),
        "should contain message"
    );
    assert!(output.contains("INFO"), "should contain level");
}

#[test]
fn test_no_colour_output_is_clean() {
    use hyperi_rustlib::logger::format::ColouredFormatter;

    let buf = TestBuf::new();
    let shared = buf.0.clone();
    let writer = move || BufWriter(Arc::clone(&shared));

    let formatter = ColouredFormatter::new(false)
        .with_file(false)
        .with_line_number(false);

    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new("info"))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .event_format(formatter)
                .with_writer(writer),
        );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("clean output test");
    });

    let output = buf.output();
    assert!(
        !output.contains('\x1b'),
        "output should NOT contain ANSI escape codes when ansi=false: {output}"
    );
    assert!(
        output.contains("clean output test"),
        "should contain message"
    );
    assert!(output.contains("INFO"), "should contain level");
}

#[test]
fn test_coloured_output_has_all_components() {
    use hyperi_rustlib::logger::format::ColouredFormatter;

    let buf = TestBuf::new();
    let shared = buf.0.clone();
    let writer = move || BufWriter(Arc::clone(&shared));

    let formatter = ColouredFormatter::new(false)
        .with_file(false)
        .with_line_number(false);

    let subscriber = tracing_subscriber::registry()
        .with(EnvFilter::new("info"))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .event_format(formatter)
                .with_writer(writer),
        );

    tracing::subscriber::with_default(subscriber, || {
        tracing::info!(count = 42, host = "localhost", "Server started");
    });

    let output = buf.output();
    // Verify all components present
    assert!(output.contains("INFO"), "should contain level");
    assert!(output.contains("Server started"), "should contain message");
    assert!(output.contains("count"), "should contain field name");
    assert!(output.contains("42"), "should contain field value");
    assert!(output.contains("localhost"), "should contain string field");
    // Should contain a timestamp-like prefix
    assert!(output.contains('T'), "should contain RFC 3339 timestamp");
}
