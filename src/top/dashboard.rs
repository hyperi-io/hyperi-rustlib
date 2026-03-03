// Project:   hyperi-rustlib
// File:      src/top/dashboard.rs
// Purpose:   TUI metrics dashboard rendering and event loop
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Ratatui-based TUI dashboard for live metrics display.
//!
//! Provides a `vector top`-style dashboard that renders Prometheus metrics
//! in a sortable table with summary gauges.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::{DefaultTerminal, Frame};

use super::config::TopConfig;
use super::metrics::{self, MetricSample, MetricType, ScrapeResult};
use super::TopError;

/// Column definitions for the metrics table.
const COLUMNS: &[(&str, usize)] = &[("Metric", 0), ("Type", 1), ("Value", 2), ("Labels", 3)];

/// Dashboard application state.
pub struct DashboardApp {
    /// Current scrape result.
    scrape: ScrapeResult,

    /// Filtered/sorted rows for display.
    rows: Vec<DisplayRow>,

    /// Table selection state.
    table_state: TableState,

    /// Current sort column index.
    sort_column: usize,

    /// Sort ascending.
    sort_ascending: bool,

    /// Metrics endpoint URL.
    metrics_url: String,

    /// Last fetch time.
    last_fetch: Instant,

    /// Poll interval.
    interval: Duration,

    /// Last error message (displayed in footer).
    last_error: Option<String>,

    /// Number of successful fetches.
    fetch_count: u64,

    /// Whether the dashboard should exit.
    should_quit: bool,
}

/// A row prepared for display.
#[derive(Clone)]
struct DisplayRow {
    name: String,
    metric_type: String,
    value: String,
    labels: String,
    raw_value: f64,
}

impl DashboardApp {
    /// Create a new dashboard app.
    pub fn new(config: &TopConfig) -> Self {
        Self {
            scrape: ScrapeResult::default(),
            rows: Vec::new(),
            table_state: TableState::default(),
            sort_column: 0,
            sort_ascending: true,
            metrics_url: config.metrics_url.clone(),
            last_fetch: Instant::now()
                .checked_sub(Duration::from_secs(999))
                .unwrap_or_else(Instant::now),
            interval: Duration::from_secs(config.interval_secs),
            last_error: None,
            fetch_count: 0,
            should_quit: false,
        }
    }

    /// Fetch and update metrics.
    fn refresh(&mut self) {
        match metrics::fetch_metrics_http(&self.metrics_url) {
            Ok(body) => {
                self.scrape = metrics::parse_prometheus(&body);
                self.last_error = None;
                self.fetch_count += 1;
                self.rebuild_rows();
            }
            Err(e) => {
                self.last_error = Some(e.to_string());
            }
        }
        self.last_fetch = Instant::now();
    }

    /// Rebuild display rows from the current scrape.
    fn rebuild_rows(&mut self) {
        self.rows = self
            .scrape
            .samples
            .iter()
            .map(|s| DisplayRow {
                name: s.name.clone(),
                metric_type: s.metric_type.as_str().to_string(),
                value: format_value(s),
                labels: s.labels_string(),
                raw_value: s.value,
            })
            .collect();

        self.sort_rows();
    }

    /// Sort display rows by current column.
    fn sort_rows(&mut self) {
        let col = self.sort_column;
        let asc = self.sort_ascending;

        self.rows.sort_by(|a, b| {
            let cmp = match col {
                0 => a.name.cmp(&b.name),
                1 => a.metric_type.cmp(&b.metric_type),
                2 => a
                    .raw_value
                    .partial_cmp(&b.raw_value)
                    .unwrap_or(std::cmp::Ordering::Equal),
                3 => a.labels.cmp(&b.labels),
                _ => std::cmp::Ordering::Equal,
            };
            if asc {
                cmp
            } else {
                cmp.reverse()
            }
        });
    }

    /// Cycle sort to next column.
    fn cycle_sort(&mut self) {
        self.sort_column = (self.sort_column + 1) % COLUMNS.len();
        self.sort_ascending = true;
        self.sort_rows();
    }

    /// Toggle sort direction.
    fn toggle_sort_direction(&mut self) {
        self.sort_ascending = !self.sort_ascending;
        self.sort_rows();
    }

