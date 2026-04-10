// Project:   hyperi-rustlib
// File:      src/transport/filter/mod.rs
// Purpose:   Transport-level message filtering engine
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # Transport Filter Engine
//!
//! Provides transport-level message filtering using CEL syntax with SIMD
//! fast-path for simple patterns. Embedded in every transport — filters are
//! configured via the config cascade and applied automatically.
//!
//! ## Performance Tiers
//!
//! - **Tier 1** — SIMD field extraction (~50-100ns/msg). Always enabled.
//! - **Tier 2** — Pre-compiled CEL (~500ns-1us/msg). Requires `allow_cel_filters_in/out`.
//! - **Tier 3** — Complex CEL with regex/iteration (~5-50us/msg). Requires `allow_complex_filters_in/out`.
//!
//! ## Usage
//!
//! Transports construct `TransportFilterEngine` from config at creation time.
//! The engine is a no-op when no filters are configured (zero overhead).
//!
//! ```yaml
//! kafka:
//!   filters_in:
//!     - expression: 'has(_internal)'
//!       action: drop
//!     - expression: 'status == "poison"'
//!       action: dlq
//! ```

pub mod classify;
pub mod compiled;
pub mod config;
pub mod metrics;

pub use config::{
    FilterAction, FilterDirection, FilterRule, FilterTier, TransportFilterTierConfig,
};

use compiled::CompiledFilter;
use metrics::FilterMetrics;

use crate::transport::error::TransportError;

/// Result of evaluating a filter against a message payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterDisposition {
    /// Message passes all filters — continue processing.
    Pass,
    /// Message matched a filter with `action: drop` — discard silently.
    Drop,
    /// Message matched a filter with `action: dlq` — route to dead-letter queue.
    Dlq,
}

/// A DLQ entry produced by inbound filtering.
///
/// The transport does NOT send to DLQ directly — it returns these entries
/// alongside passing messages. The caller handles DLQ routing using its
/// own `Dlq` handle.
#[derive(Debug, Clone)]
pub struct FilteredDlqEntry {
    /// Raw message payload.
    pub payload: Vec<u8>,
    /// Routing key (Kafka topic, gRPC metadata, etc.).
    pub key: Option<std::sync::Arc<str>>,
    /// Human-readable reason for DLQ routing (filter expression text).
    pub reason: String,
}

/// Transport-level message filter engine.
///
/// Embedded in every transport. Compiled from config at construction time.
/// Zero-cost when no filters are configured (`filters_in` and `filters_out`
/// are empty vecs → `has_inbound_filters()` returns false, branch predicted).
pub struct TransportFilterEngine {
    filters_in: Vec<CompiledFilter>,
    filters_out: Vec<CompiledFilter>,
    #[allow(dead_code)]
    metrics: FilterMetrics,
}

impl std::fmt::Debug for TransportFilterEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransportFilterEngine")
            .field("filters_in_count", &self.filters_in.len())
            .field("filters_out_count", &self.filters_out.len())
            .field("metrics", &"FilterMetrics")
            .finish()
    }
}

/// Threshold for logging a warning about filter count.
const FILTER_COUNT_WARNING_THRESHOLD: usize = 20;

impl TransportFilterEngine {
    /// Construct a filter engine from config rules.
    ///
    /// Parses each expression, classifies its tier, rejects expressions above
    /// the enabled tier, and compiles Tier 2/3 expressions to CEL programs.
    ///
    /// # Errors
    ///
    /// Returns `TransportError::Config` if:
    /// - An expression is invalid (syntax error)
    /// - An expression's tier exceeds what's enabled by `tier_config`
    pub fn new(
        filters_in: &[FilterRule],
        filters_out: &[FilterRule],
        tier_config: &TransportFilterTierConfig,
    ) -> Result<Self, TransportError> {
        let compiled_in = Self::compile_rules(filters_in, FilterDirection::In, tier_config)?;
        let compiled_out = Self::compile_rules(filters_out, FilterDirection::Out, tier_config)?;

        // Log startup info
        if !compiled_in.is_empty() || !compiled_out.is_empty() {
            for (idx, filter) in compiled_in.iter().enumerate() {
                tracing::info!(
                    index = idx,
                    tier = %filter.tier(),
                    expression = filter.expression_text(),
                    action = ?filter.action(),
                    direction = "in",
                    "Transport filter configured"
                );
            }
            for (idx, filter) in compiled_out.iter().enumerate() {
                tracing::info!(
                    index = idx,
                    tier = %filter.tier(),
                    expression = filter.expression_text(),
                    action = ?filter.action(),
                    direction = "out",
                    "Transport filter configured"
                );
            }

            // Warn about ordering (higher tier before lower tier)
            Self::warn_suboptimal_ordering(&compiled_in, "in");
            Self::warn_suboptimal_ordering(&compiled_out, "out");

            // Warn about large filter counts
            if compiled_in.len() > FILTER_COUNT_WARNING_THRESHOLD {
                tracing::warn!(
                    count = compiled_in.len(),
                    direction = "in",
                    "Large number of inbound filters — may impact throughput"
                );
            }
            if compiled_out.len() > FILTER_COUNT_WARNING_THRESHOLD {
                tracing::warn!(
                    count = compiled_out.len(),
                    direction = "out",
                    "Large number of outbound filters — may impact throughput"
                );
            }
        }

        Ok(Self {
            filters_in: compiled_in,
            filters_out: compiled_out,
            metrics: FilterMetrics::new(),
        })
    }

