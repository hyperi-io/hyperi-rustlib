// Project:   hyperi-rustlib
// File:      src/worker/engine/config.rs
// Purpose:   Configuration for the SIMD-optimised batch processing engine
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use serde::{Deserialize, Serialize};

use super::types::PayloadFormat;

/// Action to take when a message fails to parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParseErrorAction {
    /// Route failed messages to the dead-letter queue (default).
    #[default]
    Dlq,
    /// Silently skip failed messages (counted but not DLQ'd).
    Skip,
    /// Fail the entire batch on the first parse error.
    FailBatch,
}

/// Pre-route filter applied to each message before routing decisions.
///
/// Filters are evaluated in order. The first filter that matches determines
/// the message's [`super::types::PreRouteResult`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PreRouteFilterConfig {
    /// Route to DLQ if a required field is absent.
    DropFieldMissing {
        /// Name of the required field.
        field: String,
    },
    /// Route to DLQ if a field equals a specific value.
    DlqFieldValue {
        /// Name of the field to check.
        field: String,
        /// Value that triggers DLQ routing.
        value: String,
    },
}

/// Configuration for the batch processing engine.
///
/// All values are overridable via the 8-layer config cascade
/// (CLI > ENV > .env > settings.{env}.yaml > settings.yaml > defaults > rustlib > hard-coded).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchProcessingConfig {
    /// Maximum number of messages per rayon chunk.
    ///
    /// Smaller chunks reduce per-task overhead; larger chunks amortise
    /// the rayon work-stealing cost. Default 10 000 matches DFE batch sizes.
    #[serde(default = "default_max_chunk_size")]
    pub max_chunk_size: usize,

    /// Expected payload format (auto-detect by default).
    #[serde(default)]
    pub format: PayloadFormat,

    /// JSON field used to route messages to the correct downstream sink.
    ///
    /// For dfe-loader this is typically `"_table"`.
    #[serde(default)]
    pub routing_field: Option<String>,

    /// Pre-route filters applied before routing decisions.
    ///
    /// Evaluated in order — first match wins.
    #[serde(default)]
    pub pre_route_filters: Vec<PreRouteFilterConfig>,

    /// Milliseconds to pause between batches when memory pressure is high.
    #[serde(default = "default_memory_pressure_pause_ms")]
    pub memory_pressure_pause_ms: u64,

    /// Action to take when a message fails JSON parsing.
    #[serde(default = "default_parse_error_action")]
    pub parse_error_action: ParseErrorAction,

    /// Fields to pre-extract into [`super::types::ParsedMessage::Parsed::extracted`]
    /// for fast routing lookups.
    ///
    /// Extracting these at parse time avoids repeated `value.get()` traversals
    /// during routing and filtering.
    #[serde(default = "default_known_fields")]
    pub known_fields: Vec<String>,
}

fn default_max_chunk_size() -> usize {
    10_000
}

fn default_memory_pressure_pause_ms() -> u64 {
    50
}

fn default_parse_error_action() -> ParseErrorAction {
    ParseErrorAction::Dlq
}

fn default_known_fields() -> Vec<String> {
    vec![
        "_table".to_string(),
        "_timestamp".to_string(),
        "_source".to_string(),
        "host".to_string(),
        "source_type".to_string(),
        "event_type".to_string(),
    ]
}

impl Default for BatchProcessingConfig {
    fn default() -> Self {
        Self {
            max_chunk_size: default_max_chunk_size(),
            format: PayloadFormat::default(),
            routing_field: None,
            pre_route_filters: vec![],
            memory_pressure_pause_ms: default_memory_pressure_pause_ms(),
            parse_error_action: default_parse_error_action(),
            known_fields: default_known_fields(),
        }
    }
}

impl BatchProcessingConfig {
    /// Load config from the cascade under the given key (e.g. `"batch_processing"`).
    ///
    /// Falls back to defaults if the config cascade is not initialised or the
    /// key is absent.
    ///
    /// # Errors
    ///
    /// Returns a `ConfigError` only if the cascade is initialised and the key
    /// contains data that cannot be deserialised into `BatchProcessingConfig`.
    pub fn from_cascade(key: &str) -> Result<Self, crate::config::ConfigError> {
        let config: Self = if let Some(cfg) = crate::config::try_get() {
            cfg.unmarshal_key(key).unwrap_or_default()
        } else {
            tracing::debug!("Config cascade not initialised, using default BatchProcessingConfig");
            Self::default()
        };
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let config = BatchProcessingConfig::default();
        assert_eq!(config.max_chunk_size, 10_000);
        assert!(config.routing_field.is_none());
        assert_eq!(config.memory_pressure_pause_ms, 50);
        assert_eq!(config.known_fields.len(), 6);
        assert!(config.known_fields.contains(&"_table".to_string()));
    }

    #[test]
    fn from_cascade_falls_back_to_defaults() {
        let config = BatchProcessingConfig::from_cascade("batch_processing").unwrap();
        assert_eq!(config.max_chunk_size, 10_000);
    }

    #[test]
    fn parse_error_action_default_is_dlq() {
        let action = ParseErrorAction::default();
        assert!(matches!(action, ParseErrorAction::Dlq));
    }

    #[test]
    fn serde_roundtrip() {
        let config = BatchProcessingConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: BatchProcessingConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.max_chunk_size, 10_000);
    }

    #[test]
    fn pre_route_filter_serde_roundtrip() {
        let filter = PreRouteFilterConfig::DropFieldMissing {
            field: "_table".to_string(),
        };
        let json = serde_json::to_string(&filter).unwrap();
        let back: PreRouteFilterConfig = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            back,
            PreRouteFilterConfig::DropFieldMissing { .. }
        ));
    }

    #[test]
    fn parse_error_action_variants_serde() {
        let actions = [
            ParseErrorAction::Dlq,
            ParseErrorAction::Skip,
            ParseErrorAction::FailBatch,
        ];
        for action in actions {
            let json = serde_json::to_string(&action).unwrap();
            let back: ParseErrorAction = serde_json::from_str(&json).unwrap();
            assert_eq!(action, back);
        }
    }
}
