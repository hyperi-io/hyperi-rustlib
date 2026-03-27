// Project:   hyperi-rustlib
// File:      src/dlq/http.rs
// Purpose:   HTTP DLQ backend — POST failed messages to an HTTP endpoint
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! HTTP DLQ backend.
//!
//! POSTs failed messages to a configurable HTTP endpoint as JSON (single)
//! or NDJSON (batch). Uses reqwest with configurable timeout.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::backend::DlqBackend;
use super::entry::DlqEntry;
use super::error::DlqError;

/// Configuration for the HTTP DLQ backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpDlqConfig {
    /// Endpoint URL to POST failed messages to.
    pub endpoint: String,

    /// Request timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for HttpDlqConfig {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            timeout_secs: 30,
        }
    }
}

/// HTTP DLQ backend.
///
/// Sends failed messages as JSON to a configured HTTP endpoint.
/// Single entries are sent as `application/json`; batches as
/// `application/x-ndjson` (newline-delimited JSON).
pub struct HttpDlq {
    client: reqwest::Client,
    endpoint: String,
}

impl HttpDlq {
    /// Create a new HTTP DLQ backend.
    ///
    /// # Errors
    ///
    /// Returns error if the endpoint is empty or the HTTP client fails to build.
    pub fn new(config: &HttpDlqConfig) -> Result<Self, DlqError> {
        if config.endpoint.is_empty() {
            return Err(DlqError::BackendError("HTTP DLQ endpoint is empty".into()));
        }

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| DlqError::BackendError(format!("failed to build HTTP DLQ client: {e}")))?;

        Ok(Self {
            client,
            endpoint: config.endpoint.clone(),
        })
    }
}

#[async_trait]
impl DlqBackend for HttpDlq {
    async fn send(&self, entry: &DlqEntry) -> Result<(), DlqError> {
        let body = serde_json::to_vec(entry).map_err(|e| DlqError::Serialization(e.to_string()))?;

        let resp = self
            .client
            .post(&self.endpoint)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| DlqError::BackendError(format!("HTTP DLQ send failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(DlqError::BackendError(format!(
                "HTTP DLQ endpoint returned {}",
                resp.status()
            )));
        }

        #[cfg(feature = "metrics")]
        metrics::counter!("dfe_dlq_sent_total", "backend" => "http").increment(1);

        Ok(())
    }

    async fn send_batch(&self, entries: &[DlqEntry]) -> Result<(), DlqError> {
        if entries.is_empty() {
            return Ok(());
        }

        let mut body = Vec::new();
        for entry in entries {
            serde_json::to_writer(&mut body, entry)
                .map_err(|e| DlqError::Serialization(e.to_string()))?;
            body.push(b'\n');
        }

        let resp = self
            .client
            .post(&self.endpoint)
            .header("content-type", "application/x-ndjson")
            .body(body)
            .send()
            .await
            .map_err(|e| DlqError::BackendError(format!("HTTP DLQ batch send failed: {e}")))?;

        if !resp.status().is_success() {
            return Err(DlqError::BackendError(format!(
                "HTTP DLQ endpoint returned {}",
                resp.status()
            )));
        }

        #[cfg(feature = "metrics")]
        metrics::counter!("dfe_dlq_sent_total", "backend" => "http")
            .increment(entries.len() as u64);

        Ok(())
    }

    fn name(&self) -> &'static str {
        "http"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let config = HttpDlqConfig::default();
        assert!(config.endpoint.is_empty());
        assert_eq!(config.timeout_secs, 30);
    }

    #[test]
    fn config_deserialise() {
        let json = r#"{"endpoint":"http://dlq.example.com/ingest","timeout_secs":10}"#;
        let config: HttpDlqConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.endpoint, "http://dlq.example.com/ingest");
        assert_eq!(config.timeout_secs, 10);
    }

    #[test]
    fn rejects_empty_endpoint() {
        let config = HttpDlqConfig::default();
        assert!(HttpDlq::new(&config).is_err());
    }
}
