// Project:   hyperi-rustlib
// File:      src/transport/filter/classify.rs
// Purpose:   CEL expression classification into performance tiers
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Classify CEL expressions into performance tiers via text pattern matching.
//!
//! Tier 1 patterns are detected by regex and executed as SIMD field operations
//! (no CEL engine). Expressions that don't match Tier 1 are classified as
//! Tier 2 (standard CEL) or Tier 3 (complex CEL with restricted functions).

use std::sync::LazyLock;

use regex::Regex;

use super::config::FilterTier;

/// Recognised Tier 1 operation extracted from the expression text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tier1Op {
    FieldExists { field: String },
    FieldNotExists { field: String },
    FieldEquals { field: String, value: String },
    FieldNotEquals { field: String, value: String },
    FieldStartsWith { field: String, prefix: String },
    FieldEndsWith { field: String, suffix: String },
    FieldContains { field: String, substring: String },
}

/// Result of classifying an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassifyResult {
    /// Expression matches a Tier 1 SIMD pattern.
    Tier1(Tier1Op),
    /// Expression is valid CEL without restricted functions (Tier 2).
    Tier2 { fields: Vec<String> },
    /// Expression uses restricted functions (Tier 3).
    Tier3 { fields: Vec<String> },
}

impl ClassifyResult {
    #[must_use]
    pub fn tier(&self) -> FilterTier {
        match self {
            Self::Tier1(_) => FilterTier::Tier1,
            Self::Tier2 { .. } => FilterTier::Tier2,
            Self::Tier3 { .. } => FilterTier::Tier3,
        }
    }
}

// ---------------------------------------------------------------------------
// Tier 1 regex patterns (compiled once via LazyLock)
// ---------------------------------------------------------------------------

// Field name: word chars + dots for nested paths
static RE_HAS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*has\(\s*([\w.]+)\s*\)\s*$").unwrap());

static RE_NOT_HAS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*!\s*has\(\s*([\w.]+)\s*\)\s*$").unwrap());

static RE_EQ_STR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*([\w.]+)\s*==\s*"([^"]*)"\s*$"#).unwrap());

static RE_NEQ_STR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*([\w.]+)\s*!=\s*"([^"]*)"\s*$"#).unwrap());

static RE_STARTS_WITH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*([\w.]+)\s*\.\s*startsWith\(\s*"([^"]*)"\s*\)\s*$"#).unwrap()
});

static RE_ENDS_WITH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*([\w.]+)\s*\.\s*endsWith\(\s*"([^"]*)"\s*\)\s*$"#).unwrap());

static RE_CONTAINS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*([\w.]+)\s*\.\s*contains\(\s*"([^"]*)"\s*\)\s*$"#).unwrap());

// Restricted function names (Tier 3)
const RESTRICTED_FUNCTIONS: &[&str] = &[
    "matches",
    "map",
    "filter",
    "exists",
    "all",
    "exists_one",
    "timestamp",
    "duration",
];

// CEL keywords and built-in function names (NOT field references)
const CEL_KEYWORDS: &[&str] = &[
    "true",
    "false",
    "null",
    "in",
    "has",
    "size",
    "int",
    "uint",
    "double",
    "string",
    "bool",
    "type",
    "contains",
    "startsWith",
    "endsWith",
    "matches",
    "map",
    "filter",
    "exists",
    "all",
    "exists_one",
    "timestamp",
    "duration",
];

/// Classify a CEL expression into a performance tier.
///
/// Returns `Err` if the expression is syntactically invalid (can't even be
/// parsed as a potential CEL expression — empty, unbalanced quotes, etc.).
///
/// # Examples
///
/// ```rust,ignore
/// let result = classify("has(_table)");
/// assert!(matches!(result, Ok(ClassifyResult::Tier1(..))));
///
/// let result = classify("severity > 3 && source != \"internal\"");
/// assert!(matches!(result, Ok(ClassifyResult::Tier2 { .. })));
/// ```
pub fn classify(expr: &str) -> Result<ClassifyResult, String> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Err("empty expression".into());
    }

    // Try Tier 1 patterns first (ordered by expected frequency)
    if let Some(op) = try_tier1(trimmed) {
        return Ok(ClassifyResult::Tier1(op));
    }

    // Not Tier 1 — check for restricted functions (Tier 3) vs standard (Tier 2)
    let has_restricted = check_restricted_functions(trimmed);
    let fields = extract_field_references(trimmed);

    if has_restricted {
        Ok(ClassifyResult::Tier3 { fields })
    } else {
        Ok(ClassifyResult::Tier2 { fields })
    }
}

