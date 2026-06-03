// Project:   hyperi-rustlib
// File:      src/transport/filter/budget.rs
// Purpose:   Static + runtime budget for Tier 2/3 CEL filters
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Static AST + runtime payload budget for Tier 2/3 CEL filters.
//!
//! Gate 1 walks the parsed CEL AST at config-load and rejects filters
//! whose node count or iteration nesting exceeds the budget. Gate 2 caps
//! the payload size at evaluation. Wall-clock budgets (Gate 3) need
//! upstream `cel` crate support and stay out of scope.

use serde::{Deserialize, Serialize};

/// Budget knobs for Tier 2/3 CEL filters.
///
/// Lives under the `transport.filter_tiers.budget` cascade key.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FilterBudget {
    /// Max AST nodes after parse.
    #[serde(default = "default_max_ast_nodes")]
    pub max_ast_nodes: usize,

    /// Max nested iteration depth (`filter`, `map`, `exists`, `all`).
    #[serde(default = "default_max_iteration_depth")]
    pub max_iteration_depth: u8,

    /// Max payload bytes accepted at Tier 2/3 evaluation.
    #[serde(default = "default_max_payload_bytes")]
    pub max_payload_bytes: usize,
}

const fn default_max_ast_nodes() -> usize {
    200
}
const fn default_max_iteration_depth() -> u8 {
    2
}
const fn default_max_payload_bytes() -> usize {
    1024 * 1024
}

impl Default for FilterBudget {
    fn default() -> Self {
        Self {
            max_ast_nodes: default_max_ast_nodes(),
            max_iteration_depth: default_max_iteration_depth(),
            max_payload_bytes: default_max_payload_bytes(),
        }
    }
}

/// Budget violation discovered at config-load or evaluation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BudgetError {
    /// AST exceeded `max_ast_nodes`.
    #[error("CEL filter AST has {count} nodes, exceeds budget {limit}")]
    TooManyNodes { count: usize, limit: usize },
    /// Iteration nesting exceeded `max_iteration_depth`.
    #[error("CEL filter iteration nests to depth {depth}, exceeds budget {limit}")]
    IterationTooDeep { depth: u8, limit: u8 },
    /// Payload bytes exceeded `max_payload_bytes`.
    #[error("CEL filter payload is {size} bytes, exceeds budget {limit}")]
    PayloadTooLarge { size: usize, limit: usize },
    /// CEL parse failed during budget check.
    #[error("CEL filter parse failed: {0}")]
    Parse(String),
}

/// Static budget check on the CEL AST.
///
/// Returns `Err` if the parsed expression breaches `max_ast_nodes` or
/// `max_iteration_depth`. Called once per filter at config-load.
#[cfg(feature = "expression")]
pub fn check_static_budget(expression: &str, budget: &FilterBudget) -> Result<(), BudgetError> {
    // `Program::compile` parses + holds the AST. We re-parse here
    // (cheap, config-load only) rather than thread the parsed AST
    // back out of `crate::expression::compile`.
    let program =
        cel::Program::compile(expression).map_err(|e| BudgetError::Parse(format!("{e:?}")))?;

    let mut nodes = 0usize;
    let mut max_depth = 0u8;
    walk(&program.expression().expr, 0, &mut nodes, &mut max_depth);

    if nodes > budget.max_ast_nodes {
        return Err(BudgetError::TooManyNodes {
            count: nodes,
            limit: budget.max_ast_nodes,
        });
    }
    if max_depth > budget.max_iteration_depth {
        return Err(BudgetError::IterationTooDeep {
            depth: max_depth,
            limit: budget.max_iteration_depth,
        });
    }
    Ok(())
}

/// Payload-size guard at evaluation time.
#[inline]
#[must_use]
pub fn check_payload_budget(size: usize, budget: &FilterBudget) -> Result<(), BudgetError> {
    if size > budget.max_payload_bytes {
        Err(BudgetError::PayloadTooLarge {
            size,
            limit: budget.max_payload_bytes,
        })
    } else {
        Ok(())
    }
}

