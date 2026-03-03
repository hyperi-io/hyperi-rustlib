// Project:   hyperi-rustlib
// File:      src/expression/error.rs
// Purpose:   Expression error types
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Error types for CEL expression compilation and evaluation.

use thiserror::Error;

/// Errors from expression validation, compilation, or evaluation.
#[derive(Error, Debug)]
pub enum ExpressionError {
    /// Expression failed DFE profile validation or syntax check.
    #[error("Expression validation failed: {}", .0.join("; "))]
    Validation(Vec<String>),

    /// Expression could not be compiled (syntax error).
    #[error("Expression compilation failed: {0}")]
    Compilation(String),

    /// Expression evaluation failed at runtime (missing field, type mismatch).
    #[error("Expression evaluation failed: {0}")]
    Evaluation(String),
}

/// Convenience result type for expression operations.
pub type ExpressionResult<T> = Result<T, ExpressionError>;