/// Try to match a Tier 1 pattern. Returns `None` if no pattern matches.
fn try_tier1(expr: &str) -> Option<Tier1Op> {
    // has(field)
    if let Some(caps) = RE_HAS.captures(expr) {
        return Some(Tier1Op::FieldExists {
            field: caps[1].to_string(),
        });
    }

    // !has(field)
    if let Some(caps) = RE_NOT_HAS.captures(expr) {
        return Some(Tier1Op::FieldNotExists {
            field: caps[1].to_string(),
        });
    }

    // field == "value"
    if let Some(caps) = RE_EQ_STR.captures(expr) {
        return Some(Tier1Op::FieldEquals {
            field: caps[1].to_string(),
            value: caps[2].to_string(),
        });
    }

    // field != "value"
    if let Some(caps) = RE_NEQ_STR.captures(expr) {
        return Some(Tier1Op::FieldNotEquals {
            field: caps[1].to_string(),
            value: caps[2].to_string(),
        });
    }

    // field.startsWith("prefix")
    if let Some(caps) = RE_STARTS_WITH.captures(expr) {
        return Some(Tier1Op::FieldStartsWith {
            field: caps[1].to_string(),
            prefix: caps[2].to_string(),
        });
    }

    // field.endsWith("suffix")
    if let Some(caps) = RE_ENDS_WITH.captures(expr) {
        return Some(Tier1Op::FieldEndsWith {
            field: caps[1].to_string(),
            suffix: caps[2].to_string(),
        });
    }

    // field.contains("substring")
    if let Some(caps) = RE_CONTAINS.captures(expr) {
        return Some(Tier1Op::FieldContains {
            field: caps[1].to_string(),
            substring: caps[2].to_string(),
        });
    }

    None
}

/// Check if the expression uses any restricted functions (Tier 3).
///
/// Text-scanning approach (same as `profile.rs`). Scans for function names
/// followed by `(`, skipping occurrences inside string literals.
fn check_restricted_functions(expr: &str) -> bool {
    for func in RESTRICTED_FUNCTIONS {
        // Look for `func(` pattern, not inside a string
        let pattern = format!("{func}(");
        if let Some(pos) = expr.find(&pattern) {
            // Check we're not inside a string literal by counting quotes before pos
            let before = &expr[..pos];
            let quote_count = before.chars().filter(|&c| c == '"').count();
            if quote_count % 2 == 0 {
                // Even number of quotes = we're outside a string
                return true;
            }
        }
    }
    false
}

