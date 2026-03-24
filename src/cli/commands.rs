// Project:   hyperi-rustlib
// File:      src/cli/commands.rs
// Purpose:   Standard CLI subcommands for DFE services
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Standard subcommands shared across all DFE services.
//!
//! Every DFE service gets `run`, `version`, and `config-check` for free.
//! The `top` subcommand is available when the `top` feature is enabled.

/// Standard subcommands provided by rustlib.
///
/// Apps embed these via `#[command(flatten)]` in their own subcommand enum:
///
/// ```rust,ignore
/// use clap::Subcommand;
/// use hyperi_rustlib::cli::StandardCommand;
///
/// #[derive(Subcommand)]
/// enum Commands {
///     #[command(flatten)]
///     Standard(StandardCommand),
///     // App-specific subcommands here
/// }
/// ```
#[derive(Debug, Clone, clap::Subcommand)]
pub enum StandardCommand {
    /// Start the service (default if no subcommand given).
    Run,

    /// Print version information and exit.
    Version,

    /// Validate configuration and exit.
    #[command(name = "config-check")]
    ConfigCheck,

    /// Live metrics dashboard (like `vector top`).
    #[cfg(feature = "top")]
    Top(TopArgs),
}

/// Arguments for the `top` subcommand.
#[cfg(feature = "top")]
#[derive(Debug, Clone, clap::Args)]
pub struct TopArgs {
    /// Metrics endpoint URL to poll.
    #[arg(
        long = "metrics-url",
        env = "METRICS_URL",
        default_value = "http://127.0.0.1:9090/metrics"
    )]
    pub metrics_url: String,

    /// Poll interval in seconds.
    #[arg(long = "interval", default_value = "2")]
    pub interval: u64,

    /// Single scrape: print metrics to stdout and exit (no TUI).
    #[arg(long = "once")]
    pub once: bool,

    /// Output as JSON (implies --once).
    #[arg(long = "json")]
    pub json: bool,

    /// Filter metrics by name (regex pattern).
    #[arg(long = "filter", short = 'f')]
    pub filter: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_command_variants() {
        // Verify the enum variants exist and are constructible
        let _ = StandardCommand::Run;
        let _ = StandardCommand::Version;
        let _ = StandardCommand::ConfigCheck;
    }
}
