// Project:   hyperi-rustlib
// File:      src/logger/masking.rs
// Purpose:   Sensitive data masking for log output
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Sensitive data masking for tracing log output.
//!
//! Provides a [`MaskingWriter`] that intercepts formatted log output and
//! redacts sensitive field values before writing to the underlying destination.

use std::collections::HashSet;
use std::io;
use std::sync::Arc;

/// Redacted value placeholder.
pub const REDACTED: &str = "[REDACTED]";

/// Default list of sensitive field names to mask.
#[must_use]
pub fn default_sensitive_fields() -> Vec<String> {
    vec![
        // Passwords
        "password",
        "passwd",
        "pwd",
        "pass",
        // Tokens and keys
        "token",
        "secret",
        "api_key",
        "apikey",
        "api-key",
        "access_key",
        "secret_key",
        "private_key",
        "privatekey",
        // Auth
        "auth",
        "authorization",
        "bearer",
        "credential",
        "credentials",
        // OAuth
        "client_secret",
        "refresh_token",
        "access_token",
        // Other sensitive
        "ssn",
        "credit_card",
        "creditcard",
        "cvv",
        "pin",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Configuration for sensitive field detection.
///
/// Holds the set of field name patterns considered sensitive. Used both as a
/// standalone detector and as configuration for the masking writer factory.
#[derive(Debug, Clone)]
pub struct MaskingLayer {
    sensitive_fields: HashSet<String>,
}

impl MaskingLayer {
    /// Create a new masking layer with default sensitive fields.
    #[must_use]
    pub fn new() -> Self {
        Self::with_fields(default_sensitive_fields())
    }

    /// Create a masking layer with custom sensitive fields.
    #[must_use]
    pub fn with_fields(fields: Vec<String>) -> Self {
        Self {
            sensitive_fields: fields.into_iter().map(|s| s.to_lowercase()).collect(),
        }
    }

    /// Add additional sensitive fields.
    #[must_use]
    pub fn add_fields(mut self, fields: Vec<String>) -> Self {
        for field in fields {
            self.sensitive_fields.insert(field.to_lowercase());
        }
        self
    }

    /// Check if a field name should be masked.
    #[must_use]
    pub fn should_mask(&self, field_name: &str) -> bool {
        should_mask_field(field_name, &self.sensitive_fields)
    }
}

impl Default for MaskingLayer {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Writer-based masking
// ---------------------------------------------------------------------------

/// Create a masking writer factory for use with tracing-subscriber's `with_writer`.
///
/// Returns a closure that produces [`MaskingWriter`] instances wrapping stderr.
/// When the sensitive fields set is empty and no service fields are set, the
/// writer passes through without buffering or redaction.
pub fn make_masking_writer(
    sensitive_fields: HashSet<String>,
    is_json: bool,
    service_name: Option<String>,
    service_version: Option<String>,
) -> impl Fn() -> MaskingWriter<io::Stderr> + Send + Sync {
    let fields = Arc::new(sensitive_fields);
    let name = service_name.map(Arc::from);
    let version = service_version.map(Arc::from);
    move || MaskingWriter {
        inner: io::stderr(),
        buffer: Vec::with_capacity(512),
        sensitive_fields: Arc::clone(&fields),
        is_json,
        service_name: name.clone(),
        service_version: version.clone(),
    }
}

/// A writer that redacts sensitive field values from formatted log output.
///
/// Buffers each log line (tracing-subscriber writes complete lines via
/// `write_all`), applies field-level redaction, then flushes to the inner
/// writer. When the sensitive fields set is empty and no service fields are
/// set, writes pass through directly with no buffering overhead.
pub struct MaskingWriter<W: io::Write> {
    inner: W,
    buffer: Vec<u8>,
    sensitive_fields: Arc<HashSet<String>>,
    is_json: bool,
    /// Service name injected into JSON log output (JSON mode only).
    service_name: Option<Arc<str>>,
    /// Service version injected into JSON log output (JSON mode only).
    service_version: Option<Arc<str>>,
}

impl<W: io::Write> MaskingWriter<W> {
    /// Create a new masking writer wrapping the given writer.
    ///
    /// When `sensitive_fields` is empty, writes pass through with no overhead.
    /// Set `is_json` to `true` for JSON-format redaction, `false` for text.
    #[must_use]
    pub fn new(inner: W, sensitive_fields: Arc<HashSet<String>>, is_json: bool) -> Self {
        Self {
            inner,
            buffer: Vec::with_capacity(512),
            sensitive_fields,
            is_json,
            service_name: None,
            service_version: None,
        }
    }

    fn flush_buffer(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let line = String::from_utf8_lossy(&self.buffer);
        let redacted = if self.is_json {
            inject_and_redact_json_line(
                &line,
                &self.sensitive_fields,
                self.service_name.as_deref(),
                self.service_version.as_deref(),
            )
        } else {
            redact_text_line(&line, &self.sensitive_fields)
        };
        self.inner.write_all(redacted.as_bytes())?;
        self.buffer.clear();
        Ok(())
    }

    /// Returns `true` if this writer must buffer output (masking or injection active).
    fn needs_buffering(&self) -> bool {
        !self.sensitive_fields.is_empty()
            || self.service_name.is_some()
            || self.service_version.is_some()
    }
}

impl<W: io::Write> io::Write for MaskingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !self.needs_buffering() {
            return self.inner.write(buf);
        }
        self.buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.needs_buffering() {
            self.flush_buffer()?;
        }
        self.inner.flush()
    }
}

impl<W: io::Write> Drop for MaskingWriter<W> {
    fn drop(&mut self) {
        if self.needs_buffering() {
            let _ = self.flush_buffer();
        }
    }
}

// ---------------------------------------------------------------------------
// Redaction functions
// ---------------------------------------------------------------------------

/// Check if a field name matches any sensitive pattern (case-insensitive substring).
fn should_mask_field(field_name: &str, sensitive: &HashSet<String>) -> bool {
    let lower = field_name.to_lowercase();
    sensitive.iter().any(|s| lower.contains(s.as_str()))
}

/// Inject service fields and redact sensitive fields in a JSON log line.
///
/// Parses the line as JSON, inserts `service` and `version` at the root level
/// (if provided), walks the object tree and replaces values of sensitive keys
/// with `[REDACTED]`, then re-serialises.
fn inject_and_redact_json_line(
    line: &str,
    sensitive: &HashSet<String>,
    service_name: Option<&str>,
    service_version: Option<&str>,
) -> String {
    let trimmed = line.trim_end_matches('\n');
    if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let serde_json::Value::Object(ref mut map) = value {
            if let Some(name) = service_name {
                map.insert(
                    "service".to_string(),
                    serde_json::Value::String(name.to_string()),
                );
            }
            if let Some(ver) = service_version {
                map.insert(
                    "version".to_string(),
                    serde_json::Value::String(ver.to_string()),
                );
            }

            // Inject K8s context fields (no-op on bare metal — fields are None)
            let ctx = crate::env::runtime_context();
            if let Some(ref pod) = ctx.pod_name {
                map.insert(
                    "pod_name".to_string(),
                    serde_json::Value::String(pod.clone()),
                );
            }
            if let Some(ref ns) = ctx.namespace {
                map.insert(
                    "namespace".to_string(),
                    serde_json::Value::String(ns.clone()),
                );
            }
            if let Some(ref node) = ctx.node_name {
                map.insert(
                    "node_name".to_string(),
                    serde_json::Value::String(node.clone()),
                );
            }
        }
        redact_json_value(&mut value, sensitive);
        let mut result = serde_json::to_string(&value).unwrap_or_else(|_| trimmed.to_string());
        if line.ends_with('\n') {
            result.push('\n');
        }
        result
    } else {
        line.to_string()
    }
}