    /// Handle key events. Returns true if app should quit.
    fn handle_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.table_state.select_next(),
            KeyCode::Char('k') | KeyCode::Up => self.table_state.select_previous(),
            KeyCode::Char('g') | KeyCode::Home => self.table_state.select_first(),
            KeyCode::Char('G') | KeyCode::End => {
                if !self.rows.is_empty() {
                    self.table_state.select(Some(self.rows.len() - 1));
                }
            }
            KeyCode::Char('s') => self.cycle_sort(),
            KeyCode::Char('S') => self.toggle_sort_direction(),
            KeyCode::Char('r') => self.refresh(),
            _ => {}
        }
    }
}

/// Format a metric value for display.
fn format_value(sample: &MetricSample) -> String {
    let v = sample.value;
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return if v.is_sign_positive() { "+Inf" } else { "-Inf" }.to_string();
    }
    // Large integers: no decimal
    if v.fract() == 0.0 && v.abs() < 1e15 {
        #[allow(clippy::cast_possible_truncation)]
        return format!("{}", v as i64);
    }
    // Small values: more precision
    if v.abs() < 1.0 {
        return format!("{v:.6}");
    }
    format!("{v:.2}")
}

/// Render the dashboard.
fn render(frame: &mut Frame, app: &DashboardApp) {
    let area = frame.area();

    // Layout: header (3 lines) + table (fill) + footer (1 line)
    let layout = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .split(area);

    // --- Header: summary stats ---
    render_header(frame, app, layout[0]);

    // --- Table: all metrics ---
    render_table(frame, app, layout[1]);

    // --- Footer: help + status ---
    render_footer(frame, app, layout[2]);
}

/// Render the header summary panel.
fn render_header(frame: &mut Frame, app: &DashboardApp, area: ratatui::layout::Rect) {
    let total = app.scrape.samples.len();
    let counters = app
        .scrape
        .samples
        .iter()
        .filter(|s| s.metric_type == MetricType::Counter)
        .count();
    let gauges = app
        .scrape
        .samples
        .iter()
        .filter(|s| s.metric_type == MetricType::Gauge)
        .count();

    let status = if let Some(ref err) = app.last_error {
        Span::styled(format!(" error: {err}"), Style::default().fg(Color::Red))
    } else {
        Span::styled(
            format!(
                " {} metrics ({counters} counters, {gauges} gauges) | fetches: {}",
                total, app.fetch_count
            ),
            Style::default().fg(Color::Green),
        )
    };

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            " dfe top",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" — "),
        Span::styled(&app.metrics_url, Style::default().fg(Color::DarkGray)),
    ]))
    .block(Block::default().borders(Borders::BOTTOM));

    let status_bar = Paragraph::new(Line::from(vec![status]));

    // Split header area: title + stats
    let header_layout = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);

    frame.render_widget(header, header_layout[0]);
    frame.render_widget(status_bar, header_layout[1]);
}

