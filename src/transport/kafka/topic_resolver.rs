// Project:   hyperi-rustlib
// File:      src/transport/kafka/topic_resolver.rs
// Purpose:   Kafka topic auto-discovery with configurable suppression rules and regex filters
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Kafka topic resolver for auto-discovery with filter and suppression support.
//!
//! `TopicResolver` fetches the full topic list from the broker, applies
//! configurable suppression rules (e.g. `_load` suppresses `_land`), then
//! filters with include/exclude regex patterns before returning the resolved set.
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::transport::kafka::{KafkaConfig, TopicResolver};
//!
//! let config = KafkaConfig {
//!     topic_include: vec!["^events_".to_string()],
//!     ..Default::default()
//! };
//!
//! let resolver = TopicResolver::new(&config)?;
//! let topics = resolver.resolve()?;
//! println!("Resolved topics: {:?}", topics);
//! ```

use regex::Regex;
use std::collections::HashSet;

use super::admin::KafkaAdmin;
use super::config::{KafkaConfig, SuppressionRule};
use crate::transport::error::{TransportError, TransportResult};

// ============================================================================
// TopicResolver
// ============================================================================

/// Resolves Kafka topics from the broker with filtering and suppression.
///
/// Fetches all topics from the broker, applies suppression rules to eliminate
/// redundant topics (e.g. `_load` over `_land`), then filters the result with
/// include/exclude regex patterns.
pub struct TopicResolver {
    admin: KafkaAdmin,
    suppression_rules: Vec<SuppressionRule>,
    include_patterns: Vec<Regex>,
    exclude_patterns: Vec<Regex>,
}

impl TopicResolver {
    /// Create a new `TopicResolver` from a `KafkaConfig`.
    ///
    /// Compiles include/exclude regex patterns at construction time so that
    /// `resolve()` is cheap to call repeatedly.
    ///
    /// # Errors
    ///
    /// Returns error if admin client creation fails or any regex pattern is invalid.
    pub fn new(config: &KafkaConfig) -> TransportResult<Self> {
        let admin = KafkaAdmin::new(config)?;
        let include_patterns = compile_patterns(&config.topic_include)?;
        let exclude_patterns = compile_patterns(&config.topic_exclude)?;
        Ok(Self {
            admin,
            suppression_rules: config.topic_suppression_rules.clone(),
            include_patterns,
            exclude_patterns,
        })
    }

    /// Resolve the effective topic list from the broker.
    ///
    /// Steps:
    /// 1. Fetch all topics from the broker via `KafkaAdmin::list_topics()`
    /// 2. Apply suppression rules (e.g. drop `_land` when `_load` exists)
    /// 3. Filter with include/exclude regex patterns
    /// 4. Sort and deduplicate
    ///
    /// # Errors
    ///
    /// Returns error if the broker metadata fetch fails.
    pub fn resolve(&self) -> TransportResult<Vec<String>> {
        let all_topics = self.admin.list_topics()?;
        tracing::debug!(total = all_topics.len(), "Fetched broker topic list");

        let after_suppression = apply_suppression_rules(all_topics, &self.suppression_rules);

        let mut resolved: Vec<String> = after_suppression
            .into_iter()
            .filter(|t| passes_filters(t, &self.include_patterns, &self.exclude_patterns))
            .collect();

        resolved.sort();
        resolved.dedup();

        tracing::info!(count = resolved.len(), topics = ?resolved, "Resolved Kafka topics");
        Ok(resolved)
    }
}

