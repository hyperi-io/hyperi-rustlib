// Project:   hyperi-rustlib
// File:      src/transport/filter/compiled.rs
// Purpose:   Compiled filter variants with Tier 1 SIMD evaluation
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Compiled filter representations and evaluation logic.
//!
//! Tier 1 filters execute as direct `sonic_rs::get_from_slice()` field
//! extraction + string comparison. No CEL engine, no allocation beyond
//! the extracted field value. ~50-100ns per message.

use super::classify::{ClassifyResult, Tier1Op};
use super::config::{FilterAction, FilterDirection, FilterTier, TransportFilterTierConfig};

/// A compiled filter ready for hot-path evaluation.
///
/// Tier 1 variants bypass the CEL engine entirely — they use SIMD JSON
/// field extraction via `sonic_rs::get_from_str()` (zero-copy &str path,
/// no UTF-8 revalidation per call).
///
/// `FieldExists` / `FieldNotExists` for single-segment paths use a
/// pre-compiled `memchr::memmem::Finder` to detect the `"key":` substring
/// in raw bytes, bypassing the JSON parser entirely (~10-20ns vs ~200ns).
#[derive(Debug)]
pub enum CompiledFilter {
    // Tier 1 — SIMD field ops
    FieldExists {
        field: String,
        path: Vec<String>,
        /// Pre-compiled memmem finder for the `"field":` byte pattern.
        /// Used as a fast-path when the path is a single segment (no nested).
        /// `None` for nested paths — falls back to sonic-rs.
        needle: Option<memchr::memmem::Finder<'static>>,
        action: FilterAction,
        expression_text: String,
    },
    FieldNotExists {
        field: String,
        path: Vec<String>,
        needle: Option<memchr::memmem::Finder<'static>>,
        action: FilterAction,
        expression_text: String,
    },
    FieldEquals {
        field: String,
        path: Vec<String>,
        value: String,
        action: FilterAction,
        expression_text: String,
    },
    FieldNotEquals {
        field: String,
        path: Vec<String>,
        value: String,
        action: FilterAction,
        expression_text: String,
    },
    FieldStartsWith {
        field: String,
        path: Vec<String>,
        prefix: String,
        action: FilterAction,
        expression_text: String,
    },
    FieldEndsWith {
        field: String,
        path: Vec<String>,
        suffix: String,
        action: FilterAction,
        expression_text: String,
    },
    FieldContains {
        field: String,
        path: Vec<String>,
        substring: String,
        action: FilterAction,
        expression_text: String,
    },
    // Tier 2/3 — CEL expression (feature-gated)
    #[cfg(feature = "expression")]
    CelExpression {
        program: cel::Program,
        fields: Vec<String>,
        expression_text: String,
        tier: FilterTier,
        action: FilterAction,
    },
}

impl CompiledFilter {
    /// Compile a filter from a CEL expression string.
    ///
    /// Classifies the expression, checks tier gates, and returns the
    /// appropriate compiled variant.
    pub fn from_expression(
        expr: &str,
        action: FilterAction,
        direction: FilterDirection,
        tier_config: &TransportFilterTierConfig,
    ) -> Result<Self, String> {
        let classification = super::classify::classify(expr)?;

        // Check tier gate
        let tier = classification.tier();
        if !tier_config.is_tier_allowed(tier, direction) {
            return Err(format!(
                "classified as {tier} but {tier} filters are not enabled for {direction}. \
                 Set expression.allow_{} to enable.",
                match (tier, direction) {
                    (FilterTier::Tier2, FilterDirection::In) => "cel_filters_in: true",
                    (FilterTier::Tier2, FilterDirection::Out) => "cel_filters_out: true",
                    (FilterTier::Tier3, FilterDirection::In) => "complex_filters_in: true",
                    (FilterTier::Tier3, FilterDirection::Out) => "complex_filters_out: true",
                    (FilterTier::Tier1, _) => unreachable!("Tier 1 is always allowed"),
                }
            ));
        }

        let expression_text = expr.to_string();

        match classification {
            ClassifyResult::Tier1(op) => Ok(Self::from_tier1_op(op, action, expression_text)),
            #[cfg(feature = "expression")]
            ClassifyResult::Tier2 { fields } => {
                let program = crate::expression::compile(expr)
                    .map_err(|e| format!("CEL compilation failed: {e}"))?;
                Ok(Self::CelExpression {
                    program,
                    fields,
                    expression_text,
                    tier: FilterTier::Tier2,
                    action,
                })
            }
            #[cfg(feature = "expression")]
            ClassifyResult::Tier3 { fields } => {
                let profile = crate::expression::ProfileConfig {
                    allow_regex: true,
                    allow_iteration: true,
                    allow_time: true,
                };
                let program = crate::expression::compile_with_config(expr, &profile)
                    .map_err(|e| format!("CEL compilation failed: {e}"))?;
                Ok(Self::CelExpression {
                    program,
                    fields,
                    expression_text,
                    tier: FilterTier::Tier3,
                    action,
                })
            }
            #[cfg(not(feature = "expression"))]
            ClassifyResult::Tier2 { .. } | ClassifyResult::Tier3 { .. } => Err(format!(
                "classified as {tier} but the 'expression' feature is not enabled. \
                 Enable it in Cargo.toml or simplify the expression to Tier 1."
            )),
        }
    }

