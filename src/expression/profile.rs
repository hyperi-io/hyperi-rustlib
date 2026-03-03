// Project:   hyperi-rustlib
// File:      src/expression/profile.rs
// Purpose:   DFE expression profile — allowed/disallowed CEL functions
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! DFE expression profile — allowed and disallowed CEL functions.
//!
//! The DFE profile restricts CEL to a high-performance subset suitable
//! for per-record evaluation at ingest/query time. Iteration macros
//! and time functions are excluded — ClickHouse handles time natively.

/// CEL functions allowed in the DFE expression profile.
pub const ALLOWED_FUNCTIONS: &[&str] = &[
    // String operations
    "contains",
    "startsWith",
    "endsWith",
    "matches",
    // Collection
    "size",
    // Existence
    "has",
    // Type casts
    "int",
    "uint",
    "double",
    "string",
    "bool",
];

/// CEL functions explicitly excluded from the DFE profile.
///
/// Per-element iteration macros (`map`, `filter`, `exists`, `all`) are
/// excluded because they have unbounded cost proportional to collection
/// size. Time functions (`timestamp`, `duration`) are excluded because
/// ClickHouse handles time natively.
pub const DISALLOWED_FUNCTIONS: &[&str] = &[
    "map",
    "filter",
    "exists",
    "all",
    "exists_one",
    "timestamp",
    "duration",
];

/// CEL keywords and built-in names that look like function calls but
/// should be skipped during profile scanning.
const SKIP_NAMES: &[&str] = &[
    "true", "false", "null", "in", "has", "int", "uint", "double", "string", "bool",
];

/// Scan an expression for disallowed function calls.
///
/// Returns a list of error strings (empty if all function calls are
/// within the DFE profile).
#[must_use]
pub fn check_profile(expr: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let bytes = expr.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Skip non-identifier characters
        if !is_ident_start(bytes[i]) {
            i += 1;
            continue;
        }

        // Read identifier
        let start = i;
        while i < len && is_ident_char(bytes[i]) {
            i += 1;
        }
        let name = &expr[start..i];

        // Skip whitespace
        while i < len && bytes[i] == b' ' {
            i += 1;
        }

        // Check if followed by `(`
        if i < len && bytes[i] == b'(' {
            if SKIP_NAMES.contains(&name) {
                continue;
            }

            if DISALLOWED_FUNCTIONS.contains(&name) {
                errors.push(format!(
                    "Function '{name}()' is not allowed in the DFE expression profile. \
                     Excluded for performance: per-element iteration or time functions."
                ));
            }
        }
    }

    errors
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_function_passes() {
        assert!(check_profile(r#"msg.contains("error")"#).is_empty());
    }

    #[test]
    fn disallowed_map_rejected() {
        let errors = check_profile("[1,2,3].map(x, x * 2)");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("map()"));
    }

    #[test]
    fn disallowed_filter_rejected() {
        let errors = check_profile("[1,2,3].filter(x, x > 1)");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("filter()"));
    }

    #[test]
    fn disallowed_timestamp_rejected() {
        let errors = check_profile(r#"timestamp("2024-01-01T00:00:00Z")"#);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("timestamp()"));
    }

    #[test]
    fn keywords_skipped() {
        // has(), int(), bool() — look like function calls but are allowed
        assert!(check_profile("has(user.name)").is_empty());
        assert!(check_profile("int(x) > 10").is_empty());
        assert!(check_profile("bool(y)").is_empty());
    }

    #[test]
    fn plain_comparison_passes() {
        assert!(check_profile(r#"severity == "critical""#).is_empty());
    }

    #[test]
    fn compound_expression_passes() {
        assert!(check_profile(r#"severity == "critical" && amount > 10000"#).is_empty());
    }
}