impl std::fmt::Debug for TopicResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TopicResolver")
            .field(
                "suppression_rules",
                &self
                    .suppression_rules
                    .iter()
                    .map(|r| format!("{} → {}", r.preferred_suffix, r.suppressed_suffix))
                    .collect::<Vec<_>>(),
            )
            .field(
                "include_patterns",
                &self
                    .include_patterns
                    .iter()
                    .map(Regex::as_str)
                    .collect::<Vec<_>>(),
            )
            .field(
                "exclude_patterns",
                &self
                    .exclude_patterns
                    .iter()
                    .map(Regex::as_str)
                    .collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

// ============================================================================
// Suppression Logic
// ============================================================================

/// Apply suppression rules to a topic list.
///
/// For each rule: collect base names that have a `preferred_suffix` topic,
/// then remove any topic with `suppressed_suffix` whose base name is in the set.
///
/// ## Example
///
/// With the default rule (`_load` suppresses `_land`):
/// - `auth_load` + `auth_land` → keeps `auth_load`, drops `auth_land`
/// - `events_land` (no `events_load`) → kept
pub fn apply_suppression_rules(topics: Vec<String>, rules: &[SuppressionRule]) -> Vec<String> {
    if rules.is_empty() {
        return topics;
    }

    let mut result = topics;
    for rule in rules {
        let preferred_bases: HashSet<String> = result
            .iter()
            .filter_map(|t| {
                t.strip_suffix(rule.preferred_suffix.as_str())
                    .map(String::from)
            })
            .collect();

        result = result
            .into_iter()
            .filter(|t| {
                if let Some(base) = t.strip_suffix(rule.suppressed_suffix.as_str()) {
                    !preferred_bases.contains(base)
                } else {
                    true
                }
            })
            .collect();
    }
    result
}

// ============================================================================
// Filter Logic
// ============================================================================

/// Check if a topic passes include/exclude regex filters.
///
/// - **Include**: if patterns exist, the topic MUST match at least one (OR).
///   Empty include list means all topics are accepted.
/// - **Exclude**: the topic MUST NOT match any pattern (OR). Exclude wins over include.
pub fn passes_filters(topic: &str, include: &[Regex], exclude: &[Regex]) -> bool {
    if !include.is_empty() && !include.iter().any(|r| r.is_match(topic)) {
        return false;
    }
    if exclude.iter().any(|r| r.is_match(topic)) {
        return false;
    }
    true
}

fn compile_patterns(patterns: &[String]) -> TransportResult<Vec<Regex>> {
    patterns
        .iter()
        .map(|p| {
            Regex::new(p).map_err(|e| {
                TransportError::Config(format!("Invalid topic filter regex '{p}': {e}"))
            })
        })
        .collect()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_suppression_load_over_land() {
        let rules = vec![SuppressionRule {
            preferred_suffix: "_load".into(),
            suppressed_suffix: "_land".into(),
        }];
        let topics = vec![
            "auth_land".into(),
            "auth_load".into(),
            "events_land".into(),
            "syslog_load".into(),
        ];
        let result = apply_suppression_rules(topics, &rules);
        assert!(!result.contains(&"auth_land".to_string()));
        assert!(result.contains(&"auth_load".to_string()));
        assert!(result.contains(&"events_land".to_string()));
        assert!(result.contains(&"syslog_load".to_string()));
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn no_suppression_rules() {
        let topics = vec!["auth_land".into(), "auth_load".into()];
        let result = apply_suppression_rules(topics.clone(), &[]);
        assert_eq!(result, topics);
    }

    #[test]
    fn custom_suppression_rule() {
        let rules = vec![SuppressionRule {
            preferred_suffix: "_enriched".into(),
            suppressed_suffix: "_raw".into(),
        }];
        let topics = vec![
            "events_raw".into(),
            "events_enriched".into(),
            "other_raw".into(),
        ];
        let result = apply_suppression_rules(topics, &rules);
        assert!(!result.contains(&"events_raw".to_string()));
        assert!(result.contains(&"events_enriched".to_string()));
        assert!(result.contains(&"other_raw".to_string()));
    }

    #[test]
    fn multiple_suppression_rules() {
        let rules = vec![
            SuppressionRule {
                preferred_suffix: "_load".into(),
                suppressed_suffix: "_land".into(),
            },
            SuppressionRule {
                preferred_suffix: "_enriched".into(),
                suppressed_suffix: "_raw".into(),
            },
        ];
        let topics = vec![
            "auth_land".into(),
            "auth_load".into(),
            "events_raw".into(),
            "events_enriched".into(),
            "other_land".into(),
        ];
        let result = apply_suppression_rules(topics, &rules);
        assert_eq!(result.len(), 3);
        assert!(result.contains(&"auth_load".to_string()));
        assert!(result.contains(&"events_enriched".to_string()));
        assert!(result.contains(&"other_land".to_string()));
    }

    #[test]
    fn passes_filters_empty() {
        assert!(passes_filters("auth_land", &[], &[]));
    }

    #[test]
    fn passes_filters_include_only() {
        let include = vec![Regex::new("^auth").unwrap()];
        assert!(passes_filters("auth_land", &include, &[]));
        assert!(!passes_filters("events_land", &include, &[]));
    }

    #[test]
    fn passes_filters_exclude_only() {
        let exclude = vec![Regex::new("^test_").unwrap()];
        assert!(passes_filters("auth_land", &[], &exclude));
        assert!(!passes_filters("test_land", &[], &exclude));
    }

    #[test]
    fn passes_filters_both() {
        let include = vec![Regex::new("_land$").unwrap()];
        let exclude = vec![Regex::new("^test_").unwrap()];
        assert!(passes_filters("auth_land", &include, &exclude));
        assert!(!passes_filters("test_land", &include, &exclude));
        assert!(!passes_filters("auth_load", &include, &exclude));
    }

    #[test]
    fn compile_patterns_invalid_regex() {
        let result = compile_patterns(&["[invalid".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn compile_patterns_valid() {
        let result = compile_patterns(&["^auth".to_string(), "_land$".to_string()]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 2);
    }

    #[test]
    fn suppression_no_matching_pairs() {
        let rules = vec![SuppressionRule {
            preferred_suffix: "_load".into(),
            suppressed_suffix: "_land".into(),
        }];
        // No _load topics present — nothing should be suppressed
        let topics = vec!["auth_land".into(), "events_land".into()];
        let result = apply_suppression_rules(topics.clone(), &rules);
        assert_eq!(result, topics);
    }
}
