// Project:   hyperi-rustlib
// File:      src/dlq/redis_dlq.rs
// Purpose:   Redis Streams DLQ backend variant
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Redis Streams backend variant for the DLQ enum.
//!
//! Writes failed messages to a Redis Stream via `XADD`. Supports
//! optional `MAXLEN ~` trimming to bound stream size. The connection
//! is a `MultiplexedConnection` -- async-native, no `spawn_blocking`
//! needed.
//!
//! Single-batch sends are issued via Redis pipelining (one round-trip
//! per batch) when the backend serves more than one entry per call.

use serde::{Deserialize, Serialize};

use super::entry::DlqEntry;
use super::error::DlqError;

/// Configuration for the Redis DLQ backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RedisDlqConfig {
    /// Enable the Redis backend.
    pub enabled: bool,

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
            enabled: false,
            url: "redis://127.0.0.1:6379".into(),
            stream_key: "dlq".into(),
            max_len: None,
        }
    }
}

/// Redis backend -- internal variant carried by [`super::DlqBackend::Redis`].
pub struct RedisDlqInner {
    conn: redis::aio::MultiplexedConnection,
    stream_key: String,
    max_len: Option<usize>,
}

impl std::fmt::Debug for RedisDlqInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisDlqInner")
            .field("stream_key", &self.stream_key)
            .field("max_len", &self.max_len)
            .finish_non_exhaustive()
    }
}

impl RedisDlqInner {
    /// Build the Redis backend. Opens a multiplexed async connection.
    ///
    /// # Errors
    ///
    /// Returns an error if the Redis connection cannot be established.
    pub async fn new(config: &RedisDlqConfig) -> Result<Self, DlqError> {
        let client = redis::Client::open(config.url.as_str())
            .map_err(|e| DlqError::BackendError(format!("Redis DLQ open: {e}")))?;
        let conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| DlqError::BackendError(format!("Redis DLQ connect: {e}")))?;
        Ok(Self {
            conn,
            stream_key: config.stream_key.clone(),
            max_len: config.max_len,
        })
    }

    /// Send a batch via pipelined `XADD`s (single round-trip).
    pub async fn send_batch(&mut self, batch: &[DlqEntry]) -> Result<(), DlqError> {
        if batch.is_empty() {
            return Ok(());
        }

        let mut pipe = redis::pipe();
        for entry in batch {
            let json =
                serde_json::to_string(entry).map_err(|e| DlqError::Serialization(e.to_string()))?;
            let mut cmd = redis::cmd("XADD");
            cmd.arg(&self.stream_key);
            if let Some(max_len) = self.max_len {
                cmd.arg("MAXLEN").arg("~").arg(max_len);
            }
            cmd.arg("*").arg("data").arg(&json);
            pipe.add_command(cmd);
        }

        pipe.query_async::<()>(&mut self.conn)
            .await
            .map_err(|e| DlqError::BackendError(format!("Redis XADD batch: {e}")))?;

        #[cfg(feature = "metrics")]
        metrics::counter!("dfe_dlq_sent_total", "backend" => "redis").increment(batch.len() as u64);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults() {
        let config = RedisDlqConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.url, "redis://127.0.0.1:6379");
        assert_eq!(config.stream_key, "dlq");
        assert!(config.max_len.is_none());
    }

    #[test]
    fn config_deserialise() {
        let json = r#"{"enabled":true,"url":"redis://redis:6379","stream_key":"failed_events","max_len":10000}"#;
        let config: RedisDlqConfig = serde_json::from_str(json).unwrap();
        assert!(config.enabled);
        assert_eq!(config.url, "redis://redis:6379");
        assert_eq!(config.stream_key, "failed_events");
        assert_eq!(config.max_len, Some(10000));
    }
}
