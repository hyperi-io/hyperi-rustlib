// Project:   hyperi-rustlib
// File:      src/cli/output.rs
// Purpose:   CLI output formatting helpers
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Output formatting helpers for CLI tools.
//!
//! Provides consistent terminal output across all DFE services.

use std::fmt;

/// Print a success message to stderr.
pub fn print_success(msg: &str) {
    eprintln!("[ok] {msg}");
}

/// Print an error message to stderr.
pub fn print_error(msg: &str) {
    eprintln!("[error] {msg}");
}

/// Print a warning message to stderr.
pub fn print_warn(msg: &str) {
    eprintln!("[warn] {msg}");
}

/// Print an info message to stderr.
pub fn print_info(msg: &str) {
    eprintln!("[info] {msg}");
}

/// Print a key-value pair to stderr with aligned formatting.
pub fn print_kv(key: &str, value: &dyn fmt::Display) {
    eprintln!("  {key:<16} {value}");
}

/// Print a simple table to stderr.
///
/// Headers and rows are aligned by column width.
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    if headers.is_empty() {
        return;
    }

    // Calculate column widths
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }

    // Print header
    let header_line: Vec<String> = headers
        .iter()
        .zip(&widths)
        .map(|(h, w)| format!("{h:<w$}"))
        .collect();
    eprintln!("  {}", header_line.join("  "));

    // Print separator
    let sep_line: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    eprintln!("  {}", sep_line.join("  "));

    // Print rows
    for row in rows {
        let cells: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(c, w)| format!("{c:<w$}"))
            .collect();
        eprintln!("  {}", cells.join("  "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_print_table_empty_headers() {
        // Should not panic with empty headers
        print_table(&[], &[]);
    }

    #[test]
    fn test_print_table_formats() {
        // Verify it doesn't panic with real data
        let headers = &["Name", "Value"];
        let rows = vec![
            vec!["key1".to_string(), "val1".to_string()],
            vec!["longer_key".to_string(), "v".to_string()],
        ];
        print_table(headers, &rows);
    }
}
