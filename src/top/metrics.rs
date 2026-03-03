// Project:   hyperi-rustlib
// File:      src/top/metrics.rs
// Purpose:   Prometheus text format parser and HTTP metrics fetcher
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Prometheus text exposition format parser and HTTP scraper.
//!
//! Parses the plain-text format from `/metrics` endpoints into typed samples.
//! Uses raw TCP for HTTP GET to avoid async runtime conflicts in the TUI loop.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use super::TopError;

/// A single metric sample parsed from Prometheus text format.
#[derive(Debug, Clone)]
pub struct MetricSample {
    /// Metric name (e.g. `kafka_messages_consumed_total`).
    pub name: String,

    /// Label key-value pairs (e.g. `{topic="events"}`).
    pub labels: HashMap<String, String>,

    /// Metric value.
    pub value: f64,

    /// Metric type from `# TYPE` declaration.
    pub metric_type: MetricType,
}

impl MetricSample {
    /// Format labels as a compact string: `{key="val", ...}` or empty string.
    #[must_use]
    pub fn labels_string(&self) -> String {
        if self.labels.is_empty() {
            return String::new();
        }
        let pairs: Vec<String> = self
            .labels
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        format!("{{{}}}", pairs.join(", "))
    }

    /// Get the display name including labels.
    #[must_use]
    pub fn display_name(&self) -> String {
        let labels = self.labels_string();
        if labels.is_empty() {
            self.name.clone()
        } else {
            format!("{}{labels}", self.name)
        }
    }
}

/// Prometheus metric type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MetricType {
    Counter,
    Gauge,
    Histogram,
    Summary,
    #[default]
    Untyped,
}

impl MetricType {
    /// Short display label.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Counter => "counter",
            Self::Gauge => "gauge",
            Self::Histogram => "histogram",
            Self::Summary => "summary",
            Self::Untyped => "untyped",
        }
    }
}

/// Result of scraping a metrics endpoint.
#[derive(Debug, Clone, Default)]
pub struct ScrapeResult {
    /// All parsed metric samples.
    pub samples: Vec<MetricSample>,

    /// HELP text per metric name.
    pub help: HashMap<String, String>,
}

/// Parse Prometheus text exposition format into metric samples.
///
/// Handles `# HELP`, `# TYPE`, and metric lines. Skips blank lines
/// and unknown comments.
#[must_use]
pub fn parse_prometheus(text: &str) -> ScrapeResult {
    let mut result = ScrapeResult::default();
    let mut types: HashMap<String, MetricType> = HashMap::new();

    for line in text.lines() {
        let line = line.trim();

        if line.is_empty() {
            continue;
        }

        // # HELP metric_name Some description
        if let Some(rest) = line.strip_prefix("# HELP ") {
            if let Some((name, help)) = rest.split_once(' ') {
                result.help.insert(name.to_string(), help.to_string());
            }
            continue;
        }

        // # TYPE metric_name type
        if let Some(rest) = line.strip_prefix("# TYPE ") {
            if let Some((name, type_str)) = rest.split_once(' ') {
                let mt = match type_str {
                    "counter" => MetricType::Counter,
                    "gauge" => MetricType::Gauge,
                    "histogram" => MetricType::Histogram,
                    "summary" => MetricType::Summary,
                    _ => MetricType::Untyped,
                };
                types.insert(name.to_string(), mt);
            }
            continue;
        }

        // Skip other comments
        if line.starts_with('#') {
            continue;
        }

        // Parse metric line: name{labels} value [timestamp]
        if let Some(sample) = parse_metric_line(line, &types) {
            result.samples.push(sample);
        }
    }

    result
}

/// Parse a single metric line.
fn parse_metric_line(line: &str, types: &HashMap<String, MetricType>) -> Option<MetricSample> {
    let (name_and_labels, rest) = if let Some(brace_start) = line.find('{') {
        // Has labels: metric_name{labels} value
        let brace_end = line.find('}')?;
        let name = &line[..brace_start];
        let label_str = &line[brace_start + 1..brace_end];
        let after_brace = line[brace_end + 1..].trim();
        let labels = parse_labels(label_str);
        ((name.to_string(), labels), after_brace.to_string())
    } else {
        // No labels: metric_name value
        let (name, value_str) = line.split_once(' ')?;
        ((name.to_string(), HashMap::new()), value_str.to_string())
    };

    let (name, labels) = name_and_labels;

    // Parse value (first token, ignore optional timestamp)
    let value_str = rest.split_whitespace().next()?;
    let value = parse_value(value_str)?;

    // Look up metric type from earlier # TYPE declaration
    // Strip suffixes for histogram/summary sub-metrics
    let base_name = strip_metric_suffix(&name);
    let metric_type = types
        .get(&name)
        .or_else(|| types.get(base_name))
        .copied()
        .unwrap_or(MetricType::Untyped);

    Some(MetricSample {
        name,
        labels,
        value,
        metric_type,
    })
}

