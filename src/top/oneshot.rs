// Project:   hyperi-rustlib
// File:      src/top/oneshot.rs
// Purpose:   Non-interactive single-scrape output for top command
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Single-scrape output modes for `top --once` and `top --json`.
//!
//! Fetches metrics once, optionally filters, and prints to stdout
//! as either tab-separated values (TSV) or JSON.
//!
//! ## Bash-friendly output
//!
//! `--once` produces TSV (tab-separated) with a header line, suitable for
//! piping through standard Unix tools:
//!
//! ```bash
//! dfe-loader top --once | grep kafka          # filter by name
//! dfe-loader top --once | awk -F'\t' '$3>100' # value > 100
//! dfe-loader top --once | cut -f1,3           # name and value only
//! dfe-loader top --once | tail -n+2 | sort -t$'\t' -k3 -rn  # sort by value
//! ```

use std::collections::HashMap;

use super::config::{TopConfig, TopOutputMode};
use super::metrics::{self, MetricSample, MetricType};
use super::TopError;

/// Run a single scrape and print results to stdout.
///
/// # Errors
///
/// Returns `TopError::Fetch` if the metrics endpoint is unreachable.
pub fn run_oneshot(config: &TopConfig) -> Result<(), TopError> {
    let body = metrics::fetch_metrics_http(&config.metrics_url)?;
    let scrape = metrics::parse_prometheus(&body);

    let samples = filter_samples(&scrape.samples, config.filter.as_deref());

    match config.output_mode {
        TopOutputMode::Once => print_table(&samples, &config.metrics_url),
        TopOutputMode::Json => print_json(&samples),
        TopOutputMode::Tui => unreachable!("oneshot called with TUI mode"),
    }

    Ok(())
}

/// Filter samples by regex pattern on metric name.
fn filter_samples<'a>(samples: &'a [MetricSample], filter: Option<&str>) -> Vec<&'a MetricSample> {
    match filter {
        Some(pattern) => samples
            .iter()
            .filter(|s| match_filter(&s.name, pattern))
            .collect(),
        None => samples.iter().collect(),
    }
}

/// Simple pattern matching — supports basic regex-like patterns.
/// Uses contains for plain strings, or regex for patterns with metacharacters.
fn match_filter(name: &str, pattern: &str) -> bool {
    // If pattern has no regex metacharacters, do simple contains match
    if !has_regex_chars(pattern) {
        return name.contains(pattern);
    }
    // Compile regex (fallback to contains on invalid pattern)
    regex_match(name, pattern)
}

/// Check if a string contains regex metacharacters.
fn has_regex_chars(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(
            c,
            '.' | '*' | '+' | '?' | '[' | ']' | '(' | ')' | '{' | '}' | '|' | '^' | '$' | '\\'
        )
    })
}

/// Lightweight wildcard match for metric name filtering.
/// Supports `.*` as a wildcard for any sequence of characters.
/// Filters are unanchored (substring match) unless `^` or `$` are used.
fn regex_match(name: &str, pattern: &str) -> bool {
    // Handle anchors
    let anchored_start = pattern.starts_with('^');
    let anchored_end = pattern.ends_with('$');
    let pattern = pattern
        .strip_prefix('^')
        .unwrap_or(pattern)
        .strip_suffix('$')
        .unwrap_or(pattern);

    // Split on .* wildcard
    let parts: Vec<&str> = pattern.split(".*").collect();

    if parts.len() == 1 {
        // No wildcards — substring match (respecting anchors)
        if anchored_start && anchored_end {
            return name == pattern;
        }
        if anchored_start {
            return name.starts_with(pattern);
        }
        if anchored_end {
            return name.ends_with(pattern);
        }
        return name.contains(pattern);
    }

    // Check that all parts appear in order within the name
    let mut remaining = name;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match remaining.find(part) {
            Some(pos) => {
                if i == 0 && anchored_start && pos != 0 {
                    return false;
                }
                remaining = &remaining[pos + part.len()..];
            }
            None => return false,
        }
    }

    // If anchored at end, the last non-empty part must reach the end
    if anchored_end && !remaining.is_empty() {
        if let Some(last) = parts.iter().rev().find(|p| !p.is_empty()) {
            return name.ends_with(last);
        }
    }

    true
}

