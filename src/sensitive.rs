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

use std::cell::Cell;
use std::fmt;

use serde::de::Deserializer;
use serde::ser::Serializer;

const REDACTED: &str = "***REDACTED***";

thread_local! {
    /// Per-thread serde-exposure flag. When set (via [`expose_during`])
    /// [`SensitiveString::serialize`] writes the inner value verbatim
    /// instead of `***REDACTED***`. Default: `false` — every other call
    /// site continues to redact.
    static EXPOSE: Cell<bool> = const { Cell::new(false) };
}

/// Drop-guard for the thread-local exposure flag.
///
/// Using a guard (rather than a try/finally pair) ensures the flag is
/// restored even if the closure passed to [`expose_during`] panics. Held
/// for the duration of the `expose_during` body, dropped at scope-exit.
struct ExposeGuard {
    prev: bool,
}

impl ExposeGuard {
    fn enter() -> Self {
        EXPOSE.with(|e| {
            let prev = e.get();
            e.set(true);
            Self { prev }
        })
    }
}

impl Drop for ExposeGuard {
    fn drop(&mut self) {
        let prev = self.prev;
        EXPOSE.with(|e| e.set(prev));
    }
}

/// Run `f` with [`SensitiveString::serialize`] exposing inner values.
///
/// Use this around code paths that need to serialise-and-deserialise a
/// config struct without destroying its secrets — typically the
/// `figment::Figment::from(Serialized::defaults(&config))` + `.extract()`
/// round-trip in a consumer's config loader.
///
/// # Scope and reentrancy
///
/// The flag is thread-local. Calls from inside the closure on the same
/// thread observe exposure; calls from other threads do not. Nested
/// calls compose correctly (inner guards restore the outer state on
/// drop). Async callers should be aware that the flag does NOT cross
/// `.await` boundaries to other threads — keep the round-trip on one
/// thread, or wrap each thread's section in its own
/// `expose_during`.
///
/// # Panic safety
///
/// If `f` panics, the previous flag value is restored via the
/// [`ExposeGuard`] drop impl before the panic unwinds further.
///
/// # Examples
///
/// ```rust
/// use hyperi_rustlib::{SensitiveString, expose_during};
/// use serde::{Serialize, Deserialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct Cfg {
///     password: SensitiveString,
/// }
///
/// let cfg = Cfg { password: SensitiveString::new("hunter2") };
///
/// // Default: serialise redacts.
/// let json = serde_json::to_string(&cfg).unwrap();
/// assert!(json.contains("***REDACTED***"));
///
/// // Inside expose_during: serialise reveals so a round-trip preserves the value.
/// let round_tripped: Cfg = expose_during(|| {
///     let v = serde_json::to_value(&cfg).unwrap();
///     serde_json::from_value(v).unwrap()
/// });
/// assert_eq!(round_tripped.password.expose(), "hunter2");
///
/// // After the call, default redaction resumes.
/// let json = serde_json::to_string(&cfg).unwrap();
/// assert!(json.contains("***REDACTED***"));
/// ```
pub fn expose_during<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let _guard = ExposeGuard::enter();
    f()
}

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
        // Honour the thread-local exposure flag set by `expose_during`.
        // Without exposure (the default), every serialise path —
        // serde_json::to_string, config-registry dump, logger output —
        // emits the redacted constant. Inside `expose_during`, the
        // serializer emits the inner value verbatim, which is what
        // figment / serde round-trips need to avoid destroying secrets
        // (see hyperi-rustlib#41).
        if EXPOSE.with(Cell::get) {
            serializer.serialize_str(&self.0)
        } else {
            serializer.serialize_str(REDACTED)
        }
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

    // ----- Round-trip preservation (hyperi-rustlib#41) -----

    /// The motivating case from hyperi-rustlib#41: serialise to a serde
    /// `Value`, then deserialise back. Without `expose_during` the
    /// inner string is destroyed (replaced by `***REDACTED***`); inside
    /// the helper, the value survives.
    #[test]
    fn round_trip_inside_expose_during_preserves_value() {
        let s = SensitiveString::new("hunter2");
        let v = expose_during(|| serde_json::to_value(&s).unwrap());
        let round_tripped: SensitiveString = serde_json::from_value(v).unwrap();
        assert_eq!(round_tripped.expose(), "hunter2");
    }

    #[test]
    fn round_trip_outside_expose_during_redacts() {
        let s = SensitiveString::new("hunter2");
        // Default path — no `expose_during` wrap.
        let v = serde_json::to_value(&s).unwrap();
        let round_tripped: SensitiveString = serde_json::from_value(v).unwrap();
        // The serialised form was the literal "***REDACTED***", so the
        // deserialised value is that literal. This is the bug being
        // fixed for the consumer who wraps their round-trip — but the
        // default behaviour is preserved verbatim.
        assert_eq!(round_tripped.expose(), REDACTED);
    }

    #[test]
    fn expose_during_restores_after_body() {
        let s = SensitiveString::new("secret");
        // Before: redacted
        assert!(serde_json::to_string(&s).unwrap().contains(REDACTED));
        // Inside: exposed
        expose_during(|| {
            assert!(serde_json::to_string(&s).unwrap().contains("secret"));
        });
        // After: redacted again — guard restored the flag
        assert!(serde_json::to_string(&s).unwrap().contains(REDACTED));
        assert!(!serde_json::to_string(&s).unwrap().contains("secret"));
    }

    #[test]
    fn expose_during_restores_after_panic() {
        let s = SensitiveString::new("secret");
        let result = std::panic::catch_unwind(|| {
            expose_during(|| {
                // Confirm we're exposed inside the closure.
                assert!(serde_json::to_string(&s).unwrap().contains("secret"));
                panic!("simulated panic");
            })
        });
        assert!(result.is_err(), "panic should have propagated");
        // The drop guard must have restored the flag despite the panic.
        assert!(serde_json::to_string(&s).unwrap().contains(REDACTED));
        assert!(!serde_json::to_string(&s).unwrap().contains("secret"));
    }

    #[test]
    fn expose_during_nests_correctly() {
        let s = SensitiveString::new("secret");
        expose_during(|| {
            assert!(serde_json::to_string(&s).unwrap().contains("secret"));
            expose_during(|| {
                assert!(serde_json::to_string(&s).unwrap().contains("secret"));
            });
            // Inner guard restored OUTER state (which was also exposed).
            assert!(serde_json::to_string(&s).unwrap().contains("secret"));
        });
        // Outer guard restored the original (redacted) state.
        assert!(serde_json::to_string(&s).unwrap().contains(REDACTED));
    }

    #[test]
    fn struct_round_trip_inside_expose_during_preserves_values() {
        // Mirrors the dfe-loader bug: serialise a Config containing a
        // SensitiveString password, merge env overrides via figment,
        // deserialise back. Without expose_during, password becomes
        // "***REDACTED***".
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Config {
            host: String,
            password: SensitiveString,
        }
        let original = Config {
            host: "db.example.com".into(),
            password: SensitiveString::new("env-resolved-secret"),
        };
        let round_tripped: Config = expose_during(|| {
            let v = serde_json::to_value(&original).unwrap();
            serde_json::from_value(v).unwrap()
        });
        assert_eq!(round_tripped.host, "db.example.com");
        assert_eq!(round_tripped.password.expose(), "env-resolved-secret");
    }

    /// Cross-thread isolation: thread A's `expose_during` does NOT
    /// affect thread B's serialisation.
    #[test]
    fn expose_flag_is_thread_local() {
        use std::sync::{Arc, Mutex};
        let s = Arc::new(SensitiveString::new("secret"));
        let observed = Arc::new(Mutex::new(String::new()));

        let s2 = Arc::clone(&s);
        let observed2 = Arc::clone(&observed);
        let handle = std::thread::spawn(move || {
            // Thread B: no expose_during. Must observe REDACTED.
            let out = serde_json::to_string(&*s2).unwrap();
            *observed2.lock().unwrap() = out;
        });

        // Thread A: inside expose_during. Spawn happened above; let it
        // race the closure.
        expose_during(|| {
            std::thread::yield_now();
        });
        handle.join().unwrap();
        let b_output = observed.lock().unwrap().clone();
        assert!(
            b_output.contains(REDACTED),
            "thread B should have observed REDACTED, got: {b_output}"
        );
    }
}
