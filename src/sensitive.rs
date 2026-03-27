// Project:   hyperi-rustlib
// File:      src/sensitive.rs
// Purpose:   Compile-time safe sensitive string type that never serialises its value
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Sensitive string type for fields that must never be exposed.
//!
//! [`SensitiveString`] wraps a `String` but always serialises as
//! `"***REDACTED***"`. This provides compile-time guarantees that the
//! value cannot leak through serialisation — not in the config registry,
//! not in logs, not in debug output, not in API responses.
//!
//! This module is always available (no feature gate) so that any module
//! can use `SensitiveString` regardless of which features are enabled.
//!
//! # Three layers of secret protection
//!
//! | Layer | Mechanism | Catches |
//! |-------|-----------|---------|
//! | `#[serde(skip_serializing)]` | Field absent from output | Fields that should never appear |
//! | Heuristic auto-redaction | Field name pattern matching | Common names: password, secret, token, key |
//! | `SensitiveString` type | Value always serialises as redacted | Non-obvious fields: connection_string, dsn |
//!
//! # Usage
//!
//! ```rust
//! use hyperi_rustlib::SensitiveString;
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Serialize, Deserialize)]
//! struct DbConfig {
//!     host: String,
//!     port: u16,
//!     connection_string: SensitiveString,  // Always redacted
//! }
//! ```

use std::fmt;

use serde::de::Deserializer;
use serde::ser::Serializer;

const REDACTED: &str = "***REDACTED***";

/// A string value that is always redacted when serialised.
///
/// Use this for config fields that contain secrets but don't have
/// obviously-sensitive names (e.g., `connection_string`, `dsn`, `uri`).
///
/// - `Serialize` always outputs `"***REDACTED***"`
/// - `Deserialize` reads the actual value normally
/// - `Display` shows `***REDACTED***`
/// - `Debug` shows `SensitiveString(***REDACTED***)`
/// - Inner value accessible via `.expose()` for application logic
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SensitiveString(String);

impl SensitiveString {
    /// Create a new sensitive string.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Expose the inner value for application logic.
    ///
    /// This is the only way to access the actual value. The name is
    /// intentionally explicit to make usage grep-able in code review.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Check if the inner value is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl serde::Serialize for SensitiveString {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(REDACTED)
    }
}

impl<'de> serde::Deserialize<'de> for SensitiveString {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(SensitiveString)
    }
}

impl fmt::Display for SensitiveString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{REDACTED}")
    }
}

impl fmt::Debug for SensitiveString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SensitiveString({REDACTED})")
    }
}

impl From<String> for SensitiveString {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SensitiveString {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_always_redacted() {
        let s = SensitiveString::new("my_actual_secret");
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, format!("\"{REDACTED}\""));
        assert!(!json.contains("my_actual_secret"));
    }

    #[test]
    fn deserialize_reads_actual_value() {
        let json = "\"my_actual_secret\"";
        let s: SensitiveString = serde_json::from_str(json).unwrap();
        assert_eq!(s.expose(), "my_actual_secret");
    }

    #[test]
    fn display_is_redacted() {
        let s = SensitiveString::new("secret123");
        assert_eq!(format!("{s}"), REDACTED);
        assert!(!format!("{s}").contains("secret123"));
    }

    #[test]
    fn debug_is_redacted() {
        let s = SensitiveString::new("secret123");
        let debug = format!("{s:?}");
        assert!(debug.contains(REDACTED));
        assert!(!debug.contains("secret123"));
    }

    #[test]
    fn expose_returns_actual_value() {
        let s = SensitiveString::new("the_real_value");
        assert_eq!(s.expose(), "the_real_value");
    }

    #[test]
    fn default_is_empty() {
        let s = SensitiveString::default();
        assert!(s.is_empty());
        assert_eq!(s.expose(), "");
    }

    #[test]
    fn from_string() {
        let s: SensitiveString = "hello".into();
        assert_eq!(s.expose(), "hello");

        let s: SensitiveString = String::from("world").into();
        assert_eq!(s.expose(), "world");
    }

    #[test]
    fn struct_with_sensitive_field_serialises_safely() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Config {
            host: String,
            connection_string: SensitiveString,
        }

        let config = Config {
            host: "db.example.com".into(),
            connection_string: SensitiveString::new("postgres://user:pass@host/db"),
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("db.example.com"));
        assert!(json.contains(REDACTED));
        assert!(!json.contains("postgres://"));
        assert!(!json.contains("user:pass"));
    }

    #[test]
    fn struct_with_sensitive_field_deserialises_correctly() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Config {
            host: String,
            connection_string: SensitiveString,
        }

        let json =
            r#"{"host":"db.example.com","connection_string":"postgres://user:pass@host/db"}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.host, "db.example.com");
        assert_eq!(
            config.connection_string.expose(),
            "postgres://user:pass@host/db"
        );
    }

    #[test]
    fn no_leak_through_any_serialisation_path() {
        let secret = "super_secret_value_12345";
        let s = SensitiveString::new(secret);

        // serde_json
        assert!(!serde_json::to_string(&s).unwrap().contains(secret));
        // Display
        assert!(!format!("{s}").contains(secret));
        // Debug
        assert!(!format!("{s:?}").contains(secret));
        // Only expose() reveals it
        assert_eq!(s.expose(), secret);
    }
}