    /// Check if any inbound filters are configured.
    #[inline]
    #[must_use]
    pub fn has_inbound_filters(&self) -> bool {
        !self.filters_in.is_empty()
    }

    /// Check if any outbound filters are configured.
    #[inline]
    #[must_use]
    pub fn has_outbound_filters(&self) -> bool {
        !self.filters_out.is_empty()
    }

    /// Check if any inbound filter uses `action: dlq`.
    #[must_use]
    pub fn has_dlq_filters_in(&self) -> bool {
        self.filters_in
            .iter()
            .any(|f| f.action() == FilterAction::Dlq)
    }

    /// Check if any outbound filter uses `action: dlq`.
    #[must_use]
    pub fn has_dlq_filters_out(&self) -> bool {
        self.filters_out
            .iter()
            .any(|f| f.action() == FilterAction::Dlq)
    }

    /// Evaluate inbound filters against a raw payload. First-match-wins.
    ///
    /// Returns `Pass` if no filter matches (or no filters configured).
    /// MsgPack payloads always pass (SIMD extraction is JSON-only).
    #[inline]
    #[must_use]
    pub fn apply_inbound(&self, payload: &[u8]) -> FilterDisposition {
        self.apply_filters(payload, &self.filters_in, FilterDirection::In)
    }

    /// Evaluate outbound filters against a raw payload. First-match-wins.
    #[inline]
    #[must_use]
    pub fn apply_outbound(&self, payload: &[u8]) -> FilterDisposition {
        self.apply_filters(payload, &self.filters_out, FilterDirection::Out)
    }

    fn apply_filters(
        &self,
        payload: &[u8],
        filters: &[CompiledFilter],
        direction: FilterDirection,
    ) -> FilterDisposition {
        if filters.is_empty() {
            return FilterDisposition::Pass;
        }

        // MsgPack payloads bypass filters (SIMD extraction is JSON-only)
        if is_likely_msgpack(payload) {
            return FilterDisposition::Pass;
        }

        for filter in filters {
            if let Some(action) = filter.evaluate(payload) {
                self.metrics.record(direction, action);
                return match action {
                    FilterAction::Drop => FilterDisposition::Drop,
                    FilterAction::Dlq => FilterDisposition::Dlq,
                };
            }
        }

        FilterDisposition::Pass
    }

    fn compile_rules(
        rules: &[FilterRule],
        direction: FilterDirection,
        tier_config: &TransportFilterTierConfig,
    ) -> Result<Vec<CompiledFilter>, TransportError> {
        let mut compiled = Vec::with_capacity(rules.len());

        for (idx, rule) in rules.iter().enumerate() {
            let filter = CompiledFilter::from_expression(
                &rule.expression,
                rule.action,
                direction,
                tier_config,
            )
            .map_err(|e| {
                TransportError::Config(format!(
                    "filter_{direction}[{idx}]: '{expr}' — {e}",
                    expr = rule.expression
                ))
            })?;
            compiled.push(filter);
        }

        Ok(compiled)
    }

    fn warn_suboptimal_ordering(filters: &[CompiledFilter], direction: &str) {
        let mut lowest_seen = FilterTier::Tier3;
        for (idx, filter) in filters.iter().enumerate() {
            let tier = filter.tier();
            if (tier as u8) > (lowest_seen as u8) {
                tracing::warn!(
                    direction,
                    index = idx,
                    tier = %tier,
                    expression = filter.expression_text(),
                    "Higher-tier filter precedes lower-tier filter — consider reordering for better performance"
                );
            }
            if (tier as u8) < (lowest_seen as u8) {
                lowest_seen = tier;
            }
        }
    }
}