/// Print metrics as tab-separated values (TSV).
///
/// Output format: `NAME\tTYPE\tVALUE\tLABELS`
///
/// Designed for piping through `grep`, `awk -F'\t'`, `cut -f`, `sort -t$'\t'`.
/// Summary line goes to stderr so stdout is clean for piping.
fn print_table(samples: &[&MetricSample], url: &str) {
    // Summary to stderr (doesn't pollute piped stdout)
    let counters = samples
        .iter()
        .filter(|s| s.metric_type == MetricType::Counter)
        .count();
    let gauges = samples
        .iter()
        .filter(|s| s.metric_type == MetricType::Gauge)
        .count();
    let histograms = samples
        .iter()
        .filter(|s| s.metric_type == MetricType::Histogram)
        .count();
    eprintln!(
        "{} metrics ({counters} counters, {gauges} gauges, {histograms} histograms) from {url}",
        samples.len()
    );

    // TSV header
    println!("NAME\tTYPE\tVALUE\tLABELS");

    // TSV data rows
    for sample in samples {
        let labels = format_labels_tsv(&sample.labels);
        println!(
            "{}\t{}\t{}\t{}",
            sample.name,
            sample.metric_type.as_str(),
            format_value(sample),
            labels
        );
    }
}

/// Format labels as comma-separated `key=value` pairs for TSV output.
/// No braces, no quotes — clean for awk/grep.
fn format_labels_tsv(labels: &HashMap<String, String>) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let mut pairs: Vec<String> = labels.iter().map(|(k, v)| format!("{k}={v}")).collect();
    pairs.sort();
    pairs.join(",")
}

/// Print metrics as JSON array.
fn print_json(samples: &[&MetricSample]) {
    println!("[");
    for (i, sample) in samples.iter().enumerate() {
        let comma = if i + 1 < samples.len() { "," } else { "" };

        // Build labels object
        let labels_json = if sample.labels.is_empty() {
            "{}".to_string()
        } else {
            let pairs: Vec<String> = sample
                .labels
                .iter()
                .map(|(k, v)| format!("    \"{k}\": \"{v}\""))
                .collect();
            format!("{{\n{}\n  }}", pairs.join(",\n"))
        };

        let value_str = format_value_json(sample.value);

        println!("  {{");
        println!("    \"name\": \"{}\",", sample.name);
        println!("    \"type\": \"{}\",", sample.metric_type.as_str());
        println!("    \"value\": {value_str},");
        println!("    \"labels\": {labels_json}");
        println!("  }}{comma}");
    }
    println!("]");
}

/// Format a metric value for table display.
fn format_value(sample: &MetricSample) -> String {
    let v = sample.value;
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return if v.is_sign_positive() { "+Inf" } else { "-Inf" }.to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        #[allow(clippy::cast_possible_truncation)]
        return format!("{}", v as i64);
    }
    if v.abs() < 1.0 {
        return format!("{v:.6}");
    }
    format!("{v:.2}")
}

