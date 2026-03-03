// Project:   hyperi-rustlib
// File:      src/expression/mod.rs
// Purpose:   CEL expression evaluation for DFE components
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! CEL expression evaluation — compile, evaluate, validate.
//!
//! Provides a DFE-profile-restricted CEL expression evaluator built on the
//! [`cel_interpreter`] crate. Both Python (via `common-expression-language`
//! PyO3 bindings) and Rust use the **same** underlying Rust crate, ensuring
//! identical parsing and evaluation semantics across all DFE components.
//!
//! # DFE Expression Profile
//!
//! Only a high-performance subset of CEL is allowed:
//!
//! | Category | Examples |
//! |----------|---------|
//! | Comparison | `==`, `!=`, `<`, `<=`, `>`, `>=` |
//! | Logical | `&&`, `\|\|`, `!` |
//! | Membership | `in` |
//! | String | `contains()`, `startsWith()`, `endsWith()`, `matches()` |
//! | Existence | `has()` |
//! | Size | `size()` |
//! | Ternary | `? :` |
//! | Type casts | `int()`, `double()`, `string()`, `bool()` |
//! | Arithmetic | `+`, `-`, `*`, `/`, `%` |
//!
//! **Excluded:** `map()`, `filter()`, `exists()`, `all()` (iteration),
//! `timestamp()`, `duration()` (ClickHouse handles time natively).
//!
//! # Usage
//!
//! ```rust
//! use hyperi_rustlib::expression::{compile, evaluate, evaluate_condition, validate};
//! use std::collections::HashMap;
//! use serde_json::json;
//!
//! // Validate (returns errors list, empty if valid)
//! assert!(validate(r#"severity == "critical""#).is_empty());
//!
//! // One-shot evaluation
//! let mut data = HashMap::new();
//! data.insert("amount".into(), json!(15000));
//! let result = evaluate("amount > 10000", &data).unwrap();
//! assert_eq!(result, true.into());
//!
//! // Boolean condition (missing fields → false)
//! assert!(!evaluate_condition(r#"severity == "critical""#, &HashMap::new()));
//!
//! // Compile for hot-path reuse
//! let program = compile("score > threshold").unwrap();
//! // ... program.execute(&context) per record
//! ```
//!
//! See `dfe-engine/docs/EXPRESSIONS-CEL.md` for the full profile specification.

pub mod error;
pub mod evaluator;
pub mod profile;

pub use error::{ExpressionError, ExpressionResult};
pub use evaluator::{build_context, compile, evaluate, evaluate_condition, validate};
pub use profile::{ALLOWED_FUNCTIONS, DISALLOWED_FUNCTIONS};
