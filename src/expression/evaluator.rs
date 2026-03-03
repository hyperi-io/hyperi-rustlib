// Project:   hyperi-rustlib
// File:      src/expression/evaluator.rs
// Purpose:   CEL expression compile / evaluate / validate wrappers
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

#![allow(clippy::implicit_hasher)] // Public API uses HashMap<String, JsonValue> intentionally

//! Core CEL expression operations — compile, evaluate, validate.
//!
//! Wraps the [`cel_interpreter`] crate, enforcing the DFE expression
//! profile on every compilation path. Both Python (via `common-expression-
//! language` PyO3 bindings) and Rust share the **same** `cel-interpreter`
//! Rust crate — zero behavioural drift between services.
//!
//! # Usage
//!
//! ```rust
//! use hyperi_rustlib::expression::{compile, evaluate, evaluate_condition, validate};
//! use std::collections::HashMap;
//! use serde_json::json;
//!
//! // Validate before storing (UI pre-submit)
//! assert!(validate(r#"severity == "critical""#).is_empty());
//!
//! // One-shot evaluation
//! let mut data = HashMap::new();
//! data.insert("severity".into(), json!("critical"));
//! let result = evaluate(r#"severity == "critical""#, &data).unwrap();
//! assert_eq!(result, true.into());
//!
//! // Boolean condition (missing fields → false)
//! let empty = HashMap::new();
//! assert!(!evaluate_condition(r#"severity == "critical""#, &empty));
//!
//! // Compile once, evaluate many (hot path)
//! let program = compile("amount > threshold").unwrap();
//! // ... call program.execute(&context) per record
//! ```

use std::collections::HashMap;

use cel_interpreter::{Context, Program, Value};
use serde_json::Value as JsonValue;

use super::error::{ExpressionError, ExpressionResult};
use super::profile;

// ── Validate ──────────────────────────────────────────────────────

/// Validate an expression for syntax and DFE profile compliance.
///
/// Returns a list of error strings (empty if valid).
/// Designed for UI pre-submit validation — call this before storing
/// expressions.
#[must_use]
pub fn validate(expr: &str) -> Vec<String> {
    if expr.trim().is_empty() {
        return vec!["Expression is empty".to_string()];
    }

    // Check DFE profile (disallowed functions)
    let profile_errors = profile::check_profile(expr);
    if !profile_errors.is_empty() {
        return profile_errors;
    }

    // Check syntax via CEL compiler
    match Program::compile(expr) {
        Ok(_) => vec![],
        Err(e) => vec![format!("{e}")],
    }
}

// ── Compile ───────────────────────────────────────────────────────

/// Compile a CEL expression, enforcing the DFE profile.
///
/// Use this for hot paths where the same expression is evaluated against
/// many records. Compile once, then call [`Program::execute`] per record.
///
/// # Errors
///
/// Returns [`ExpressionError::Validation`] if the expression violates the
/// DFE profile, or [`ExpressionError::Compilation`] if it has a syntax error.
pub fn compile(expr: &str) -> ExpressionResult<Program> {
    let errors = validate(expr);
    if !errors.is_empty() {
        return Err(ExpressionError::Validation(errors));
    }
    Program::compile(expr).map_err(|e| ExpressionError::Compilation(format!("{e}")))
}

// ── Evaluate ──────────────────────────────────────────────────────

/// Compile and evaluate a CEL expression in one step.
///
/// For repeated evaluation of the same expression, use [`compile`] instead.
///
/// # Errors
///
/// Returns an error if the expression is invalid, violates the DFE profile,
/// or evaluation fails (missing fields, type mismatch).
pub fn evaluate(expr: &str, data: &HashMap<String, JsonValue>) -> ExpressionResult<Value> {
    let program = compile(expr)?;
    let context = build_context(data)?;
    program
        .execute(&context)
        .map_err(|e| ExpressionError::Evaluation(format!("{e}")))
}

/// Build a CEL [`Context`] from a JSON-compatible data map.
///
/// Each key-value pair from the map is added as a top-level variable
/// in the CEL execution context. Supports all JSON types via the
/// `cel-interpreter` json feature (serde integration).
pub fn build_context(data: &HashMap<String, JsonValue>) -> ExpressionResult<Context<'_>> {
    let mut context = Context::default();
    for (key, value) in data {
        context.add_variable_from_value(key, json_to_cel(value));
    }
    Ok(context)
}

// ── Evaluate Condition ────────────────────────────────────────────

/// Evaluate a boolean condition, returning `false` on missing fields.
///
/// This is the safe evaluation mode for scoring `when` conditions,
/// alert triggers, and routing rules. If a field referenced in the
/// expression is missing from `data`, returns `false` instead of
/// returning an error.
///
/// Non-boolean results are coerced: non-zero integers are truthy,
/// zero and errors are falsy.
#[must_use]
pub fn evaluate_condition(expr: &str, data: &HashMap<String, JsonValue>) -> bool {
    match evaluate(expr, data) {
        Ok(Value::Bool(b)) => b,
        Ok(Value::Int(n)) => n != 0,
        Ok(Value::UInt(n)) => n != 0,
        Ok(Value::Float(f)) => f != 0.0,
        // Everything else (Null, String, List, Map, errors) → false
        _ => false,
    }
}

// ── Helpers ───────────────────────────────────────────────────────

/// Convert a `serde_json::Value` to a CEL `Value`.
fn json_to_cel(json: &JsonValue) -> Value {
    match json {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(b) => Value::Bool(*b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if let Some(u) = n.as_u64() {
                Value::UInt(u)
            } else {
                Value::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        JsonValue::String(s) => Value::String(s.clone().into()),
        JsonValue::Array(arr) => {
            Value::List(arr.iter().map(json_to_cel).collect::<Vec<_>>().into())
        }
        JsonValue::Object(obj) => {
            let hash: HashMap<cel_interpreter::objects::Key, Value> = obj
                .iter()
                .map(|(k, v)| (k.clone().into(), json_to_cel(v)))
                .collect();
            Value::Map(hash.into())
        }
    }
}