/// Parse label pairs from `key="value",key2="value2"` format.
fn parse_labels(label_str: &str) -> HashMap<String, String> {
    let mut labels = HashMap::new();

    for pair in split_label_pairs(label_str) {
        if let Some((key, value)) = pair.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"');
            if !key.is_empty() {
                labels.insert(key.to_string(), value.to_string());
            }
        }
    }

    labels
}

/// Split label string by commas, respecting quoted values.
fn split_label_pairs(s: &str) -> Vec<&str> {
    let mut pairs = Vec::new();
    let mut start = 0;
    let mut in_quotes = false;

    for (i, c) in s.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                let pair = s[start..i].trim();
                if !pair.is_empty() {
                    pairs.push(pair);
                }
                start = i + 1;
            }
            _ => {}
        }
    }

    // Last pair
    let last = s[start..].trim();
    if !last.is_empty() {
        pairs.push(last);
    }

    pairs
}

/// Parse a Prometheus value (handles NaN, +Inf, -Inf).
fn parse_value(s: &str) -> Option<f64> {
    match s {
        "NaN" => Some(f64::NAN),
        "+Inf" => Some(f64::INFINITY),
        "-Inf" => Some(f64::NEG_INFINITY),
        _ => s.parse().ok(),
    }
}

/// Strip histogram/summary suffixes to find base metric name.
fn strip_metric_suffix(name: &str) -> &str {
    for suffix in &["_bucket", "_count", "_sum", "_total", "_created", "_info"] {
        if let Some(base) = name.strip_suffix(suffix) {
            return base;
        }
    }
    name
}

/// Fetch metrics from an HTTP endpoint using raw TCP.
///
/// Simple HTTP/1.0 GET — suitable for Prometheus `/metrics` endpoints
/// which return plain text without chunked encoding.
///
/// # Errors
///
/// Returns `TopError::Fetch` if the connection or response fails.
pub fn fetch_metrics_http(url: &str) -> Result<String, TopError> {
    let (host, port, path) = parse_http_url(url)?;

    let addr = format!("{host}:{port}");
    let mut stream = TcpStream::connect_timeout(
        &addr
            .parse()
            .map_err(|e| TopError::Fetch(format!("invalid address {addr}: {e}")))?,
        Duration::from_secs(5),
    )
    .map_err(|e| TopError::Fetch(format!("connect to {addr}: {e}")))?;

    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| TopError::Fetch(format!("set timeout: {e}")))?;

    // Send HTTP/1.0 GET (server will close connection after response)
    write!(
        stream,
        "GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| TopError::Fetch(format!("send request: {e}")))?;

    // Read response
    let reader = BufReader::new(stream);
    let mut body = String::new();
    let mut in_body = false;
    let mut status_ok = false;

    for line in reader.lines() {
        let line = line.map_err(|e| TopError::Fetch(format!("read response: {e}")))?;

        if in_body {
            body.push_str(&line);
            body.push('\n');
        } else {
            // Check status line
            if !status_ok && line.starts_with("HTTP/") {
                status_ok = line.contains("200");
            }
            // Empty line separates headers from body
            if line.is_empty() {
                in_body = true;
            }
        }
    }

    if !status_ok {
        return Err(TopError::Fetch(format!("non-200 response from {url}")));
    }

    Ok(body)
}

