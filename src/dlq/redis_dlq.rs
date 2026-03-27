// Project:   hyperi-rustlib
// File:      src/dlq/redis_dlq.rs
// Purpose:   Redis Streams DLQ backend — XADD failed messages to a stream
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Redis Streams DLQ backend.
//!
//! Writes failed messages to a Redis Stream via `XADD`. Supports optional
//! `MAXLEN` trimming to bound stream size.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::backend::DlqBackend;
use super::entry::DlqEntry;
use super::error::DlqError;

/// Configuration for the Redis DLQ backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RedisDlqConfig {
    /// Redis connection URL (e.g. `redis://localhost:6379`).
    pub url: String,

    /// Stream key to write DLQ entries to.
    pub stream_key: String,

    /// Optional maximum stream length (approximate trimming via `MAXLEN ~`).
    pub max_len: Option<usize>,
}

impl Default for RedisDlqConfig {
    fn default() -> Self {
        Self {
            url: "redis://127.0.0.1:6379".into(),
            stream_key: "dlq".into(),
            max_len: None,
        }
    }
}

/// Redis Streams DLQ backend.
///
/// Writes each DLQ entry as a stream entry with a `data` field containing
/// the JSON-serialised [`DlqEntry`].
pub struct RedisDlq {
    conn: Mutex<redis::aio::MultiplexedConnection>,
    config: RedisDlqConfig,
}

impl RedisDlq {
    /// Create a new Redis DLQ backend.
    ///
    /// # Errors
    ///
    /// Returns error if the connection to Redis fails.
    pub async fn new(config: RedisDlqConfig) -> Result<Self, DlqError> {
        let client = redis::Client::open(config.url.as_str())
            .map_err(|e| DlqError::BackendError(format!("Redis DLQ connect failed: {e}")))?;

        let conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| DlqError::BackendError(format!("Redis DLQ connect failed: {e}")))?;

        Ok(Self {
            conn: Mutex::new(conn),
            config,
        })
    }
}

#[async_trait]
impl DlqBackend for RedisDlq {
    async fn send(&self, entry: &DlqEntry) -> Result<(), DlqError> {
        let json =
            serde_json::to_string(entry).map_err(|e| DlqError::Serialization(e.to_string()))?;

        let mut conn = self.conn.lock().await;

        let mut cmd = redis::cmd("XADD");
        cmd.arg(&self.config.stream_key);
        if let Some(max_len) = self.config.max_len {
            cmd.arg("MAXLEN").arg("~").arg(max_len);
        }
        cmd.arg("*").arg("data").arg(&json);

        cmd.query_async::<String>(&mut *conn)
            .await
            .map_err(|e| DlqError::BackendError(format!("Redis XADD failed: {e}")))?;

        #[cfg(feature = "metrics")]
        metrics::counter!("dfe_dlq_sent_total", "backend" => "redis").increment(1);

        Ok(())
    }

    fn name(&self) -> &'static str {
        "redis"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let config = RedisDlqConfig::default();
        assert_eq!(config.url, "redis://127.0.0.1:6379");
        assert_eq!(config.stream_key, "dlq");
        assert!(config.max_len.is_none());
    }

    #[test]
    fn config_deserialise() {
        let json = r#"{"url":"redis://redis:6379","stream_key":"failed_events","max_len":10000}"#;
        let config: RedisDlqConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.url, "redis://redis:6379");
        assert_eq!(config.stream_key, "failed_events");
        assert_eq!(config.max_len, Some(10000));
    }
}