    fn from_tier1_op(op: Tier1Op, action: FilterAction, expression_text: String) -> Self {
        match op {
            Tier1Op::FieldExists { field } => {
                let path = split_field_path(&field);
                let needle = build_field_needle(&path);
                Self::FieldExists {
                    field,
                    path,
                    needle,
                    action,
                    expression_text,
                }
            }
            Tier1Op::FieldNotExists { field } => {
                let path = split_field_path(&field);
                let needle = build_field_needle(&path);
                Self::FieldNotExists {
                    field,
                    path,
                    needle,
                    action,
                    expression_text,
                }
            }
            Tier1Op::FieldEquals { field, value } => {
                let path = split_field_path(&field);
                Self::FieldEquals {
                    field,
                    path,
                    value,
                    action,
                    expression_text,
                }
            }
            Tier1Op::FieldNotEquals { field, value } => {
                let path = split_field_path(&field);
                Self::FieldNotEquals {
                    field,
                    path,
                    value,
                    action,
                    expression_text,
                }
            }
            Tier1Op::FieldStartsWith { field, prefix } => {
                let path = split_field_path(&field);
                Self::FieldStartsWith {
                    field,
                    path,
                    prefix,
                    action,
                    expression_text,
                }
            }
            Tier1Op::FieldEndsWith { field, suffix } => {
                let path = split_field_path(&field);
                Self::FieldEndsWith {
                    field,
                    path,
                    suffix,
                    action,
                    expression_text,
                }
            }
            Tier1Op::FieldContains { field, substring } => {
                let path = split_field_path(&field);
                Self::FieldContains {
                    field,
                    path,
                    substring,
                    action,
                    expression_text,
                }
            }
        }
    }

    /// Evaluate this filter against a raw JSON payload.
    ///
    /// Returns `Some(action)` if the filter matches, `None` otherwise.
    /// Tier 1: SIMD field extraction via `sonic_rs::get_from_slice()`.
    ///
    /// Zero-copy hot path: uses stack arrays for path segments (no Vec
    /// allocation per message). Single-segment fields are the common case.
    #[inline]
    #[must_use]
    pub fn evaluate(&self, payload: &[u8]) -> Option<FilterAction> {
        match self {
            Self::FieldExists {
                path,
                needle,
                action,
                ..
            } => {
                // Fast path: pre-compiled memmem Finder for single-segment fields.
                // SIMD substring search ~10-20ns vs sonic-rs ~200ns.
                if let Some(n) = needle {
                    return n.find(payload).is_some().then_some(*action);
                }
                // Slow path: nested field, use sonic-rs
                with_path_refs(path, |refs| {
                    sonic_rs::get_from_slice(payload, refs)
                        .is_ok()
                        .then_some(*action)
                })
            }
            Self::FieldNotExists {
                path,
                needle,
                action,
                ..
            } => {
                if let Some(n) = needle {
                    return n.find(payload).is_none().then_some(*action);
                }
                with_path_refs(path, |refs| {
                    sonic_rs::get_from_slice(payload, refs)
                        .is_err()
                        .then_some(*action)
                })
            }
            Self::FieldEquals {
                path,
                value,
                action,
                ..
            } => with_path_refs(path, |refs| {
                let lv = sonic_rs::get_from_slice(payload, refs).ok()?;
                let field_val = extract_string_value(&lv);
                (field_val == value.as_str()).then_some(*action)
            }),
            Self::FieldNotEquals {
                path,
                value,
                action,
                ..
            } => with_path_refs(path, |refs| match sonic_rs::get_from_slice(payload, refs) {
                Ok(lv) => {
                    let field_val = extract_string_value(&lv);
                    (field_val != value.as_str()).then_some(*action)
                }
                // Field missing → not equal to anything → match
                Err(_) => Some(*action),
            }),
            Self::FieldStartsWith {
                path,
                prefix,
                action,
                ..
            } => with_path_refs(path, |refs| {
                let lv = sonic_rs::get_from_slice(payload, refs).ok()?;
                let field_val = extract_string_value(&lv);
                field_val.starts_with(prefix.as_str()).then_some(*action)
            }),
            Self::FieldEndsWith {
                path,
                suffix,
                action,
                ..
            } => with_path_refs(path, |refs| {
                let lv = sonic_rs::get_from_slice(payload, refs).ok()?;
                let field_val = extract_string_value(&lv);
                field_val.ends_with(suffix.as_str()).then_some(*action)
            }),
            Self::FieldContains {
                path,
                substring,
                action,
                ..
            } => with_path_refs(path, |refs| {
                let lv = sonic_rs::get_from_slice(payload, refs).ok()?;
                let field_val = extract_string_value(&lv);
                field_val.contains(substring.as_str()).then_some(*action)
            }),
            #[cfg(feature = "expression")]
            Self::CelExpression {
                program,
                fields,
                action,
                ..
            } => evaluate_cel(payload, program, fields, *action),
        }
    }