/// Parse `http://host:port/path` into components.
fn parse_http_url(url: &str) -> Result<(String, u16, String), TopError> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| TopError::Fetch(format!("URL must start with http:// — got {url}")))?;

    let (host_port, path) = rest
        .split_once('/')
        .map(|(hp, p)| (hp, format!("/{p}")))
        .unwrap_or((rest, "/metrics".to_string()));

    let (host, port) = match host_port.split_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(9090)),
        None => (host_port.to_string(), 80),
    };

    Ok((host, port, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_metric() {
        let text = "process_cpu_seconds_total 0.52\n";
        let result = parse_prometheus(text);
        assert_eq!(result.samples.len(), 1);
        assert_eq!(result.samples[0].name, "process_cpu_seconds_total");
        assert!(result.samples[0].labels.is_empty());
        assert!((result.samples[0].value - 0.52).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_metric_with_labels() {
        let text = r#"kafka_messages_consumed_total{topic="events"} 12345"#;
        let result = parse_prometheus(text);
        assert_eq!(result.samples.len(), 1);
        assert_eq!(result.samples[0].name, "kafka_messages_consumed_total");
        assert_eq!(result.samples[0].labels.get("topic").unwrap(), "events");
        assert!((result.samples[0].value - 12345.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_metric_with_multiple_labels() {
        let text = r#"http_requests_total{method="GET", status="200"} 42"#;
        let result = parse_prometheus(text);
        assert_eq!(result.samples.len(), 1);
        assert_eq!(result.samples[0].labels.get("method").unwrap(), "GET");
        assert_eq!(result.samples[0].labels.get("status").unwrap(), "200");
    }

    #[test]
    fn test_parse_type_and_help() {
        let text = concat!(
            "# HELP process_cpu CPU time\n",
            "# TYPE process_cpu counter\n",
            "process_cpu 1.5\n",
        );
        let result = parse_prometheus(text);
        assert_eq!(result.help.get("process_cpu").unwrap(), "CPU time");
        assert_eq!(result.samples[0].metric_type, MetricType::Counter);
    }

    #[test]
    fn test_parse_gauge_type() {
        let text = concat!(
            "# TYPE buffer_rows gauge\n",
            r#"buffer_rows{table="auth"} 500"#,
            "\n",
        );
        let result = parse_prometheus(text);
        assert_eq!(result.samples[0].metric_type, MetricType::Gauge);
        assert_eq!(result.samples[0].labels.get("table").unwrap(), "auth");
    }

    #[test]
    fn test_parse_histogram_suffix_type() {
        let text = concat!(
            "# TYPE http_duration histogram\n",
            r#"http_duration_bucket{le="0.05"} 24054"#,
            "\n",
            "http_duration_sum 53423\n",
            "http_duration_count 144320\n",
        );
        let result = parse_prometheus(text);
        assert_eq!(result.samples.len(), 3);
        // _bucket, _sum, _count all resolve to histogram type via base name
        assert_eq!(result.samples[0].metric_type, MetricType::Histogram);
        assert_eq!(result.samples[1].metric_type, MetricType::Histogram);
        assert_eq!(result.samples[2].metric_type, MetricType::Histogram);
    }

    #[test]
    fn test_parse_special_values() {
        let text = concat!(
            "metric_nan NaN\n",
            "metric_inf +Inf\n",
            "metric_neg_inf -Inf\n",
        );
        let result = parse_prometheus(text);
        assert!(result.samples[0].value.is_nan());
        assert!(result.samples[1].value.is_infinite());
        assert!(
            result.samples[2].value.is_infinite() && result.samples[2].value.is_sign_negative()
        );
    }

    #[test]
    fn test_parse_skips_blank_and_comments() {
        let text = concat!(
            "# This is a random comment\n",
            "\n",
            "metric_a 1\n",
            "\n",
            "# Another comment\n",
            "metric_b 2\n",
        );
        let result = parse_prometheus(text);
        assert_eq!(result.samples.len(), 2);
    }

    #[test]
    fn test_parse_metric_with_timestamp() {
        // Timestamps are optional and should be ignored
        let text = "metric_a 42 1625000000000\n";
        let result = parse_prometheus(text);
        assert_eq!(result.samples.len(), 1);
        assert!((result.samples[0].value - 42.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_labels_string_empty() {
        let sample = MetricSample {
            name: "test".into(),
            labels: HashMap::new(),
            value: 1.0,
            metric_type: MetricType::Gauge,
        };
        assert_eq!(sample.labels_string(), "");
    }

    #[test]
    fn test_display_name_with_labels() {
        let mut labels = HashMap::new();
        labels.insert("topic".into(), "events".into());
        let sample = MetricSample {
            name: "kafka_msgs".into(),
            labels,
            value: 1.0,
            metric_type: MetricType::Counter,
        };
        let name = sample.display_name();
        assert!(name.starts_with("kafka_msgs{"));
        assert!(name.contains("topic=events"));
    }

    #[test]
    fn test_parse_http_url() {
        let (host, port, path) = parse_http_url("http://127.0.0.1:9090/metrics").unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 9090);
        assert_eq!(path, "/metrics");
    }

    #[test]
    fn test_parse_http_url_no_port() {
        let (host, port, path) = parse_http_url("http://localhost/health").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 80);
        assert_eq!(path, "/health");
    }

    #[test]
    fn test_parse_http_url_no_path() {
        let (host, port, path) = parse_http_url("http://localhost:9090").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 9090);
        assert_eq!(path, "/metrics");
    }

    #[test]
    fn test_parse_http_url_invalid_scheme() {
        assert!(parse_http_url("https://localhost/metrics").is_err());
    }

    #[test]
    fn test_metric_type_as_str() {
        assert_eq!(MetricType::Counter.as_str(), "counter");
        assert_eq!(MetricType::Gauge.as_str(), "gauge");
        assert_eq!(MetricType::Histogram.as_str(), "histogram");
        assert_eq!(MetricType::Summary.as_str(), "summary");
        assert_eq!(MetricType::Untyped.as_str(), "untyped");
    }

    #[test]
    fn test_quoted_label_with_comma() {
        let text = r#"metric{path="/api/v1,v2",method="GET"} 1"#;
        let result = parse_prometheus(text);
        assert_eq!(result.samples.len(), 1);
        assert_eq!(result.samples[0].labels.get("path").unwrap(), "/api/v1,v2");
        assert_eq!(result.samples[0].labels.get("method").unwrap(), "GET");
    }
}