/// Recursively redact sensitive keys in a JSON value.
fn redact_json_value(value: &mut serde_json::Value, sensitive: &HashSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                if should_mask_field(key, sensitive) {
                    *val = serde_json::Value::String(REDACTED.to_string());
                } else {
                    redact_json_value(val, sensitive);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                redact_json_value(item, sensitive);
            }
        }
        _ => {}
    }
}

/// Redact sensitive fields in a text-format log line.
///
/// Tracing-subscriber's text formatter outputs fields as `name=value` (Debug)
/// or `name="string value"` (quoted strings). This function finds sensitive
/// field names and replaces their values with `[REDACTED]`.
fn redact_text_line(line: &str, sensitive: &HashSet<String>) -> String {
    let mut result = String::with_capacity(line.len());
    let mut pos = 0;

    while pos < line.len() {
        match line[pos..].find('=') {
            None => {
                result.push_str(&line[pos..]);
                break;
            }
            Some(rel_eq) => {
                let eq_pos = pos + rel_eq;

                // Scan backwards from '=' to find the field name start
                let field_start = line[pos..eq_pos]
                    .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.')
                    .map_or(pos, |rp| pos + rp + 1);
                let field_name = &line[field_start..eq_pos];

                if !field_name.is_empty() && should_mask_field(field_name, sensitive) {
                    // Copy everything up to and including '='
                    result.push_str(&line[pos..=eq_pos]);

                    // Skip the value and replace with redacted placeholder
                    let after_eq = eq_pos + 1;
                    let value_end = skip_field_value(line, after_eq);
                    result.push_str(REDACTED);
                    pos = value_end;
                } else {
                    // Not sensitive — copy through the '=' and continue
                    result.push_str(&line[pos..=eq_pos]);
                    pos = eq_pos + 1;
                }
            }
        }
    }

    result
}