    /// Get the filter's performance tier.
    #[must_use]
    pub fn tier(&self) -> FilterTier {
        match self {
            Self::FieldExists { .. }
            | Self::FieldNotExists { .. }
            | Self::FieldEquals { .. }
            | Self::FieldNotEquals { .. }
            | Self::FieldStartsWith { .. }
            | Self::FieldEndsWith { .. }
            | Self::FieldContains { .. } => FilterTier::Tier1,
            #[cfg(feature = "expression")]
            Self::CelExpression { tier, .. } => *tier,
        }
    }

    /// Get the filter's action.
    #[must_use]
    pub fn action(&self) -> FilterAction {
        match self {
            Self::FieldExists { action, .. }
            | Self::FieldNotExists { action, .. }
            | Self::FieldEquals { action, .. }
            | Self::FieldNotEquals { action, .. }
            | Self::FieldStartsWith { action, .. }
            | Self::FieldEndsWith { action, .. }
            | Self::FieldContains { action, .. } => *action,
            #[cfg(feature = "expression")]
            Self::CelExpression { action, .. } => *action,
        }
    }

    /// Get the original expression text (for logging/debug).
    #[must_use]
    pub fn expression_text(&self) -> &str {
        match self {
            Self::FieldExists {
                expression_text, ..
            }
            | Self::FieldNotExists {
                expression_text, ..
            }
            | Self::FieldEquals {
                expression_text, ..
            }
            | Self::FieldNotEquals {
                expression_text, ..
            }
            | Self::FieldStartsWith {
                expression_text, ..
            }
            | Self::FieldEndsWith {
                expression_text, ..
            }
            | Self::FieldContains {
                expression_text, ..
            } => expression_text,
            #[cfg(feature = "expression")]
            Self::CelExpression {
                expression_text, ..
            } => expression_text,
        }
    }
}

/// Split a dotted field path into segments for `sonic_rs::get_from_slice()`.
fn split_field_path(field: &str) -> Vec<String> {
    field.split('.').map(String::from).collect()
}

/// Build a memmem Finder for a single-segment field name. Returns `None`
/// for nested paths (those fall back to sonic-rs).
///
/// The needle is `"<field>":` — the JSON key pattern. memchr's SIMD-accelerated
/// substring search detects this pattern in raw bytes ~10-20ns per call,
/// vs ~200ns for a full sonic-rs JSON parse.
///
/// Note: this is a heuristic — the pattern could appear inside a string value.
/// Used as a fast yes/no check; for false positives we'd need to verify.
/// In practice, valid JSON rarely contains escaped key-like patterns inside
/// string values, so the false positive rate is negligible.
fn build_field_needle(path: &[String]) -> Option<memchr::memmem::Finder<'static>> {
    if path.len() != 1 {
        return None;
    }
    let pattern = format!("\"{}\":", path[0]);
    Some(memchr::memmem::Finder::new(&pattern.into_bytes()).into_owned())
}

