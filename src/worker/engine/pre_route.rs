// Project:   hyperi-rustlib
// File:      src/worker/engine/pre_route.rs
// Purpose:   Zero-copy pre-route field extraction and filter evaluation
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Pre-route phase: extract a routing field from raw JSON bytes using
//! `sonic_rs::get_from_slice` (SIMD-accelerated), then apply filters
//! to decide whether the message continues, is dropped, or goes to DLQ.
//!
//! Hot path: ~50–100 ns per message.

use sonic_rs::JsonValueTrait as _;

use super::config::PreRouteFilterConfig;

/// Result of extracting the routing field from raw bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreRouteExtraction {
    /// Field found with this string value.
    Found(String),
    /// Field not present in the JSON object.
    Missing,
    /// Payload is not valid JSON.
    ParseError(String),
}

/// Outcome after applying filters to a pre-route extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreRouteOutcome {
    /// Message passes — proceed to parse + transform.
    Continue,
    /// Message filtered out — skip parse, include in commit.
    Filtered,
    /// Message routes to DLQ with reason.
    Dlq(String),
}

/// Runtime filter derived from [`PreRouteFilterConfig`].
#[derive(Debug, Clone)]
pub enum PreRouteFilter {
    /// Drop (filter) the message when the named field is absent.
    DropFieldMissing(String),
    /// Route to DLQ when the named field equals a specific value.
    DlqFieldValue { field: String, value: String },
}

/// Extract a routing field from raw JSON bytes using SIMD.
///
/// Uses `sonic_rs::get_from_slice` for zero-copy extraction. The error path
/// (distinguishing a missing field from invalid JSON) requires a full
/// validity check and is intentionally cold.
///
/// # Behaviour
/// - `Ok(lazy_value)` and value is a string → `Found(string)`
/// - `Ok(lazy_value)` and value is not a string → `Found(raw_str)` (raw JSON)
/// - `Err` with `is_not_found()` → `Missing`
/// - `Err` other → `ParseError`
#[inline]
pub fn extract_routing_field(payload: &[u8], field_name: &str) -> PreRouteExtraction {
    match sonic_rs::get_from_slice(payload, &[field_name]) {
        Ok(lv) => {
            // Extract the string value if it is a JSON string, otherwise
            // fall back to the raw representation (e.g. a number or bool
            // used as a routing key).
            let value = lv
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| lv.as_raw_str().to_owned());
            PreRouteExtraction::Found(value)
        }
        Err(e) if e.is_not_found() => PreRouteExtraction::Missing,
        Err(e) => PreRouteExtraction::ParseError(e.to_string()),
    }
}

/// Apply a list of runtime filters to a pre-route extraction result.
///
/// Filters are evaluated in order — first match wins. If no filter matches
/// the message continues.
pub fn apply_filters(
    extraction: &PreRouteExtraction,
    filters: &[PreRouteFilter],
) -> PreRouteOutcome {
    for filter in filters {
        match (filter, extraction) {
            (PreRouteFilter::DropFieldMissing(_field), PreRouteExtraction::Missing) => {
                return PreRouteOutcome::Filtered;
            }
            (
                PreRouteFilter::DlqFieldValue {
                    field: _field,
                    value: expected,
                },
                PreRouteExtraction::Found(actual),
            ) if actual == expected => {
                return PreRouteOutcome::Dlq(format!("field value '{}' matches DLQ rule", actual));
            }
            _ => {}
        }
    }

    // A parse error with no filters still results in Continue — the parse
    // phase will detect and handle the invalid payload.
    PreRouteOutcome::Continue
}

