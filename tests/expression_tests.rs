// Project:   hyperi-rustlib
// File:      tests/expression_tests.rs
// Purpose:   Integration tests for CEL expression module
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED
//
// Run with: cargo test --test expression_tests --features expression

#![cfg(feature = "expression")]

use std::collections::HashMap;

use serde_json::json;

use hyperi_rustlib::expression::{
    compile, evaluate, evaluate_condition, validate, ExpressionError, ALLOWED_FUNCTIONS,
    DISALLOWED_FUNCTIONS,
};

// ── validate() ────────────────────────────────────────────────

#[test]
fn validate_valid_comparison() {
    assert!(validate(r#"severity == "critical""#).is_empty());
}

#[test]
fn validate_valid_numeric() {
    assert!(validate("amount > 10000").is_empty());
}

#[test]
fn validate_valid_logical() {
    assert!(validate("a > 1 && b < 10").is_empty());
}

#[test]
fn validate_valid_membership() {
    assert!(validate(r#"status in ["active", "pending"]"#).is_empty());
}

#[test]
fn validate_valid_string_function() {
    assert!(validate(r#"msg.contains("error")"#).is_empty());
}

#[test]
fn validate_valid_starts_with() {
    assert!(validate(r#"path.startsWith("/api/")"#).is_empty());
}

#[test]
fn validate_valid_ends_with() {
    assert!(validate(r#"file.endsWith(".log")"#).is_empty());
}

#[test]
fn validate_valid_matches() {
    assert!(validate(r#"name.matches("^web-[0-9]+$")"#).is_empty());
}

#[test]
fn validate_valid_has() {
    assert!(validate("has(user.name)").is_empty());
}

#[test]
fn validate_valid_size() {
    assert!(validate("size(tags) > 0").is_empty());
}

#[test]
fn validate_valid_ternary() {
    assert!(validate("is_admin ? 95 : 50").is_empty());
}

#[test]
fn validate_valid_type_cast() {
    assert!(validate("int(x) > 10").is_empty());
}

#[test]
fn validate_valid_arithmetic() {
    assert!(validate("price * quantity > threshold").is_empty());
}

#[test]
fn validate_valid_boolean_literal() {
    assert!(validate("enabled == true").is_empty());
}

#[test]
fn validate_valid_null_check() {
    assert!(validate("x == null").is_empty());
}

#[test]
fn validate_valid_compound() {
    assert!(validate(r#"severity == "critical" && amount > 10000 && !is_test"#).is_empty());
}

#[test]
fn validate_empty_expression() {
    let errors = validate("");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].to_lowercase().contains("empty"));
}

#[test]
fn validate_whitespace_only() {
    let errors = validate("   ");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].to_lowercase().contains("empty"));
}

#[test]
fn validate_disallowed_map() {
    let errors = validate("[1,2,3].map(x, x * 2)");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("map()"));
    assert!(errors[0].contains("not allowed"));
}

#[test]
fn validate_disallowed_filter() {
    let errors = validate("[1,2,3].filter(x, x > 1)");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("filter()"));
}

#[test]
fn validate_disallowed_exists() {
    let errors = validate("[1,2,3].exists(x, x > 2)");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("exists()"));
}

#[test]
fn validate_disallowed_timestamp() {
    let errors = validate(r#"timestamp("2024-01-01T00:00:00Z")"#);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("timestamp()"));
}

#[test]
fn validate_disallowed_duration() {
    let errors = validate(r#"duration("1h")"#);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("duration()"));
}

// ── evaluate() ────────────────────────────────────────────────

#[test]
fn evaluate_arithmetic() {
    let data = HashMap::new();
    let result = evaluate("1 + 2", &data).unwrap();
    assert_eq!(result, 3.into());
}

#[test]
fn evaluate_string_comparison_true() {
    let mut data = HashMap::new();
    data.insert("severity".into(), json!("critical"));
    let result = evaluate(r#"severity == "critical""#, &data).unwrap();
    assert_eq!(result, true.into());
}

#[test]
fn evaluate_string_comparison_false() {
    let mut data = HashMap::new();
    data.insert("severity".into(), json!("low"));
    let result = evaluate(r#"severity == "critical""#, &data).unwrap();
    assert_eq!(result, false.into());
}

#[test]
fn evaluate_numeric_gt() {
    let mut data = HashMap::new();
    data.insert("amount".into(), json!(15000));
    let result = evaluate("amount > 10000", &data).unwrap();
    assert_eq!(result, true.into());
}

#[test]
fn evaluate_numeric_gt_false() {
    let mut data = HashMap::new();
    data.insert("amount".into(), json!(5000));
    let result = evaluate("amount > 10000", &data).unwrap();
    assert_eq!(result, false.into());
}

#[test]
fn evaluate_logical_and() {
    let mut data = HashMap::new();
    data.insert("a".into(), json!(5));
    data.insert("b".into(), json!(3));
    let result = evaluate("a > 1 && b < 10", &data).unwrap();
    assert_eq!(result, true.into());
}

#[test]
fn evaluate_logical_or() {
    let mut data = HashMap::new();
    data.insert("a".into(), json!(5));
    data.insert("b".into(), json!(3));
    let result = evaluate("a > 100 || b < 10", &data).unwrap();
    assert_eq!(result, true.into());
}

#[test]
fn evaluate_logical_not() {
    let mut data = HashMap::new();
    data.insert("is_test".into(), json!(false));
    let result = evaluate("!is_test", &data).unwrap();
    assert_eq!(result, true.into());
}

#[test]
fn evaluate_membership_in() {
    let mut data = HashMap::new();
    data.insert("status".into(), json!("active"));
    let result = evaluate(r#"status in ["active", "pending"]"#, &data).unwrap();
    assert_eq!(result, true.into());
}

#[test]
fn evaluate_membership_in_false() {
    let mut data = HashMap::new();
    data.insert("status".into(), json!("blocked"));
    let result = evaluate(r#"status in ["active", "pending"]"#, &data).unwrap();
    assert_eq!(result, false.into());
}

#[test]
fn evaluate_string_contains() {
    let mut data = HashMap::new();
    data.insert("msg".into(), json!("an error occurred"));
    let result = evaluate(r#"msg.contains("error")"#, &data).unwrap();
    assert_eq!(result, true.into());
}

#[test]
fn evaluate_string_starts_with() {
    let mut data = HashMap::new();
    data.insert("path".into(), json!("/api/v1/users"));
    let result = evaluate(r#"path.startsWith("/api/")"#, &data).unwrap();
    assert_eq!(result, true.into());
}

#[test]
fn evaluate_string_ends_with() {
    let mut data = HashMap::new();
    data.insert("file".into(), json!("app.log"));
    let result = evaluate(r#"file.endsWith(".log")"#, &data).unwrap();
    assert_eq!(result, true.into());
}

#[test]
fn evaluate_string_matches_regex() {
    let mut data = HashMap::new();
    data.insert("name".into(), json!("web-42"));
    let result = evaluate(r#"name.matches("^web-[0-9]+$")"#, &data).unwrap();
    assert_eq!(result, true.into());
}

#[test]
fn evaluate_size_list() {
    let mut data = HashMap::new();
    data.insert("tags".into(), json!(["a", "b"]));
    let result = evaluate("size(tags) > 0", &data).unwrap();
    assert_eq!(result, true.into());
}

#[test]
fn evaluate_ternary_true() {
    let mut data = HashMap::new();
    data.insert("is_admin".into(), json!(true));
    let result = evaluate("is_admin ? 95 : 50", &data).unwrap();
    assert_eq!(result, 95.into());
}

#[test]
fn evaluate_ternary_false() {
    let mut data = HashMap::new();
    data.insert("is_admin".into(), json!(false));
    let result = evaluate("is_admin ? 95 : 50", &data).unwrap();
    assert_eq!(result, 50.into());
}

#[test]
fn evaluate_boolean_true() {
    let mut data = HashMap::new();
    data.insert("enabled".into(), json!(true));
    let result = evaluate("enabled == true", &data).unwrap();
    assert_eq!(result, true.into());
}

#[test]
fn evaluate_null_check() {
    let mut data = HashMap::new();
    data.insert("x".into(), json!(null));
    let result = evaluate("x == null", &data).unwrap();
    assert_eq!(result, true.into());
}

#[test]
fn evaluate_compound_condition() {
    let mut data = HashMap::new();
    data.insert("severity".into(), json!("critical"));
    data.insert("amount".into(), json!(15000));
    let result = evaluate(r#"severity == "critical" && amount > 10000"#, &data).unwrap();
    assert_eq!(result, true.into());
}

#[test]
fn evaluate_missing_field_errors() {
    let data = HashMap::new();
    let result = evaluate(r#"severity == "critical""#, &data);
    assert!(result.is_err());
}

#[test]
fn evaluate_invalid_expression_errors() {
    let data = HashMap::new();
    let result = evaluate("== broken", &data);
    assert!(result.is_err());
}

#[test]
fn evaluate_disallowed_function_errors() {
    let data = HashMap::new();
    let result = evaluate("[1,2].map(x, x * 2)", &data);
    assert!(result.is_err());
}

// ── evaluate_condition() ──────────────────────────────────────

#[test]
fn condition_match() {
    let mut data = HashMap::new();
    data.insert("severity".into(), json!("critical"));
    assert!(evaluate_condition(r#"severity == "critical""#, &data));
}

#[test]
fn condition_no_match() {
    let mut data = HashMap::new();
    data.insert("severity".into(), json!("low"));
    assert!(!evaluate_condition(r#"severity == "critical""#, &data));
}

#[test]
fn condition_missing_field_returns_false() {
    let data = HashMap::new();
    assert!(!evaluate_condition(r#"severity == "critical""#, &data));
}

#[test]
fn condition_type_mismatch_returns_false() {
    let mut data = HashMap::new();
    data.insert("amount".into(), json!("not_a_number"));
    assert!(!evaluate_condition("amount > 10", &data));
}

#[test]
fn condition_invalid_expression_returns_false() {
    let data = HashMap::new();
    assert!(!evaluate_condition("== broken", &data));
}

#[test]
fn condition_empty_expression_returns_false() {
    let data = HashMap::new();
    assert!(!evaluate_condition("", &data));
}

#[test]
fn condition_compound() {
    let mut data = HashMap::new();
    data.insert("severity".into(), json!("critical"));
    data.insert("amount".into(), json!(15000));
    assert!(evaluate_condition(
        r#"severity == "critical" && amount > 10000"#,
        &data
    ));
}

#[test]
fn condition_compound_partial_match() {
    let mut data = HashMap::new();
    data.insert("severity".into(), json!("critical"));
    data.insert("amount".into(), json!(5000));
    assert!(!evaluate_condition(
        r#"severity == "critical" && amount > 10000"#,
        &data
    ));
}

#[test]
fn condition_in_membership() {
    let mut data = HashMap::new();
    data.insert("status".into(), json!("active"));
    assert!(evaluate_condition(
        r#"status in ["active", "pending"]"#,
        &data
    ));
}

#[test]
fn condition_negated_in() {
    let mut data = HashMap::new();
    data.insert("status".into(), json!("active"));
    assert!(evaluate_condition(
        r#"!(status in ["blocked", "banned"])"#,
        &data
    ));
}

#[test]
fn condition_numeric_comparison() {
    let mut data = HashMap::new();
    data.insert("amount".into(), json!(15000));
    assert!(evaluate_condition("amount > 10000", &data));
}

#[test]
fn condition_ternary_truthy() {
    let mut data = HashMap::new();
    data.insert("is_admin".into(), json!(true));
    // Ternary returns 95 (non-zero int) → truthy
    assert!(evaluate_condition("is_admin ? 95 : 0", &data));
}

#[test]
fn condition_ternary_falsy() {
    let mut data = HashMap::new();
    data.insert("is_admin".into(), json!(true));
    // Ternary returns 0 → falsy
    assert!(!evaluate_condition("is_admin ? 0 : 50", &data));
}

// ── compile() ─────────────────────────────────────────────────

#[test]
fn compile_and_execute() {
    let program = compile("price * quantity > threshold").unwrap();
    let data = HashMap::from([
        ("price".into(), json!(10)),
        ("quantity".into(), json!(5)),
        ("threshold".into(), json!(40)),
    ]);
    let ctx = hyperi_rustlib::expression::build_context(&data).unwrap();
    let result = program.execute(&ctx).unwrap();
    assert_eq!(result, true.into());
}

#[test]
fn compile_reuse() {
    let program = compile(r#"severity == "critical""#).unwrap();

    let data1 = HashMap::from([("severity".into(), json!("critical"))]);
    let ctx1 = hyperi_rustlib::expression::build_context(&data1).unwrap();
    assert_eq!(program.execute(&ctx1).unwrap(), true.into());

    let data2 = HashMap::from([("severity".into(), json!("low"))]);
    let ctx2 = hyperi_rustlib::expression::build_context(&data2).unwrap();
    assert_eq!(program.execute(&ctx2).unwrap(), false.into());
}

#[test]
fn compile_invalid_raises() {
    assert!(compile("== broken").is_err());
}

#[test]
fn compile_disallowed_raises() {
    assert!(compile("[1,2].map(x, x*2)").is_err());
}

#[test]
fn compile_empty_raises() {
    assert!(compile("").is_err());
}

// ── Profile ───────────────────────────────────────────────────

#[test]
fn allowed_functions_contains_core() {
    for f in &[
        "contains",
        "startsWith",
        "endsWith",
        "matches",
        "size",
        "has",
    ] {
        assert!(ALLOWED_FUNCTIONS.contains(f), "missing allowed: {f}");
    }
}

#[test]
fn allowed_functions_contains_casts() {
    for f in &["int", "uint", "double", "string", "bool"] {
        assert!(ALLOWED_FUNCTIONS.contains(f), "missing allowed: {f}");
    }
}

#[test]
fn disallowed_functions_present() {
    for f in &[
        "map",
        "filter",
        "exists",
        "all",
        "exists_one",
        "timestamp",
        "duration",
    ] {
        assert!(DISALLOWED_FUNCTIONS.contains(f), "missing disallowed: {f}");
    }
}

#[test]
fn no_overlap_between_allowed_and_disallowed() {
    for f in ALLOWED_FUNCTIONS {
        assert!(
            !DISALLOWED_FUNCTIONS.contains(f),
            "overlap: {f} in both allowed and disallowed"
        );
    }
}

// ── ExpressionError ───────────────────────────────────────────

#[test]
fn error_validation_display() {
    let err = ExpressionError::Validation(vec!["error one".into(), "error two".into()]);
    let msg = format!("{err}");
    assert!(msg.contains("error one"));
    assert!(msg.contains("error two"));
}

#[test]
fn error_compilation_display() {
    let err = ExpressionError::Compilation("bad syntax".into());
    let msg = format!("{err}");
    assert!(msg.contains("bad syntax"));
}
