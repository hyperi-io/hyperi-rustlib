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
/// field extraction via `sonic_rs::get_from_slice()`.
#[derive(Debug)]
pub enum CompiledFilter {
    // Tier 1 — SIMD field ops
    FieldExists {
        field: String,
        path: Vec<String>,
        action: FilterAction,
        expression_text: String,
    },
    FieldNotExists {
        field: String,
        path: Vec<String>,
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
        program: cel_interpreter::Program,
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
                Self::FieldExists {
                    field,
                    path,
                    action,
                    expression_text,
                }
            }
            Tier1Op::FieldNotExists { field } => {
                let path = split_field_path(&field);
                Self::FieldNotExists {
                    field,
                    path,
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
    pub fn evaluate(&self, payload: &[u8]) -> Option<FilterAction> {
        match self {
            Self::FieldExists { path, action, .. } => {
                let refs: Vec<&str> = path.iter().map(String::as_str).collect();
                if sonic_rs::get_from_slice(payload, refs.as_slice()).is_ok() {
                    Some(*action)
                } else {
                    None
                }
            }
            Self::FieldNotExists { path, action, .. } => {
                let refs: Vec<&str> = path.iter().map(String::as_str).collect();
                if sonic_rs::get_from_slice(payload, refs.as_slice()).is_err() {
                    Some(*action)
                } else {
                    None
                }
            }
            Self::FieldEquals {
                path,
                value,
                action,
                ..
            } => {
                let refs: Vec<&str> = path.iter().map(String::as_str).collect();
                match sonic_rs::get_from_slice(payload, refs.as_slice()) {
                    Ok(lv) => {
                        let field_val = extract_string_value(&lv);
                        if field_val == value.as_str() {
                            Some(*action)
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                }
            }
            Self::FieldNotEquals {
                path,
                value,
                action,
                ..
            } => {
                let refs: Vec<&str> = path.iter().map(String::as_str).collect();
                match sonic_rs::get_from_slice(payload, refs.as_slice()) {
                    Ok(lv) => {
                        let field_val = extract_string_value(&lv);
                        if field_val == value.as_str() {
                            None
                        } else {
                            Some(*action)
                        }
                    }
                    // Field missing → not equal to anything → match
                    Err(_) => Some(*action),
                }
            }
            Self::FieldStartsWith {
                path,
                prefix,
                action,
                ..
            } => {
                let refs: Vec<&str> = path.iter().map(String::as_str).collect();
                match sonic_rs::get_from_slice(payload, refs.as_slice()) {
                    Ok(lv) => {
                        let field_val = extract_string_value(&lv);
                        if field_val.starts_with(prefix.as_str()) {
                            Some(*action)
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                }
            }
            Self::FieldEndsWith {
                path,
                suffix,
                action,
                ..
            } => {
                let refs: Vec<&str> = path.iter().map(String::as_str).collect();
                match sonic_rs::get_from_slice(payload, refs.as_slice()) {
                    Ok(lv) => {
                        let field_val = extract_string_value(&lv);
                        if field_val.ends_with(suffix.as_str()) {
                            Some(*action)
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                }
            }
            Self::FieldContains {
                path,
                substring,
                action,
                ..
            } => {
                let refs: Vec<&str> = path.iter().map(String::as_str).collect();
                match sonic_rs::get_from_slice(payload, refs.as_slice()) {
                    Ok(lv) => {
                        let field_val = extract_string_value(&lv);
                        if field_val.contains(substring.as_str()) {
                            Some(*action)
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                }
            }
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

/// Extract a string value from a `sonic_rs::LazyValue`.
///
/// For string values, strips the surrounding quotes. For non-string values
/// (numbers, booleans), returns the raw JSON representation.
fn extract_string_value(lv: &sonic_rs::LazyValue<'_>) -> String {
    use sonic_rs::JsonValueTrait;
    if let Some(s) = lv.as_str() {
        s.to_string()
    } else {
        // For non-string values, use raw representation
        lv.as_raw_str().to_string()
    }
}

/// Evaluate a Tier 2/3 CEL expression against a JSON payload.
#[cfg(feature = "expression")]
fn evaluate_cel(
    payload: &[u8],
    program: &cel_interpreter::Program,
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
        Ok(cel_interpreter::Value::Bool(true)) => Some(action),
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