/// Convert config-layer filter definitions to runtime filters.
pub fn filters_from_config(configs: &[PreRouteFilterConfig]) -> Vec<PreRouteFilter> {
    configs
        .iter()
        .map(|c| match c {
            PreRouteFilterConfig::DropFieldMissing { field } => {
                PreRouteFilter::DropFieldMissing(field.clone())
            }
            PreRouteFilterConfig::DlqFieldValue { field, value } => PreRouteFilter::DlqFieldValue {
                field: field.clone(),
                value: value.clone(),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- extraction tests ---------------------------------------------------

    #[test]
    fn extract_routing_field_found() {
        let payload = br#"{"_table": "events", "host": "web1"}"#;
        let result = extract_routing_field(payload, "_table");
        assert_eq!(result, PreRouteExtraction::Found("events".to_string()));
    }

    #[test]
    fn extract_routing_field_missing() {
        let payload = br#"{"host": "web1"}"#;
        let result = extract_routing_field(payload, "_table");
        assert_eq!(result, PreRouteExtraction::Missing);
    }

    #[test]
    fn extract_routing_field_invalid_json() {
        let payload = b"not json at all {{{";
        let result = extract_routing_field(payload, "_table");
        assert!(
            matches!(result, PreRouteExtraction::ParseError(_)),
            "expected ParseError, got {result:?}"
        );
    }

    #[test]
    fn extract_routing_field_numeric_value_returns_raw() {
        // Routing fields are sometimes integers in practice (e.g. a table ID).
        // The extractor should return the raw representation rather than panic.
        let payload = br#"{"_table": 42}"#;
        let result = extract_routing_field(payload, "_table");
        assert_eq!(result, PreRouteExtraction::Found("42".to_string()));
    }

    #[test]
    fn extract_routing_field_nested_object() {
        let payload = br#"{"meta": {"source": "kafka"}, "_table": "logs"}"#;
        let result = extract_routing_field(payload, "_table");
        assert_eq!(result, PreRouteExtraction::Found("logs".to_string()));
    }

    // ---- filter tests -------------------------------------------------------

    #[test]
    fn filter_drop_missing_field() {
        let filters = vec![PreRouteFilter::DropFieldMissing("_table".to_string())];
        let result = apply_filters(&PreRouteExtraction::Missing, &filters);
        assert_eq!(result, PreRouteOutcome::Filtered);
    }

    #[test]
    fn filter_dlq_on_specific_value() {
        let filters = vec![PreRouteFilter::DlqFieldValue {
            field: "_table".to_string(),
            value: "poison".to_string(),
        }];
        let result = apply_filters(&PreRouteExtraction::Found("poison".to_string()), &filters);
        assert!(
            matches!(result, PreRouteOutcome::Dlq(_)),
            "expected Dlq, got {result:?}"
        );
    }

    #[test]
    fn filter_dlq_does_not_trigger_on_different_value() {
        let filters = vec![PreRouteFilter::DlqFieldValue {
            field: "_table".to_string(),
            value: "poison".to_string(),
        }];
        let result = apply_filters(&PreRouteExtraction::Found("events".to_string()), &filters);
        assert_eq!(result, PreRouteOutcome::Continue);
    }

    #[test]
    fn no_filters_always_continue() {
        assert_eq!(
            apply_filters(&PreRouteExtraction::Found("x".to_string()), &[]),
            PreRouteOutcome::Continue
        );
        assert_eq!(
            apply_filters(&PreRouteExtraction::Missing, &[]),
            PreRouteOutcome::Continue
        );
        assert_eq!(
            apply_filters(&PreRouteExtraction::ParseError("bad".to_string()), &[]),
            PreRouteOutcome::Continue
        );
    }

    #[test]
    fn filter_drop_missing_does_not_affect_found() {
        let filters = vec![PreRouteFilter::DropFieldMissing("_table".to_string())];
        let result = apply_filters(&PreRouteExtraction::Found("events".to_string()), &filters);
        assert_eq!(result, PreRouteOutcome::Continue);
    }

    #[test]
    fn filters_from_config_roundtrip() {
        let configs = vec![
            PreRouteFilterConfig::DropFieldMissing {
                field: "_table".to_string(),
            },
            PreRouteFilterConfig::DlqFieldValue {
                field: "status".to_string(),
                value: "error".to_string(),
            },
        ];
        let filters = filters_from_config(&configs);
        assert_eq!(filters.len(), 2);
        assert!(matches!(filters[0], PreRouteFilter::DropFieldMissing(_)));
        assert!(matches!(filters[1], PreRouteFilter::DlqFieldValue { .. }));
    }

    #[test]
    fn first_matching_filter_wins() {
        // Two filters: drop-missing and DLQ-on-value. With a Missing extraction
        // only the first (drop-missing) should fire.
        let filters = vec![
            PreRouteFilter::DropFieldMissing("_table".to_string()),
            PreRouteFilter::DlqFieldValue {
                field: "_table".to_string(),
                value: "anything".to_string(),
            },
        ];
        let result = apply_filters(&PreRouteExtraction::Missing, &filters);
        assert_eq!(result, PreRouteOutcome::Filtered);
    }
}
