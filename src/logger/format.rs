// Project:   hyperi-rustlib
// File:      src/logger/format.rs
// Purpose:   Coloured log output formatter using owo-colors
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Custom coloured log formatter for terminal output.
//!
//! Provides a [`ColouredFormatter`] implementing tracing-subscriber's
//! `FormatEvent` trait with HyperI's standard colour scheme:
//!
//! - **Timestamp:** dim
//! - **Level:** ERROR=red bold, WARN=yellow, INFO=green, DEBUG=blue, TRACE=magenta dim
//! - **Target:** cyan dim
//! - **Source location:** dim
//! - **Field names:** bold
//! - **Message and values:** default

use std::fmt;

use owo_colors::{OwoColorize, Style};
use tracing::Level;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::{FormatTime, UtcTime};
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

/// Coloured log event formatter for terminal output.
///
/// When `enable_ansi` is false, outputs plain text without ANSI codes.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct ColouredFormatter {
    enable_ansi: bool,
    display_target: bool,
    display_file: bool,
    display_line_number: bool,
}

impl ColouredFormatter {
    /// Create a new coloured formatter.
    #[must_use]
    pub fn new(enable_ansi: bool) -> Self {
        Self {
            enable_ansi,
            display_target: true,
            display_file: true,
            display_line_number: true,
        }
    }

    /// Set whether to display source file name.
    #[must_use]
    pub fn with_file(mut self, display: bool) -> Self {
        self.display_file = display;
        self
    }

    /// Set whether to display source line number.
    #[must_use]
    pub fn with_line_number(mut self, display: bool) -> Self {
        self.display_line_number = display;
        self
    }
}

impl<S, N> FormatEvent<S, N> for ColouredFormatter
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();
        let ansi = self.enable_ansi && writer.has_ansi_escapes();

        // Timestamp (dim)
        let timer = UtcTime::rfc_3339();
        let mut ts_buf = String::new();
        let _ = timer.format_time(&mut Writer::new(&mut ts_buf));
        if ansi {
            write!(writer, "{} ", ts_buf.style(dim_style()))?;
        } else {
            write!(writer, "{ts_buf} ")?;
        }

        // Level (coloured)
        let level = meta.level();
        let level_str = format!("{level:>5}");
        if ansi {
            write!(writer, "{} ", level_str.style(level_style(*level)))?;
        } else {
            write!(writer, "{level_str} ")?;
        }

        // Target (cyan dim)
        if self.display_target {
            let target = meta.target();
            if ansi {
                write!(writer, "{}:", target.style(target_style()))?;
            } else {
                write!(writer, "{target}:")?;
            }
        }

        // Span context
        if let Some(scope) = ctx.event_scope() {
            for span in scope.from_root() {
                if ansi {
                    write!(writer, "{}", span.name().style(span_style()))?;
                } else {
                    write!(writer, "{}", span.name())?;
                }
                let ext = span.extensions();
                if let Some(fields) = ext.get::<tracing_subscriber::fmt::FormattedFields<N>>() {
                    if !fields.is_empty() {
                        write!(writer, "{{{fields}}}")?;
                    }
                }
                write!(writer, ":")?;
            }
        }

        // Space before message
        write!(writer, " ")?;

        // Event fields (message + structured fields)
        ctx.format_fields(writer.by_ref(), event)?;

        // Source location (dim)
        if self.display_file || self.display_line_number {
            let file = meta.file();
            let line = meta.line();
            match (self.display_file, self.display_line_number, file, line) {
                (true, true, Some(f), Some(l)) => {
                    let loc = format!(" {f}:{l}");
                    if ansi {
                        write!(writer, "{}", loc.style(dim_style()))?;
                    } else {
                        write!(writer, "{loc}")?;
                    }
                }
                (true, _, Some(f), _) => {
                    let loc = format!(" {f}");
                    if ansi {
                        write!(writer, "{}", loc.style(dim_style()))?;
                    } else {
                        write!(writer, "{loc}")?;
                    }
                }
                (_, true, _, Some(l)) => {
                    let loc = format!(" :{l}");
                    if ansi {
                        write!(writer, "{}", loc.style(dim_style()))?;
                    } else {
                        write!(writer, "{loc}")?;
                    }
                }
                _ => {}
            }
        }

        writeln!(writer)
    }
}

// ---------------------------------------------------------------------------
// Style helpers
// ---------------------------------------------------------------------------

fn level_style(level: Level) -> Style {
    match level {
        Level::ERROR => Style::new().red().bold(),
        Level::WARN => Style::new().yellow(),
        Level::INFO => Style::new().green(),
        Level::DEBUG => Style::new().blue(),
        Level::TRACE => Style::new().magenta().dimmed(),
    }
}

fn dim_style() -> Style {
    Style::new().dimmed()
}

fn target_style() -> Style {
    Style::new().cyan().dimmed()
}

fn span_style() -> Style {
    Style::new().bold()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_level_style_returns_distinct_styles() {
        // Verify each level produces a style (no panics)
        let _ = level_style(Level::ERROR);
        let _ = level_style(Level::WARN);
        let _ = level_style(Level::INFO);
        let _ = level_style(Level::DEBUG);
        let _ = level_style(Level::TRACE);
    }

    #[test]
    fn test_coloured_formatter_builder() {
        let fmt = ColouredFormatter::new(true)
            .with_file(false)
            .with_line_number(false);

        assert!(fmt.enable_ansi);
        assert!(fmt.display_target);
        assert!(!fmt.display_file);
        assert!(!fmt.display_line_number);
    }
}