/// Quick heuristic: is this payload likely MsgPack (not JSON)?
///
/// Checks the first byte for MsgPack markers. Same heuristic as
/// `PayloadFormat::detect()` in `types.rs`.
#[inline]
fn is_likely_msgpack(payload: &[u8]) -> bool {
    match payload.first() {
        Some(b) => matches!(
            b,
            0x80..=0x8f | 0xde..=0xdf | 0x90..=0x9f | 0xdc..=0xdd
        ),
        None => false, // empty payload is not MsgPack
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_no_filters_always_passes() {
        let engine = TransportFilterEngine::new(&[], &[], &Default::default()).unwrap();
        assert!(!engine.has_inbound_filters());
        assert!(!engine.has_outbound_filters());
        assert_eq!(
            engine.apply_inbound(br#"{"any":"thing"}"#),
            FilterDisposition::Pass
        );
    }

    #[test]
    fn engine_tier1_drop_filter() {
        let rules = vec![FilterRule {
            expression: r#"status == "poison""#.into(),
            action: FilterAction::Drop,
        }];
        let engine = TransportFilterEngine::new(&rules, &[], &Default::default()).unwrap();
        assert!(engine.has_inbound_filters());

        assert_eq!(
            engine.apply_inbound(br#"{"status":"poison","data":"x"}"#),
            FilterDisposition::Drop
        );
        assert_eq!(
            engine.apply_inbound(br#"{"status":"ok","data":"x"}"#),
            FilterDisposition::Pass
        );
    }

    #[test]
    fn engine_first_match_wins() {
        let rules = vec![
            FilterRule {
                expression: r#"status == "drop_me""#.into(),
                action: FilterAction::Drop,
            },
            FilterRule {
                expression: r#"status == "drop_me""#.into(),
                action: FilterAction::Dlq,
            },
        ];
        let engine = TransportFilterEngine::new(&rules, &[], &Default::default()).unwrap();
        // First filter matches → Drop, not Dlq
        assert_eq!(
            engine.apply_inbound(br#"{"status":"drop_me"}"#),
            FilterDisposition::Drop
        );
    }

    #[test]
    fn engine_tier2_rejected_without_opt_in() {
        let rules = vec![FilterRule {
            expression: r#"severity > 3 && source != "internal""#.into(),
            action: FilterAction::Drop,
        }];
        let result = TransportFilterEngine::new(&rules, &[], &Default::default());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Tier 2"), "Error should mention tier: {err}");
    }

    #[test]
    fn engine_tier3_rejected_without_complex_opt_in() {
        let tier_config = TransportFilterTierConfig {
            allow_cel_filters_in: true,
            ..Default::default()
        };
        let rules = vec![FilterRule {
            expression: r#"field.matches("^prod-.*")"#.into(),
            action: FilterAction::Drop,
        }];
        let result = TransportFilterEngine::new(&rules, &[], &tier_config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Tier 3"), "Error should mention tier: {err}");
    }

    #[test]
    fn engine_invalid_expression_errors() {
        let rules = vec![FilterRule {
            expression: "this is not valid ((( CEL".into(),
            action: FilterAction::Drop,
        }];
        let result = TransportFilterEngine::new(&rules, &[], &Default::default());
        assert!(result.is_err());
    }

    #[test]
    fn engine_has_dlq_filters_detection() {
        let rules = vec![FilterRule {
            expression: "has(field)".into(),
            action: FilterAction::Dlq,
        }];
        let engine = TransportFilterEngine::new(&rules, &[], &Default::default()).unwrap();
        assert!(engine.has_dlq_filters_in());
        assert!(!engine.has_dlq_filters_out());
    }

    #[test]
    fn engine_msgpack_payload_passes_through() {
        let rules = vec![FilterRule {
            expression: "has(_table)".into(),
            action: FilterAction::Drop,
        }];
        let engine = TransportFilterEngine::new(&rules, &[], &Default::default()).unwrap();
        // MsgPack fixmap header (0x81)
        let msgpack = &[
            0x81, 0xa6, 0x5f, 0x74, 0x61, 0x62, 0x6c, 0x65, 0xa6, 0x65, 0x76, 0x65, 0x6e, 0x74,
            0x73,
        ];
        assert_eq!(engine.apply_inbound(msgpack), FilterDisposition::Pass);
    }

    #[test]
    fn engine_outbound_filter_independent() {
        let in_rules = vec![FilterRule {
            expression: "has(drop_in)".into(),
            action: FilterAction::Drop,
        }];
        let out_rules = vec![FilterRule {
            expression: "has(drop_out)".into(),
            action: FilterAction::Drop,
        }];
        let engine =
            TransportFilterEngine::new(&in_rules, &out_rules, &Default::default()).unwrap();

        let payload_in = br#"{"drop_in":true}"#;
        assert_eq!(engine.apply_inbound(payload_in), FilterDisposition::Drop);
        assert_eq!(engine.apply_outbound(payload_in), FilterDisposition::Pass);

        let payload_out = br#"{"drop_out":true}"#;
        assert_eq!(engine.apply_inbound(payload_out), FilterDisposition::Pass);
        assert_eq!(engine.apply_outbound(payload_out), FilterDisposition::Drop);
    }

    #[test]
    fn is_likely_msgpack_detection() {
        assert!(is_likely_msgpack(&[0x81])); // fixmap
        assert!(is_likely_msgpack(&[0x90])); // fixarray
        assert!(is_likely_msgpack(&[0xde])); // map16
        assert!(!is_likely_msgpack(b"{")); // JSON object
        assert!(!is_likely_msgpack(b"[")); // JSON array
        assert!(!is_likely_msgpack(b"")); // empty
    }
}