/// Render the metrics table.
fn render_table(frame: &mut Frame, app: &DashboardApp, area: ratatui::layout::Rect) {
    // Build header with sort indicator
    let header_cells: Vec<Cell> = COLUMNS
        .iter()
        .enumerate()
        .map(|(i, (name, _))| {
            let indicator = if i == app.sort_column {
                if app.sort_ascending {
                    " ▲"
                } else {
                    " ▼"
                }
            } else {
                ""
            };
            Cell::from(format!("{name}{indicator}")).style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect();

    let header = Row::new(header_cells).bottom_margin(0);

    // Build data rows
    let rows: Vec<Row> = app
        .rows
        .iter()
        .map(|r| {
            let type_color = match r.metric_type.as_str() {
                "counter" => Color::Cyan,
                "gauge" => Color::Green,
                "histogram" => Color::Magenta,
                "summary" => Color::Blue,
                _ => Color::DarkGray,
            };

            Row::new(vec![
                Cell::from(r.name.clone()),
                Cell::from(r.metric_type.clone()).style(Style::default().fg(type_color)),
                Cell::from(r.value.clone()).style(Style::default().fg(Color::White)),
                Cell::from(r.labels.clone()).style(Style::default().fg(Color::DarkGray)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(40),
        Constraint::Length(10),
        Constraint::Length(16),
        Constraint::Percentage(30),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(" Metrics "))
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(table, area, &mut app.table_state.clone());
}

/// Render the footer help bar.
fn render_footer(frame: &mut Frame, app: &DashboardApp, area: ratatui::layout::Rect) {
    let sort_name = COLUMNS.get(app.sort_column).map_or("?", |(name, _)| name);
    let sort_dir = if app.sort_ascending { "asc" } else { "desc" };

    let help = Paragraph::new(Line::from(vec![
        Span::styled(" q", Style::default().fg(Color::Yellow)),
        Span::raw(":quit "),
        Span::styled("j/k", Style::default().fg(Color::Yellow)),
        Span::raw(":nav "),
        Span::styled("s", Style::default().fg(Color::Yellow)),
        Span::raw(":sort "),
        Span::styled("S", Style::default().fg(Color::Yellow)),
        Span::raw(":reverse "),
        Span::styled("r", Style::default().fg(Color::Yellow)),
        Span::raw(":refresh "),
        Span::styled("g/G", Style::default().fg(Color::Yellow)),
        Span::raw(":top/bottom "),
        Span::raw("| "),
        Span::styled(
            format!("sort: {sort_name} {sort_dir}"),
            Style::default().fg(Color::DarkGray),
        ),
    ]))
    .style(Style::default().bg(Color::Black));

    frame.render_widget(help, area);
}

/// Run the TUI dashboard main loop.
///
/// Blocks until the user presses `q` or `Esc`.
///
/// # Errors
///
/// Returns `TopError` on terminal or fetch failures.
pub fn run_dashboard(config: &TopConfig) -> Result<(), TopError> {
    let mut terminal = ratatui::init();
    let result = run_loop(&mut terminal, config);
    ratatui::restore();
    result
}

/// Inner event loop.
fn run_loop(terminal: &mut DefaultTerminal, config: &TopConfig) -> Result<(), TopError> {
    let mut app = DashboardApp::new(config);

    // Initial fetch
    app.refresh();

    // Select first row
    if !app.rows.is_empty() {
        app.table_state.select(Some(0));
    }

    loop {
        // Periodic refresh
        if app.last_fetch.elapsed() >= app.interval {
            app.refresh();
        }

        // Render
        terminal
            .draw(|frame| render(frame, &app))
            .map_err(|e| TopError::Terminal(e.to_string()))?;

        // Handle events (100ms poll timeout for responsive input + timer checks)
        if event::poll(Duration::from_millis(100)).map_err(|e| TopError::Terminal(e.to_string()))? {
            if let Event::Key(key) = event::read().map_err(|e| TopError::Terminal(e.to_string()))? {
                if key.kind == KeyEventKind::Press {
                    app.handle_key(key.code);
                    if app.should_quit {
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_value_integer() {
        let sample = MetricSample {
            name: "test".into(),
            labels: Default::default(),
            value: 42.0,
            metric_type: MetricType::Counter,
        };
        assert_eq!(format_value(&sample), "42");
    }

    #[test]
    fn test_format_value_float() {
        let sample = MetricSample {
            name: "test".into(),
            labels: Default::default(),
            value: 3.14159,
            metric_type: MetricType::Gauge,
        };
        assert_eq!(format_value(&sample), "3.14");
    }

    #[test]
    fn test_format_value_small() {
        let sample = MetricSample {
            name: "test".into(),
            labels: Default::default(),
            value: 0.000_123,
            metric_type: MetricType::Gauge,
        };
        assert_eq!(format_value(&sample), "0.000123");
    }

    #[test]
    fn test_format_value_nan() {
        let sample = MetricSample {
            name: "test".into(),
            labels: Default::default(),
            value: f64::NAN,
            metric_type: MetricType::Untyped,
        };
        assert_eq!(format_value(&sample), "NaN");
    }

    #[test]
    fn test_dashboard_app_cycle_sort() {
        let config = TopConfig::default();
        let mut app = DashboardApp::new(&config);
        assert_eq!(app.sort_column, 0);
        app.cycle_sort();
        assert_eq!(app.sort_column, 1);
        app.cycle_sort();
        assert_eq!(app.sort_column, 2);
        app.cycle_sort();
        assert_eq!(app.sort_column, 3);
        app.cycle_sort();
        assert_eq!(app.sort_column, 0);
    }

    #[test]
    fn test_dashboard_app_toggle_sort() {
        let config = TopConfig::default();
        let mut app = DashboardApp::new(&config);
        assert!(app.sort_ascending);
        app.toggle_sort_direction();
        assert!(!app.sort_ascending);
        app.toggle_sort_direction();
        assert!(app.sort_ascending);
    }

    #[test]
    fn test_handle_key_quit() {
        let config = TopConfig::default();
        let mut app = DashboardApp::new(&config);
        app.handle_key(KeyCode::Char('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn test_handle_key_esc() {
        let config = TopConfig::default();
        let mut app = DashboardApp::new(&config);
        app.handle_key(KeyCode::Esc);
        assert!(app.should_quit);
    }
}