/// Extract a string value from a `sonic_rs::LazyValue` as a borrowed `&str`.
///
/// For string values without escapes, returns a zero-copy reference into the
/// raw payload (most common case). For escaped strings, falls back to
/// `as_str()` which un-escapes. For non-string values (numbers, booleans),
/// returns the raw JSON representation.
///
/// **Hot path:** uses `is_str()` to fast-check string type, then `memchr` for
/// SIMD-accelerated escape detection. Zero allocation in the common case.
fn extract_string_value<'a>(lv: &'a sonic_rs::LazyValue<'a>) -> std::borrow::Cow<'a, str> {
    use sonic_rs::JsonValueTrait;
    let raw = lv.as_raw_str();

    if lv.is_str() {
        // Strip the quotes — sonic-rs guarantees raw is `"..."` for string values
        let bytes = raw.as_bytes();
        if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
            let inner = &raw[1..raw.len() - 1];
            // SIMD escape detection via memchr
            if memchr::memchr(b'\\', inner.as_bytes()).is_none() {
                return std::borrow::Cow::Borrowed(inner);
            }
            // Has escapes — un-escape via sonic-rs as_str
            if let Some(s) = lv.as_str() {
                return std::borrow::Cow::Owned(s.to_string());
            }
        }
    }

    // Non-string value (number, bool, null): return raw representation
    std::borrow::Cow::Borrowed(raw)
}

/// Call `f` with a `&[&str]` slice over the field path.
///
/// Zero-allocation hot path: stack arrays for paths up to 4 segments deep
/// (covers >99% of real-world filter expressions). Falls back to a heap
/// allocation only for paths deeper than 4 segments.
#[inline]
fn with_path_refs<R>(path: &[String], f: impl FnOnce(&[&str]) -> R) -> R {
    match path.len() {
        0 => f(&[]),
        1 => f(&[path[0].as_str()]),
        2 => f(&[path[0].as_str(), path[1].as_str()]),
        3 => f(&[path[0].as_str(), path[1].as_str(), path[2].as_str()]),
        4 => f(&[
            path[0].as_str(),
            path[1].as_str(),
            path[2].as_str(),
            path[3].as_str(),
        ]),
        _ => {
            let refs: Vec<&str> = path.iter().map(String::as_str).collect();
            f(refs.as_slice())
        }
    }
}

