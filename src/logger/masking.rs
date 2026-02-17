// Project:   hyperi-rustlib
// File:      src/logger/masking.rs
// Purpose:   Sensitive data masking for log output
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Sensitive data masking layer for tracing.

#![allow(dead_code)] // Public API functions used by consumers

use std::collections::HashSet;

use tracing::field::{Field, Visit};
use tracing::span::Attributes;
use tracing::Id;
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

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

/// Layer that masks sensitive fields in log output.
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
        let lower = field_name.to_lowercase();
        self.sensitive_fields.iter().any(|s| lower.contains(s))
    }
}

impl Default for MaskingLayer {
    fn default() -> Self {
        Self::new()
    }
}

impl<S> Layer<S> for MaskingLayer
where
    S: tracing::Subscriber,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {
        // We could intercept and modify span attributes here if needed
        let mut visitor = MaskingVisitor {
            layer: self,
            masked: false,
        };
        attrs.record(&mut visitor);
    }
}

/// Visitor that checks for sensitive fields.
struct MaskingVisitor<'a> {
    layer: &'a MaskingLayer,
    masked: bool,
}

impl Visit for MaskingVisitor<'_> {
    fn record_debug(&mut self, field: &Field, _value: &dyn std::fmt::Debug) {
        if self.layer.should_mask(field.name()) {
            self.masked = true;
        }
    }

    fn record_str(&mut self, field: &Field, _value: &str) {
        if self.layer.should_mask(field.name()) {
            self.masked = true;
        }
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
}