/// Format a value for JSON output (NaN/Inf as strings since JSON has no native representation).
fn format_value_json(v: f64) -> String {
    if v.is_nan() {
        return "\"NaN\"".to_string();
    }
    if v.is_infinite() {
        return if v.is_sign_positive() {
            "\"+Inf\""
        } else {
            "\"-Inf\""
        }
        .to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        #[allow(clippy::cast_possible_truncation)]
        return format!("{}", v as i64);
    }
    format!("{v}")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn make_sample(name: &str, value: f64, mt: MetricType) -> MetricSample {
        MetricSample {
            name: name.to_string(),
            labels: HashMap::new(),
            value,
            metric_type: mt,
        }
    }

    fn make_sample_with_labels(
        name: &str,
        value: f64,
        mt: MetricType,
        labels: Vec<(&str, &str)>,
    ) -> MetricSample {
        MetricSample {
            name: name.to_string(),
            labels: labels
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            value,
            metric_type: mt,
        }
    }

    #[test]
    fn test_filter_no_pattern() {
        let samples = vec![
            make_sample("cpu_total", 1.0, MetricType::Counter),
            make_sample("mem_bytes", 2.0, MetricType::Gauge),
        ];
        let filtered = filter_samples(&samples, None);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_filter_plain_substring() {
        let samples = vec![
            make_sample("loader_kafka_lag", 10.0, MetricType::Gauge),
            make_sample("loader_buffer_rows", 20.0, MetricType::Gauge),
            make_sample("loader_kafka_offsets", 30.0, MetricType::Counter),
        ];
        let filtered = filter_samples(&samples, Some("kafka"));
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].name, "loader_kafka_lag");
        assert_eq!(filtered[1].name, "loader_kafka_offsets");
    }

    #[test]
    fn test_filter_wildcard_pattern() {
        let samples = vec![
            make_sample("loader_buffer_rows", 10.0, MetricType::Gauge),
            make_sample("loader_buffer_bytes", 20.0, MetricType::Gauge),
            make_sample("loader_insert_total", 30.0, MetricType::Counter),
        ];
        let filtered = filter_samples(&samples, Some("buffer.*rows"));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "loader_buffer_rows");
    }

    #[test]
    fn test_filter_no_match() {
        let samples = vec![make_sample("cpu_total", 1.0, MetricType::Counter)];
        let filtered = filter_samples(&samples, Some("nonexistent"));
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_format_value_integer() {
        let sample = make_sample("test", 42.0, MetricType::Counter);
        assert_eq!(format_value(&sample), "42");
    }

    #[test]
    fn test_format_value_float() {
        let sample = make_sample("test", 3.14, MetricType::Gauge);
        assert_eq!(format_value(&sample), "3.14");
    }

    #[test]
    fn test_format_value_small() {
        let sample = make_sample("test", 0.000_123, MetricType::Gauge);
        assert_eq!(format_value(&sample), "0.000123");
    }

    #[test]
    fn test_format_value_json_nan() {
        assert_eq!(format_value_json(f64::NAN), "\"NaN\"");
    }

    #[test]
    fn test_format_value_json_inf() {
        assert_eq!(format_value_json(f64::INFINITY), "\"+Inf\"");
        assert_eq!(format_value_json(f64::NEG_INFINITY), "\"-Inf\"");
    }

    #[test]
    fn test_format_value_json_integer() {
        assert_eq!(format_value_json(42.0), "42");
    }

    #[test]
    fn test_format_value_json_float() {
        assert_eq!(format_value_json(3.14), "3.14");
    }

    #[test]
    fn test_has_regex_chars() {
        assert!(!has_regex_chars("kafka_lag"));
        assert!(has_regex_chars("kafka.*lag"));
        assert!(has_regex_chars("^kafka"));
        assert!(has_regex_chars("lag$"));
        assert!(has_regex_chars("kafka.lag"));
    }

    #[test]
    fn test_match_filter_simple_contains() {
        assert!(match_filter("loader_kafka_lag", "kafka"));
        assert!(!match_filter("loader_buffer_rows", "kafka"));
    }

    #[test]
    fn test_match_filter_wildcard() {
        assert!(match_filter("loader_kafka_lag", "loader.*lag"));
        assert!(!match_filter("loader_kafka_lag", "buffer.*lag"));
    }

    #[test]
    fn test_print_table_with_labels() {
        // Verify it doesn't panic with labelled samples
        let sample = make_sample_with_labels(
            "kafka_lag",
            42.0,
            MetricType::Gauge,
            vec![("topic", "events"), ("partition", "0")],
        );
        let samples = vec![&sample];
        // Just verify it doesn't panic — TSV output goes to stdout
        print_table(&samples, "http://localhost:9090/metrics");
    }

    #[test]
    fn test_format_labels_tsv_empty() {
        assert_eq!(format_labels_tsv(&HashMap::new()), "");
    }

    #[test]
    fn test_format_labels_tsv_sorted() {
        let mut labels = HashMap::new();
        labels.insert("topic".to_string(), "events".to_string());
        labels.insert("partition".to_string(), "0".to_string());
        let result = format_labels_tsv(&labels);
        // Labels are sorted alphabetically
        assert_eq!(result, "partition=0,topic=events");
    }
}
