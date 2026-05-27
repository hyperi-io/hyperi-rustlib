// Project:   hyperi-rustlib
// File:      src/dlq/http.rs
// Purpose:   HTTP POST DLQ backend variant
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! HTTP backend variant for the DLQ enum.
//!
//! POSTs failed messages as NDJSON to a configured HTTP endpoint via
//! reqwest. The reqwest client is async-native so no `spawn_blocking`
//! is needed.

use serde::{Deserialize, Serialize};

use super::entry::DlqEntry;
use super::error::DlqError;

/// Configuration for the HTTP DLQ backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HttpDlqConfig {
    /// Enable the HTTP backend.
    pub enabled: bool,

    /// Endpoint URL to POST failed messages to.
    pub endpoint: String,

    /// Request timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for HttpDlqConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: String::new(),
            timeout_secs: 30,
        }
    }
}

/// HTTP backend -- internal variant carried by [`super::DlqBackend::Http`].
#[derive(Debug)]
pub struct HttpDlqInner {
    client: reqwest::Client,
    endpoint: String,
}

impl HttpDlqInner {
    /// Build the HTTP backend.
    ///
    /// # Errors
    ///
    /// Returns an error if `endpoint` is empty or the reqwest client
    /// cannot be built.
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

    /// Send a batch as `application/x-ndjson`.
    pub async fn send_batch(&mut self, batch: &[DlqEntry]) -> Result<(), DlqError> {
        if batch.is_empty() {
            return Ok(());
        }

        let mut body = Vec::with_capacity(batch.len() * 256);
        for entry in batch {
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
        metrics::counter!("dfe_dlq_sent_total", "backend" => "http").increment(batch.len() as u64);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let config = HttpDlqConfig::default();
        assert!(!config.enabled);
        assert!(config.endpoint.is_empty());
        assert_eq!(config.timeout_secs, 30);
    }

    #[test]
    fn config_deserialise() {
        let json =
            r#"{"enabled":true,"endpoint":"http://dlq.example.com/ingest","timeout_secs":10}"#;
        let config: HttpDlqConfig = serde_json::from_str(json).unwrap();
        assert!(config.enabled);
        assert_eq!(config.endpoint, "http://dlq.example.com/ingest");
        assert_eq!(config.timeout_secs, 10);
    }

    #[test]
    fn rejects_empty_endpoint() {
        let config = HttpDlqConfig::default();
        assert!(HttpDlqInner::new(&config).is_err());
    }
}
