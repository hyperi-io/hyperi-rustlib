// Project:   hyperi-rustlib
// File:      src/transport/filter/config.rs
// Purpose:   Configuration types for transport-level message filtering
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Filter configuration types for transport-level message filtering.
//!
//! Filters use CEL syntax. The engine classifies expressions at config load
//! time and selects the optimal execution strategy (Tier 1 SIMD, Tier 2 CEL,
//! Tier 3 complex CEL).

use serde::{Deserialize, Serialize};

/// A single filter rule — CEL expression + disposition action.
///
/// Written in CEL syntax regardless of execution tier. The engine determines
/// the optimal execution strategy at construction time.
///
/// # Examples
///
/// ```yaml
/// - expression: 'has(_table)'
///   action: drop
/// - expression: 'status == "poison"'
///   action: dlq
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilterRule {
    /// CEL expression to evaluate against each message payload.
    pub expression: String,

    /// Action to take when the expression matches. Defaults to `drop`.
    #[serde(default)]
    pub action: FilterAction,
}

/// Disposition action when a filter matches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterAction {
    /// Silently discard the message (counted in metrics).
    #[default]
    Drop,
    /// Route the message to the dead-letter queue (counted + security audit).
    Dlq,
}

/// Performance tier for filter expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterTier {
    /// SIMD field extraction + string comparison. ~50-100ns/msg. Always enabled.
    Tier1,
    /// Pre-compiled CEL with extracted fields. ~500ns-1us/msg. Requires opt-in.
    Tier2,
    /// Complex CEL with restricted functions (regex, iteration). ~5-50us/msg. Requires opt-in.
    Tier3,
}

impl std::fmt::Display for FilterTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tier1 => write!(f, "Tier 1 (SIMD)"),
            Self::Tier2 => write!(f, "Tier 2 (CEL)"),
            Self::Tier3 => write!(f, "Tier 3 (complex CEL)"),
        }
    }
}

/// Direction of filtering (inbound or outbound).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterDirection {
    In,
    Out,
}

impl std::fmt::Display for FilterDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::In => write!(f, "in"),
            Self::Out => write!(f, "out"),
        }
    }
}

/// Tier gate configuration — controls which filter tiers are enabled.
///
/// Lives under the `expression` config cascade key alongside `ProfileConfig`.
/// Separate struct because it serves a different purpose (transport-level
/// gating vs expression-level function restriction).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // 4 independent boolean tier gates (in/out x cel/complex)
pub struct TransportFilterTierConfig {
    /// Enable Tier 2 (CEL engine) for inbound transport filters.
    #[serde(default)]
    pub allow_cel_filters_in: bool,

    /// Enable Tier 2 (CEL engine) for outbound transport filters.
    #[serde(default)]
    pub allow_cel_filters_out: bool,

    /// Enable Tier 3 (complex CEL: regex, iteration, time) for inbound filters.
    /// Implies `allow_cel_filters_in`.
    #[serde(default)]
    pub allow_complex_filters_in: bool,

    /// Enable Tier 3 (complex CEL: regex, iteration, time) for outbound filters.
    /// Implies `allow_cel_filters_out`.
    #[serde(default)]
    pub allow_complex_filters_out: bool,
}

impl TransportFilterTierConfig {
    /// Check if the given tier is allowed for the given direction.
    #[must_use]
    pub fn is_tier_allowed(&self, tier: FilterTier, direction: FilterDirection) -> bool {
        match (tier, direction) {
            (FilterTier::Tier1, _) => true, // always allowed
            (FilterTier::Tier2, FilterDirection::In) => {
                self.allow_cel_filters_in || self.allow_complex_filters_in
            }
            (FilterTier::Tier2, FilterDirection::Out) => {
                self.allow_cel_filters_out || self.allow_complex_filters_out
            }
            (FilterTier::Tier3, FilterDirection::In) => self.allow_complex_filters_in,
            (FilterTier::Tier3, FilterDirection::Out) => self.allow_complex_filters_out,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_rule_deserializes_from_yaml() {
        let yaml = "expression: 'has(_table)'\naction: drop\n";
        let rule: FilterRule = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(rule.expression, "has(_table)");
        assert_eq!(rule.action, FilterAction::Drop);
    }

    #[test]
    fn filter_action_defaults_to_drop() {
        let yaml = "expression: 'has(_table)'";
        let rule: FilterRule = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(rule.action, FilterAction::Drop);
    }

    #[test]
    fn filter_action_dlq_variant() {
        let yaml = "expression: 'status == \"poison\"'\naction: dlq\n";
        let rule: FilterRule = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(rule.action, FilterAction::Dlq);
    }

    #[test]
    fn tier_config_defaults_all_false() {
        let config = TransportFilterTierConfig::default();
        assert!(!config.allow_cel_filters_in);
        assert!(!config.allow_cel_filters_out);
        assert!(!config.allow_complex_filters_in);
        assert!(!config.allow_complex_filters_out);
    }

    #[test]
    fn tier1_always_allowed() {
        let config = TransportFilterTierConfig::default();
        assert!(config.is_tier_allowed(FilterTier::Tier1, FilterDirection::In));
        assert!(config.is_tier_allowed(FilterTier::Tier1, FilterDirection::Out));
    }

    #[test]
    fn tier2_requires_opt_in() {
        let config = TransportFilterTierConfig::default();
        assert!(!config.is_tier_allowed(FilterTier::Tier2, FilterDirection::In));

        let config = TransportFilterTierConfig {
            allow_cel_filters_in: true,
            ..Default::default()
        };
        assert!(config.is_tier_allowed(FilterTier::Tier2, FilterDirection::In));
        assert!(!config.is_tier_allowed(FilterTier::Tier2, FilterDirection::Out));
    }

    #[test]
    fn tier3_implies_tier2() {
        let config = TransportFilterTierConfig {
            allow_complex_filters_in: true,
            ..Default::default()
        };
        // Tier 3 enabled implies Tier 2 is also allowed
        assert!(config.is_tier_allowed(FilterTier::Tier2, FilterDirection::In));
        assert!(config.is_tier_allowed(FilterTier::Tier3, FilterDirection::In));
        // But not outbound
        assert!(!config.is_tier_allowed(FilterTier::Tier2, FilterDirection::Out));
    }

    #[test]
    fn filter_tier_display() {
        assert_eq!(FilterTier::Tier1.to_string(), "Tier 1 (SIMD)");
        assert_eq!(FilterTier::Tier2.to_string(), "Tier 2 (CEL)");
        assert_eq!(FilterTier::Tier3.to_string(), "Tier 3 (complex CEL)");
    }
}
