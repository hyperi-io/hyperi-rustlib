// Project:   hyperi-rustlib
// File:      src/expression/profile.rs
// Purpose:   DFE expression profile — allowed/restricted CEL functions
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! DFE expression profile — allowed and restricted CEL functions.
//!
//! The DFE profile restricts CEL to a high-performance subset suitable
//! for per-record evaluation at ingest/query time. Functions with
//! unbounded or unpredictable cost are blocked by default but can be
//! unlocked per-category via [`ProfileConfig`].

/// CEL functions allowed unconditionally in the DFE profile.
pub const ALLOWED_FUNCTIONS: &[&str] = &[
    // String operations (SIMD-friendly, bounded cost)
    "contains",
    "startsWith",
    "endsWith",
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

/// Restricted function categories — blocked by default, opt-in via config.
///
/// Each category has a reason for restriction and a config flag to unlock.
pub const RESTRICTED_REGEX: &[&str] = &["matches"];
pub const RESTRICTED_ITERATION: &[&str] = &["map", "filter", "exists", "all", "exists_one"];
pub const RESTRICTED_TIME: &[&str] = &["timestamp", "duration"];

/// All restricted functions (union of all categories).
pub const DISALLOWED_FUNCTIONS: &[&str] = &[
    "matches",
    "map",
    "filter",
    "exists",
    "all",
    "exists_one",
    "timestamp",
    "duration",
];

/// Configuration for the DFE expression profile.
///
/// Each flag unlocks a category of restricted functions. All default
/// to `false` (blocked). Set explicitly in application config to opt in.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct ProfileConfig {
    /// Allow `matches()` (regex). Unbounded cost per record — use only
    /// when `contains()`/`startsWith()`/`endsWith()` are insufficient.
    pub allow_regex: bool,
    /// Allow `map()`, `filter()`, `exists()`, `all()`, `exists_one()`.
    /// O(n) per collection element — cost proportional to data size.
    pub allow_iteration: bool,
    /// Allow `timestamp()`, `duration()`. Excluded because ClickHouse
    /// handles time natively — rarely needed in CEL expressions.
    pub allow_time: bool,
}

impl ProfileConfig {
    /// Returns the set of functions that are currently blocked.
    #[must_use]
    pub fn blocked_functions(&self) -> Vec<&'static str> {
        let mut blocked = Vec::new();
        if !self.allow_regex {
            blocked.extend_from_slice(RESTRICTED_REGEX);
        }
        if !self.allow_iteration {
            blocked.extend_from_slice(RESTRICTED_ITERATION);
        }
        if !self.allow_time {
            blocked.extend_from_slice(RESTRICTED_TIME);
        }
        blocked
    }
}

/// CEL keywords and built-in names that look like function calls but
/// should be skipped during profile scanning.
const SKIP_NAMES: &[&str] = &[
    "true", "false", "null", "in", "has", "int", "uint", "double", "string", "bool",
];

/// Scan an expression for restricted function calls using default config.
///
/// Returns a list of error strings (empty if all function calls are
/// within the DFE profile). Equivalent to `check_profile_with_config`
/// with [`ProfileConfig::default()`] (all restrictions active).
#[must_use]
pub fn check_profile(expr: &str) -> Vec<String> {
    check_profile_with_config(expr, &ProfileConfig::default())
}

/// Scan an expression for restricted function calls.
///
/// The scanner skips string literals to avoid false positives on
/// function names that appear inside quoted values.
///
/// Returns a list of error strings (empty if compliant).
#[must_use]
pub fn check_profile_with_config(expr: &str, config: &ProfileConfig) -> Vec<String> {
    let blocked = config.blocked_functions();
    if blocked.is_empty() {
        return Vec::new();
    }

    let mut errors = Vec::new();
    let bytes = expr.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Skip string literals (double-quoted and single-quoted)
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            i = skip_string_literal(bytes, i);
            continue;
        }

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

        // Skip whitespace between identifier and potential `(`
        let mut peek = i;
        while peek < len && bytes[peek] == b' ' {
            peek += 1;
        }

        // Check if followed by `(`
        if peek < len && bytes[peek] == b'(' {
            if SKIP_NAMES.contains(&name) {
                continue;
            }

            if blocked.contains(&name) {
                let reason = restriction_reason(name);
                errors.push(format!(
                    "Function '{name}()' is not allowed in the DFE expression profile. {reason}"
                ));
            }
        }
    }

    errors
}

/// Skip past a string literal, handling escape sequences.
///
/// `start` must point to the opening quote character.
/// Returns the index after the closing quote (or end of input).
fn skip_string_literal(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            // Skip escaped character
            i += 2;
            continue;
        }
        if bytes[i] == quote {
            return i + 1;
        }
        i += 1;
    }
    // Unterminated string — return end of input
    bytes.len()
}