/// Skip past a field value in text-format output, returning the position after the value.
fn skip_field_value(line: &str, start: usize) -> usize {
    if start >= line.len() {
        return start;
    }
    if line.as_bytes()[start] == b'"' {
        // Quoted value — find closing quote (handle escaped quotes)
        let mut i = start + 1;
        while i < line.len() {
            if line.as_bytes()[i] == b'"' && line.as_bytes()[i - 1] != b'\\' {
                return i + 1;
            }
            i += 1;
        }
        line.len()
    } else {
        // Unquoted value — ends at next whitespace
        line[start..]
            .find(char::is_whitespace)
            .map_or(line.len(), |wp| start + wp)
    }
}

/// Mask sensitive values in a string.
///
/// Replaces values that look like tokens, keys, or passwords with `[REDACTED]`.
#[must_use]
pub fn mask_sensitive_string(input: &str, patterns: &[&str]) -> String {
    let mut result = input.to_string();

    for pattern in patterns {
        // Simple pattern matching for key=value or "key": "value"
        let search_patterns = [
            format!("{pattern}="),
            format!("{pattern}:"),
            format!("\"{pattern}\""),
        ];

        for search in &search_patterns {
            if let Some(start) = result.to_lowercase().find(&search.to_lowercase()) {
                // Find the value after the pattern
                let value_start = start + search.len();
                if let Some(rest) = result.get(value_start..) {
                    // Find end of value (space, comma, quote, or end of string)
                    let value_end = rest
                        .find(|c: char| c.is_whitespace() || c == ',' || c == '"' || c == '}')
                        .unwrap_or(rest.len());

                    let before = &result[..value_start];
                    let after = &rest[value_end..];
                    result = format!("{before}{REDACTED}{after}");
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Shared buffer for testing MaskingWriter (survives writer drop)
    struct TestWriter(Arc<Mutex<Vec<u8>>>);

    impl io::Write for TestWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn test_default_sensitive_fields() {
        let fields = default_sensitive_fields();
        assert!(fields.contains(&"password".to_string()));
        assert!(fields.contains(&"token".to_string()));
        assert!(fields.contains(&"api_key".to_string()));
        assert!(fields.contains(&"secret".to_string()));
    }

    #[test]
    fn test_masking_layer_should_mask() {
        let layer = MaskingLayer::new();

        assert!(layer.should_mask("password"));
        assert!(layer.should_mask("PASSWORD"));
        assert!(layer.should_mask("user_password"));
        assert!(layer.should_mask("api_key"));
        assert!(layer.should_mask("secret_token"));

        assert!(!layer.should_mask("username"));
        assert!(!layer.should_mask("host"));
        assert!(!layer.should_mask("port"));
    }

    #[test]
    fn test_masking_layer_custom_fields() {
        let layer = MaskingLayer::with_fields(vec!["custom_secret".to_string()]);

        assert!(layer.should_mask("custom_secret"));
        assert!(!layer.should_mask("password")); // Not in custom list
    }

    #[test]
    fn test_masking_layer_add_fields() {
        let layer = MaskingLayer::new().add_fields(vec!["my_custom_field".to_string()]);

        assert!(layer.should_mask("my_custom_field"));
        assert!(layer.should_mask("password")); // Still has defaults
    }

    #[test]
    fn test_mask_sensitive_string() {
        let input = "password=secret123 username=john";
        let result = mask_sensitive_string(input, &["password"]);
        assert!(result.contains("[REDACTED]"));
        assert!(result.contains("username=john"));
    }

    // --- JSON redaction tests ---

    #[test]
    fn test_redact_json_line_sensitive_field() {
        let sensitive: HashSet<String> = ["password".to_string()].into_iter().collect();
        let input =
            "{\"level\":\"INFO\",\"fields\":{\"message\":\"hello\",\"password\":\"secret123\"}}\n";
        let result = inject_and_redact_json_line(input, &sensitive, None, None);
        assert!(result.contains("[REDACTED]"));
        assert!(!result.contains("secret123"));
        assert!(result.contains("hello"));
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn test_redact_json_line_nested() {
        let sensitive: HashSet<String> = ["token".to_string()].into_iter().collect();
        let input = r#"{"fields":{"config":{"token":"abc123","host":"localhost"}}}"#;
        let result = inject_and_redact_json_line(input, &sensitive, None, None);
        assert!(!result.contains("abc123"));
        assert!(result.contains("localhost"));
    }

    #[test]
    fn test_redact_json_line_preserves_non_sensitive() {
        let sensitive: HashSet<String> = ["password".to_string()].into_iter().collect();
        let input = r#"{"level":"INFO","fields":{"username":"john","host":"db.example.com"}}"#;
        let result = inject_and_redact_json_line(input, &sensitive, None, None);
        assert!(result.contains("john"));
        assert!(result.contains("db.example.com"));
    }

    #[test]
    fn test_redact_json_line_invalid_json_passthrough() {
        let sensitive: HashSet<String> = ["password".to_string()].into_iter().collect();
        let input = "this is not json\n";
        let result = inject_and_redact_json_line(input, &sensitive, None, None);
        assert_eq!(result, input);
    }

    // --- Text redaction tests ---

    #[test]
    fn test_redact_text_line_quoted_value() {
        let sensitive: HashSet<String> = ["password".to_string()].into_iter().collect();
        let input = r#"2026-01-01T00:00:00Z  INFO target: hello password="secret123" user="john""#;
        let result = redact_text_line(input, &sensitive);
        assert!(!result.contains("secret123"));
        assert!(result.contains("password=[REDACTED]"));
        assert!(result.contains(r#"user="john""#));
    }

    #[test]
    fn test_redact_text_line_unquoted_value() {
        let sensitive: HashSet<String> = ["token".to_string()].into_iter().collect();
        let input = "2026-01-01T00:00:00Z  INFO target: msg token=abc123 count=42";
        let result = redact_text_line(input, &sensitive);
        assert!(!result.contains("abc123"));
        assert!(result.contains("token=[REDACTED]"));
        assert!(result.contains("count=42"));
    }

    #[test]
    fn test_redact_text_line_no_sensitive_fields() {
        let sensitive: HashSet<String> = ["password".to_string()].into_iter().collect();
        let input = "2026-01-01T00:00:00Z  INFO target: hello username=john count=42";
        let result = redact_text_line(input, &sensitive);
        assert_eq!(result, input);
    }

    #[test]
    fn test_redact_text_line_case_insensitive() {
        let sensitive: HashSet<String> = ["password".to_string()].into_iter().collect();
        let input = r#"2026-01-01T00:00:00Z  INFO target: msg PASSWORD="secret""#;
        let result = redact_text_line(input, &sensitive);
        assert!(!result.contains("secret"));
        assert!(result.contains("[REDACTED]"));
    }

    #[test]
    fn test_redact_text_line_multiple_sensitive() {
        let sensitive: HashSet<String> = ["password".to_string(), "token".to_string()]
            .into_iter()
            .collect();
        let input = r#"password="pass1" host=localhost token=tok123"#;
        let result = redact_text_line(input, &sensitive);
        assert!(!result.contains("pass1"));
        assert!(!result.contains("tok123"));
        assert!(result.contains("host=localhost"));
        assert_eq!(result.matches("[REDACTED]").count(), 2);
    }

    // --- MaskingWriter tests ---

    #[test]
    fn test_masking_writer_passthrough_when_empty() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let sensitive = Arc::new(HashSet::new());
        {
            let mut writer = MaskingWriter {
                inner: TestWriter(Arc::clone(&buf)),
                buffer: Vec::new(),
                sensitive_fields: sensitive,
                is_json: false,
                service_name: None,
                service_version: None,
            };
            io::Write::write_all(&mut writer, b"password=secret\n").unwrap();
        }
        let guard = buf.lock().unwrap();
        let output = String::from_utf8_lossy(&guard);
        assert_eq!(output, "password=secret\n");
    }

    #[test]
    fn test_masking_writer_redacts_text_on_drop() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let sensitive: HashSet<String> = ["password".to_string()].into_iter().collect();
        {
            let mut writer = MaskingWriter {
                inner: TestWriter(Arc::clone(&buf)),
                buffer: Vec::new(),
                sensitive_fields: Arc::new(sensitive),
                is_json: false,
                service_name: None,
                service_version: None,
            };
            io::Write::write_all(&mut writer, b"password=secret123 user=john\n").unwrap();
        }
        let guard = buf.lock().unwrap();
        let output = String::from_utf8_lossy(&guard);
        assert!(output.contains("[REDACTED]"));
        assert!(!output.contains("secret123"));
        assert!(output.contains("user=john"));
    }

    #[test]
    fn test_masking_writer_redacts_json_on_drop() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let sensitive: HashSet<String> = ["password".to_string()].into_iter().collect();
        {
            let mut writer = MaskingWriter {
                inner: TestWriter(Arc::clone(&buf)),
                buffer: Vec::new(),
                sensitive_fields: Arc::new(sensitive),
                is_json: true,
                service_name: None,
                service_version: None,
            };
            let json = b"{\"message\":\"hello\",\"password\":\"secret123\"}\n";
            io::Write::write_all(&mut writer, json).unwrap();
        }
        let guard = buf.lock().unwrap();
        let output = String::from_utf8_lossy(&guard);
        assert!(output.contains("[REDACTED]"));
        assert!(!output.contains("secret123"));
        assert!(output.contains("hello"));
    }
}