#[cfg(feature = "expression")]
fn walk(expr: &cel::common::ast::Expr, depth: u8, nodes: &mut usize, max_depth: &mut u8) {
    use cel::common::ast::Expr;
    *nodes += 1;
    match expr {
        Expr::Call(c) => {
            if let Some(t) = &c.target {
                walk(&t.expr, depth, nodes, max_depth);
            }
            for a in &c.args {
                walk(&a.expr, depth, nodes, max_depth);
            }
        }
        Expr::Comprehension(c) => {
            let d = depth.saturating_add(1);
            if d > *max_depth {
                *max_depth = d;
            }
            walk(&c.iter_range.expr, d, nodes, max_depth);
            walk(&c.accu_init.expr, d, nodes, max_depth);
            walk(&c.loop_cond.expr, d, nodes, max_depth);
            walk(&c.loop_step.expr, d, nodes, max_depth);
            walk(&c.result.expr, d, nodes, max_depth);
        }
        Expr::List(l) => {
            for e in &l.elements {
                walk(&e.expr, depth, nodes, max_depth);
            }
        }
        Expr::Map(m) => {
            for entry in &m.entries {
                walk_entry(entry, depth, nodes, max_depth);
            }
        }
        Expr::Select(s) => walk(&s.operand.expr, depth, nodes, max_depth),
        Expr::Struct(s) => {
            for entry in &s.entries {
                walk_entry(entry, depth, nodes, max_depth);
            }
        }
        Expr::Ident(_) | Expr::Literal(_) | Expr::Unspecified => {}
    }
}

#[cfg(feature = "expression")]
fn walk_entry(
    entry: &cel::common::ast::IdedEntryExpr,
    depth: u8,
    nodes: &mut usize,
    max_depth: &mut u8,
) {
    use cel::common::ast::EntryExpr;
    *nodes += 1;
    match &entry.expr {
        EntryExpr::StructField(f) => walk(&f.value.expr, depth, nodes, max_depth),
        EntryExpr::MapEntry(m) => {
            walk(&m.key.expr, depth, nodes, max_depth);
            walk(&m.value.expr, depth, nodes, max_depth);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_permissive_for_typical_filters() {
        let b = FilterBudget::default();
        assert_eq!(b.max_ast_nodes, 200);
        assert_eq!(b.max_iteration_depth, 2);
        assert_eq!(b.max_payload_bytes, 1024 * 1024);
    }

    #[test]
    fn payload_guard_accepts_under_cap() {
        let b = FilterBudget::default();
        assert!(check_payload_budget(1024, &b).is_ok());
    }

    #[test]
    fn payload_guard_rejects_over_cap() {
        let b = FilterBudget {
            max_payload_bytes: 100,
            ..Default::default()
        };
        let err = check_payload_budget(101, &b).unwrap_err();
        assert!(matches!(err, BudgetError::PayloadTooLarge { .. }));
    }

    #[cfg(feature = "expression")]
    #[test]
    fn static_budget_accepts_simple_filter() {
        let b = FilterBudget::default();
        // `has(...)` needs a select expr per CEL spec.
        assert!(check_static_budget("has(obj.foo)", &b).is_ok());
        assert!(check_static_budget(r#"status == "poison""#, &b).is_ok());
    }

    #[cfg(feature = "expression")]
    #[test]
    fn static_budget_counts_nodes() {
        let b = FilterBudget {
            max_ast_nodes: 3,
            ..Default::default()
        };
        // `a + b + c` parses to 5+ nodes (3 idents, 2 calls).
        let err = check_static_budget("a + b + c", &b).unwrap_err();
        assert!(matches!(err, BudgetError::TooManyNodes { .. }));
    }

    #[cfg(feature = "expression")]
    #[test]
    fn static_budget_rejects_deep_iteration() {
        let b = FilterBudget {
            max_iteration_depth: 1,
            ..Default::default()
        };
        // Two levels of filter nesting.
        let err = check_static_budget(
            "items.filter(x, x.tags.filter(t, t == 'a').size() > 0).size() > 0",
            &b,
        )
        .unwrap_err();
        assert!(matches!(err, BudgetError::IterationTooDeep { .. }));
    }

    #[cfg(feature = "expression")]
    #[test]
    fn static_budget_accepts_single_iteration() {
        let b = FilterBudget::default();
        assert!(check_static_budget("items.exists(x, x == 'a')", &b).is_ok());
    }

    #[cfg(feature = "expression")]
    #[test]
    fn static_budget_surfaces_parse_errors() {
        let b = FilterBudget::default();
        let err = check_static_budget("(((", &b).unwrap_err();
        assert!(matches!(err, BudgetError::Parse(_)));
    }
}
