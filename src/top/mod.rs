// Project:   hyperi-rustlib
// File:      src/top/mod.rs
// Purpose:   TUI metrics dashboard module
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Live TUI metrics dashboard — like `vector top` for DFE services.
//!
//! Polls a running service's Prometheus `/metrics` endpoint and displays
//! a sortable, auto-refreshing table of metrics in the terminal.
//!
//! ## Usage
//!
//! ```bash
//! dfe-loader top                                  # Interactive TUI (default)
//! dfe-loader top --metrics-url http://remote:9090 # Remote endpoint
//! dfe-loader top --interval 5                     # 5-second refresh
//! dfe-loader top --once                           # Single scrape, table to stdout
//! dfe-loader top --json                           # Single scrape, JSON to stdout
//! dfe-loader top --once --filter kafka            # Filter by name substring
//! dfe-loader top --json --filter "buffer.*rows"   # Filter with wildcard pattern
//! ```
//!
//! ## Keybindings (TUI mode)
//!
//! | Key | Action |
//! |-----|--------|
//! | `q` / `Esc` | Quit |
//! | `j` / `↓` | Move down |
//! | `k` / `↑` | Move up |
//! | `g` / `Home` | Go to top |
//! | `G` / `End` | Go to bottom |
//! | `s` | Cycle sort column |
//! | `S` | Reverse sort direction |
//! | `r` | Force refresh |

mod config;
mod dashboard;
pub mod metrics;
mod oneshot;

pub use config::{TopConfig, TopOutputMode};
pub use dashboard::run_dashboard;
pub use metrics::{MetricSample, MetricType, ScrapeResult, fetch_metrics_http, parse_prometheus};
pub use oneshot::run_oneshot;

/// Errors from the TUI dashboard.
#[derive(Debug)]
pub enum TopError {
    /// Terminal initialisation or rendering error.
    Terminal(String),

    /// Metrics fetch error.
    Fetch(String),

    /// Runtime/threading error.
    Runtime(String),
}

impl std::fmt::Display for TopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Terminal(e) => write!(f, "terminal error: {e}"),
            Self::Fetch(e) => write!(f, "fetch error: {e}"),
            Self::Runtime(e) => write!(f, "runtime error: {e}"),
        }
    }
}

impl std::error::Error for TopError {}

/// Run the metrics dashboard or one-shot output.
///
/// In TUI mode, blocks until the user presses `q` or `Esc`.
/// In `--once` or `--json` mode, scrapes once and prints to stdout.
///
/// # Errors
///
/// Returns `TopError` on terminal or network failures.
pub fn run_top(config: &TopConfig) -> Result<(), TopError> {
    match config.output_mode {
        TopOutputMode::Tui => run_dashboard(config),
        TopOutputMode::Once | TopOutputMode::Json => run_oneshot(config),
    }
}