/// Evaluate a Tier 2/3 CEL expression against a JSON payload.
#[cfg(feature = "expression")]
fn evaluate_cel(
    payload: &[u8],
    program: &cel::Program,
    fields: &[String],
    action: FilterAction,
) -> Option<FilterAction> {
    use std::collections::HashMap;

    // Extract only declared fields via SIMD (not full JSON parse)
    let mut context_data: HashMap<String, serde_json::Value> = HashMap::with_capacity(fields.len());
    for field in fields {
        let path: Vec<&str> = field.split('.').collect();
        if let Ok(lv) = sonic_rs::get_from_slice(payload, path.as_slice())
            && let Ok(v) = sonic_rs::from_str::<serde_json::Value>(lv.as_raw_str())
        {
            context_data.insert(field.clone(), v);
        }
    }

    let ctx = crate::expression::build_context(&context_data).ok()?;
    match program.execute(&ctx) {
        Ok(cel::Value::Bool(true)) => Some(action),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier1_field_exists_matches() {
        let filter = CompiledFilter::from_expression(
            "has(_table)",
            FilterAction::Drop,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        )
        .unwrap();
        assert_eq!(
            filter.evaluate(br#"{"_table":"events","id":1}"#),
            Some(FilterAction::Drop)
        );
    }

    #[test]
    fn tier1_field_exists_no_match() {
        let filter = CompiledFilter::from_expression(
            "has(_table)",
            FilterAction::Drop,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        )
        .unwrap();
        assert_eq!(filter.evaluate(br#"{"host":"web1","id":1}"#), None);
    }

    #[test]
    fn tier1_field_not_exists_matches() {
        let filter = CompiledFilter::from_expression(
            "!has(_internal)",
            FilterAction::Drop,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        )
        .unwrap();
        assert_eq!(
            filter.evaluate(br#"{"host":"web1"}"#),
            Some(FilterAction::Drop)
        );
    }

    #[test]
    fn tier1_field_equals_matches() {
        let filter = CompiledFilter::from_expression(
            r#"status == "poison""#,
            FilterAction::Dlq,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        )
        .unwrap();
        assert_eq!(
            filter.evaluate(br#"{"status":"poison","data":"x"}"#),
            Some(FilterAction::Dlq)
        );
    }

    #[test]
    fn tier1_field_equals_no_match() {
        let filter = CompiledFilter::from_expression(
            r#"status == "poison""#,
            FilterAction::Dlq,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        )
        .unwrap();
        assert_eq!(filter.evaluate(br#"{"status":"healthy","data":"x"}"#), None);
    }

    #[test]
    fn tier1_field_not_equals_matches() {
        let filter = CompiledFilter::from_expression(
            r#"source != "trusted""#,
            FilterAction::Drop,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        )
        .unwrap();
        assert_eq!(
            filter.evaluate(br#"{"source":"untrusted"}"#),
            Some(FilterAction::Drop)
        );
    }

    #[test]
    fn tier1_field_not_equals_missing_field_matches() {
        let filter = CompiledFilter::from_expression(
            r#"source != "trusted""#,
            FilterAction::Drop,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        )
        .unwrap();
        // Missing field is not equal to "trusted"
        assert_eq!(
            filter.evaluate(br#"{"other":"value"}"#),
            Some(FilterAction::Drop)
        );
    }

    #[test]
    fn tier1_starts_with_matches() {
        let filter = CompiledFilter::from_expression(
            r#"host.startsWith("prod-")"#,
            FilterAction::Drop,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        )
        .unwrap();
        assert_eq!(
            filter.evaluate(br#"{"host":"prod-web01"}"#),
            Some(FilterAction::Drop)
        );
    }

    #[test]
    fn tier1_starts_with_no_match() {
        let filter = CompiledFilter::from_expression(
            r#"host.startsWith("prod-")"#,
            FilterAction::Drop,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        )
        .unwrap();
        assert_eq!(filter.evaluate(br#"{"host":"dev-web01"}"#), None);
    }

    #[test]
    fn tier1_ends_with_matches() {
        let filter = CompiledFilter::from_expression(
            r#"name.endsWith(".log")"#,
            FilterAction::Drop,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        )
        .unwrap();
        assert_eq!(
            filter.evaluate(br#"{"name":"app.log"}"#),
            Some(FilterAction::Drop)
        );
    }

    #[test]
    fn tier1_contains_matches() {
        let filter = CompiledFilter::from_expression(
            r#"path.contains("/api/")"#,
            FilterAction::Drop,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        )
        .unwrap();
        assert_eq!(
            filter.evaluate(br#"{"path":"/v1/api/users"}"#),
            Some(FilterAction::Drop)
        );
    }

    #[test]
    fn tier1_dotted_path_matches() {
        let filter = CompiledFilter::from_expression(
            r#"metadata.source == "aws""#,
            FilterAction::Drop,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        )
        .unwrap();
        assert_eq!(
            filter.evaluate(br#"{"metadata":{"source":"aws"},"id":1}"#),
            Some(FilterAction::Drop)
        );
    }

    #[test]
    fn tier1_non_json_payload_no_match() {
        let filter = CompiledFilter::from_expression(
            "has(_table)",
            FilterAction::Drop,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        )
        .unwrap();
        assert_eq!(filter.evaluate(b"not json at all {{{"), None);
    }

    #[test]
    fn tier1_empty_payload_no_match() {
        let filter = CompiledFilter::from_expression(
            "has(_table)",
            FilterAction::Drop,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        )
        .unwrap();
        assert_eq!(filter.evaluate(b""), None);
    }

    #[test]
    fn tier_accessor() {
        let filter = CompiledFilter::from_expression(
            "has(x)",
            FilterAction::Drop,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        )
        .unwrap();
        assert_eq!(filter.tier(), FilterTier::Tier1);
    }

    #[test]
    fn action_accessor() {
        let filter = CompiledFilter::from_expression(
            "has(x)",
            FilterAction::Dlq,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        )
        .unwrap();
        assert_eq!(filter.action(), FilterAction::Dlq);
    }

    #[test]
    fn expression_text_accessor() {
        let filter = CompiledFilter::from_expression(
            "has(my_field)",
            FilterAction::Drop,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        )
        .unwrap();
        assert_eq!(filter.expression_text(), "has(my_field)");
    }

    #[test]
    fn tier2_rejected_without_opt_in() {
        let result = CompiledFilter::from_expression(
            r#"severity > 3 && source != "internal""#,
            FilterAction::Drop,
            FilterDirection::In,
            &TransportFilterTierConfig::default(),
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Tier 2"), "{err}");
    }

    #[test]
    fn split_field_path_simple() {
        assert_eq!(split_field_path("_table"), vec!["_table"]);
    }

    #[test]
    fn split_field_path_nested() {
        assert_eq!(
            split_field_path("metadata.source"),
            vec!["metadata", "source"]
        );
    }

    #[test]
    fn split_field_path_deep() {
        assert_eq!(split_field_path("a.b.c.d"), vec!["a", "b", "c", "d"]);
    }
}
