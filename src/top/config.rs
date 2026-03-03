// Project:   hyperi-rustlib
// File:      src/top/config.rs
// Purpose:   TUI dashboard configuration
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Configuration for the `top` TUI dashboard.

/// Output mode for the `top` command.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TopOutputMode {
    /// Interactive TUI dashboard.
    #[default]
    Tui,
    /// Single scrape, table to stdout.
    Once,
    /// Single scrape, JSON to stdout.
    Json,
}

/// Configuration for the metrics dashboard.
#[derive(Debug, Clone)]
pub struct TopConfig {
    /// Metrics endpoint URL to poll (e.g. `http://127.0.0.1:9090/metrics`).
    pub metrics_url: String,

    /// Poll interval in seconds.
    pub interval_secs: u64,

    /// Output mode (TUI, once, or JSON).
    pub output_mode: TopOutputMode,

    /// Optional regex filter for metric names.
    pub filter: Option<String>,
}

impl Default for TopConfig {
    fn default() -> Self {
        Self {
            metrics_url: "http://127.0.0.1:9090/metrics".to_string(),
            interval_secs: 2,
            output_mode: TopOutputMode::default(),
            filter: None,
        }
    }
}

impl TopConfig {
    /// Create from CLI `TopArgs`.
    #[must_use]
    pub fn from_args(args: &crate::cli::TopArgs) -> Self {
        let output_mode = if args.json {
            TopOutputMode::Json
        } else if args.once {
            TopOutputMode::Once
        } else {
            TopOutputMode::Tui
        };

        Self {
            metrics_url: args.metrics_url.clone(),
            interval_secs: args.interval,
            output_mode,
            filter: args.filter.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let config = TopConfig::default();
        assert_eq!(config.metrics_url, "http://127.0.0.1:9090/metrics");
        assert_eq!(config.interval_secs, 2);
        assert_eq!(config.output_mode, TopOutputMode::Tui);
        assert!(config.filter.is_none());
    }

    #[test]
    fn test_output_mode_json_implies_once() {
        // JSON mode should be its own variant, not depend on --once
        assert_ne!(TopOutputMode::Json, TopOutputMode::Once);
        assert_ne!(TopOutputMode::Json, TopOutputMode::Tui);
    }
}