fn restriction_reason(name: &str) -> &'static str {
    match name {
        "matches" => {
            "Regex has unbounded cost per record. Use contains()/startsWith()/endsWith() instead, or set allow_regex: true in expression config."
        }
        "map" | "filter" | "exists" | "all" | "exists_one" => {
            "Per-element iteration has O(n) cost proportional to collection size. Set allow_iteration: true in expression config to permit."
        }
        "timestamp" | "duration" => {
            "Time functions excluded — ClickHouse handles time natively. Set allow_time: true in expression config to permit."
        }
        _ => "Restricted by DFE expression profile.",
    }
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

    // ── Default config (all restricted) ─────────────────────────

    #[test]
    fn allowed_function_passes() {
        assert!(check_profile(r#"msg.contains("error")"#).is_empty());
    }

    #[test]
    fn starts_with_passes() {
        assert!(check_profile(r#"path.startsWith("/api/")"#).is_empty());
    }

    #[test]
    fn ends_with_passes() {
        assert!(check_profile(r#"file.endsWith(".log")"#).is_empty());
    }

    #[test]
    fn matches_blocked_by_default() {
        let errors = check_profile(r#"name.matches("^web-[0-9]+$")"#);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("matches()"));
        assert!(errors[0].contains("allow_regex"));
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
    fn disallowed_duration_rejected() {
        let errors = check_profile(r#"duration("1h")"#);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("duration()"));
    }

    #[test]
    fn keywords_skipped() {
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

    // ── String literal false-positive prevention ────────────────

    #[test]
    fn function_name_inside_string_not_flagged() {
        // "filter" appears inside a string literal, not as a function call
        assert!(check_profile(r#"msg.contains("filter")"#).is_empty());
    }

    #[test]
    fn function_name_inside_string_with_parens_not_flagged() {
        // "map(" appears inside a string — should not be flagged
        assert!(check_profile(r#"msg.contains("map(x)")"#).is_empty());
    }

    #[test]
    fn matches_inside_string_not_flagged() {
        assert!(check_profile(r#"msg.contains("matches")"#).is_empty());
    }

    #[test]
    fn timestamp_inside_string_not_flagged() {
        assert!(check_profile(r#"label == "timestamp""#).is_empty());
    }

    #[test]
    fn escaped_quote_inside_string_handled() {
        // String with escaped quote: "filter\"(" — scanner must not exit early
        assert!(check_profile(r#"msg.contains("filter\"(")"#).is_empty());
    }

    #[test]
    fn single_quoted_string_handled() {
        assert!(check_profile("msg.contains('filter')").is_empty());
    }

    #[test]
    fn real_call_after_string_still_caught() {
        // String contains "ok" but then a real map() call follows
        let errors = check_profile(r#""ok" + items.map(x, x)"#);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("map()"));
    }

    // ── Config overrides ────────────────────────────────────────

    #[test]
    fn matches_allowed_with_regex_config() {
        let config = ProfileConfig {
            allow_regex: true,
            ..Default::default()
        };
        assert!(check_profile_with_config(r#"name.matches("^web-[0-9]+$")"#, &config).is_empty());
    }

    #[test]
    fn map_still_blocked_with_regex_config() {
        let config = ProfileConfig {
            allow_regex: true,
            ..Default::default()
        };
        let errors = check_profile_with_config("[1,2].map(x, x)", &config);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("map()"));
    }

    #[test]
    fn iteration_allowed_with_config() {
        let config = ProfileConfig {
            allow_iteration: true,
            ..Default::default()
        };
        assert!(check_profile_with_config("[1,2].map(x, x * 2)", &config).is_empty());
        assert!(check_profile_with_config("[1,2].filter(x, x > 1)", &config).is_empty());
        assert!(check_profile_with_config("[1,2].exists(x, x > 1)", &config).is_empty());
    }

    #[test]
    fn time_allowed_with_config() {
        let config = ProfileConfig {
            allow_time: true,
            ..Default::default()
        };
        assert!(
            check_profile_with_config(r#"timestamp("2024-01-01T00:00:00Z")"#, &config).is_empty()
        );
        assert!(check_profile_with_config(r#"duration("1h")"#, &config).is_empty());
    }

    #[test]
    fn all_restrictions_lifted() {
        let config = ProfileConfig {
            allow_regex: true,
            allow_iteration: true,
            allow_time: true,
        };
        assert!(config.blocked_functions().is_empty());
        assert!(
            check_profile_with_config(r#"name.matches("x") && [1].map(x, x)"#, &config).is_empty()
        );
    }

    // ── Edge cases ──────────────────────────────────────────────

    #[test]
    fn identifier_not_followed_by_paren_is_fine() {
        // "filter" as a variable name, not a function call
        assert!(check_profile("filter > 10").is_empty());
    }

    #[test]
    fn identifier_with_space_before_paren() {
        let errors = check_profile("[1,2].map (x, x)");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("map()"));
    }

    #[test]
    fn empty_expression() {
        assert!(check_profile("").is_empty());
    }

    #[test]
    fn whitespace_only() {
        assert!(check_profile("   ").is_empty());
    }

    #[test]
    fn multiple_violations_reported() {
        let errors = check_profile("[1].map(x, x).filter(y, y > 0)");
        assert_eq!(errors.len(), 2);
    }
}