/// Extract field references from an expression (for Tier 2/3 CEL context building).
///
/// Scans for identifier patterns that aren't CEL keywords or function names.
/// Returns unique field names (may include dotted paths).
fn extract_field_references(expr: &str) -> Vec<String> {
    static RE_IDENT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\b([a-zA-Z_][\w.]*)\b").unwrap());

    let mut fields: Vec<String> = Vec::new();
    let mut in_string = false;

    // Track string boundaries
    for (i, ch) in expr.char_indices() {
        if ch == '"' {
            in_string = !in_string;
        }
        if in_string {
            continue;
        }

        // Check if an identifier starts here
        if let Some(m) = RE_IDENT.find(&expr[i..])
            && m.start() == 0
        {
            let ident = m.as_str();
            // Skip CEL keywords
            let base = ident.split('.').next().unwrap_or(ident);
            if !CEL_KEYWORDS.contains(&base) && !fields.contains(&ident.to_string()) {
                fields.push(ident.to_string());
            }
        }
    }

    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_has_field() {
        let result = classify("has(_table)").unwrap();
        assert_eq!(result.tier(), FilterTier::Tier1);
        assert!(matches!(
            result,
            ClassifyResult::Tier1(Tier1Op::FieldExists { ref field }) if field == "_table"
        ));
    }

    #[test]
    fn classify_not_has_field() {
        let result = classify("!has(_internal)").unwrap();
        assert_eq!(result.tier(), FilterTier::Tier1);
        assert!(matches!(
            result,
            ClassifyResult::Tier1(Tier1Op::FieldNotExists { ref field }) if field == "_internal"
        ));
    }

    #[test]
    fn classify_field_equals_string() {
        let result = classify(r#"status == "poison""#).unwrap();
        assert_eq!(result.tier(), FilterTier::Tier1);
        assert!(matches!(
            result,
            ClassifyResult::Tier1(Tier1Op::FieldEquals { ref field, ref value })
                if field == "status" && value == "poison"
        ));
    }

    #[test]
    fn classify_field_not_equals() {
        let result = classify(r#"source != "internal""#).unwrap();
        assert_eq!(result.tier(), FilterTier::Tier1);
        assert!(matches!(
            result,
            ClassifyResult::Tier1(Tier1Op::FieldNotEquals { ref field, ref value })
                if field == "source" && value == "internal"
        ));
    }

    #[test]
    fn classify_starts_with() {
        let result = classify(r#"host.startsWith("prod-")"#).unwrap();
        assert_eq!(result.tier(), FilterTier::Tier1);
        assert!(matches!(
            result,
            ClassifyResult::Tier1(Tier1Op::FieldStartsWith { ref field, ref prefix })
                if field == "host" && prefix == "prod-"
        ));
    }

    #[test]
    fn classify_ends_with() {
        let result = classify(r#"name.endsWith(".log")"#).unwrap();
        assert_eq!(result.tier(), FilterTier::Tier1);
        assert!(matches!(
            result,
            ClassifyResult::Tier1(Tier1Op::FieldEndsWith { ref field, ref suffix })
                if field == "name" && suffix == ".log"
        ));
    }

    #[test]
    fn classify_contains() {
        let result = classify(r#"path.contains("/api/")"#).unwrap();
        assert_eq!(result.tier(), FilterTier::Tier1);
        assert!(matches!(
            result,
            ClassifyResult::Tier1(Tier1Op::FieldContains { ref field, ref substring })
                if field == "path" && substring == "/api/"
        ));
    }

    #[test]
    fn classify_dotted_path() {
        let result = classify(r#"metadata.source == "aws""#).unwrap();
        assert_eq!(result.tier(), FilterTier::Tier1);
        assert!(matches!(
            result,
            ClassifyResult::Tier1(Tier1Op::FieldEquals { ref field, ref value })
                if field == "metadata.source" && value == "aws"
        ));
    }

    #[test]
    fn classify_compound_expression_is_tier2() {
        let result = classify(r#"severity > 3 && source != "internal""#).unwrap();
        assert_eq!(result.tier(), FilterTier::Tier2);
    }

    #[test]
    fn classify_regex_is_tier3() {
        let result = classify(r#"field.matches("^prod-.*")"#).unwrap();
        assert_eq!(result.tier(), FilterTier::Tier3);
    }

    #[test]
    fn classify_iteration_is_tier3() {
        let result = classify(r#"tags.exists(t, t == "pii")"#).unwrap();
        assert_eq!(result.tier(), FilterTier::Tier3);
    }

    #[test]
    fn classify_empty_expression_errors() {
        assert!(classify("").is_err());
        assert!(classify("   ").is_err());
    }

    #[test]
    fn classify_whitespace_tolerance() {
        let result = classify(r#"  has( _table )  "#).unwrap();
        assert_eq!(result.tier(), FilterTier::Tier1);
    }

    #[test]
    fn classify_tier2_extracts_fields() {
        let result = classify(r#"severity > 3 && source != "internal""#).unwrap();
        if let ClassifyResult::Tier2 { fields } = result {
            assert!(fields.contains(&"severity".to_string()));
            assert!(fields.contains(&"source".to_string()));
        } else {
            panic!("Expected Tier2");
        }
    }

    #[test]
    fn restricted_function_in_string_not_detected() {
        // "matches" inside a string literal should NOT trigger Tier 3
        let result = classify(r#"field == "matches""#).unwrap();
        assert_eq!(result.tier(), FilterTier::Tier1);
    }
}
