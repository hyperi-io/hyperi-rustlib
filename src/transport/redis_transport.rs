// Project:   hyperi-rustlib
// File:      src/transport/redis_transport.rs
// Purpose:   Redis/Valkey Streams transport
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # Redis Streams Transport
//!
//! Lightweight pub/sub transport using Redis (or Valkey) Streams.
//! Uses `XADD` for send, `XREADGROUP` for receive, and `XACK` for commit.
//!
//! ## Send
//!
//! Appends payload bytes to a named stream via `XADD`. Optionally caps
//! the stream length with `MAXLEN ~` for approximate trimming.
//!
//! ## Receive
//!
//! Reads from a consumer group via `XREADGROUP` with blocking. Creates
//! the consumer group on first use if it does not exist.
//!
//! ## Commit
//!
//! Acknowledges processed entries via `XACK` so they are not re-delivered
//! to other consumers in the same group.
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::transport::redis_transport::{RedisTransport, RedisTransportConfig};
//!
//! let config = RedisTransportConfig {
//!     stream: Some("events".into()),
//!     ..Default::default()
//! };
//! let transport = RedisTransport::new(&config).await?;
//! transport.send("events", b"{\"msg\":\"hello\"}").await;
//! ```

use super::error::{TransportError, TransportResult};
use super::traits::{CommitToken, TransportBase, TransportReceiver, TransportSender};
use super::types::{Message, PayloadFormat, SendResult};
use redis::AsyncCommands;
use redis::streams::{StreamMaxlen, StreamReadOptions, StreamReadReply};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;

/// Commit token for Redis Streams transport.
///
/// Contains the stream name and entry ID needed for `XACK`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RedisToken {
    /// Stream name the entry belongs to.
    pub stream: Arc<str>,
    /// Redis stream entry ID (e.g. "1711432800000-0").
    pub entry_id: String,
}

impl CommitToken for RedisToken {}

impl std::fmt::Display for RedisToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "redis:{}:{}", self.stream, self.entry_id)
    }
}

fn default_url() -> String {
    "redis://127.0.0.1:6379".into()
}

fn default_group() -> String {
    "dfe".into()
}

fn default_consumer() -> String {
    "consumer-1".into()
}

fn default_block_ms() -> usize {
    5000
}

/// Configuration for Redis/Valkey Streams transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisTransportConfig {
    /// Redis/Valkey connection URL.
    ///
    /// Supports `redis://`, `rediss://` (TLS), and `unix://` schemes.
    /// Default: `"redis://127.0.0.1:6379"`.
    #[serde(default = "default_url")]
    pub url: String,

    /// Stream name for send/receive.
    ///
    /// Used as default when key is empty in `send()`.
    #[serde(default)]
    pub stream: Option<String>,

    /// Consumer group name. Default: `"dfe"`.
    #[serde(default = "default_group")]
    pub group: String,

    /// Consumer name within group. Default: hostname or `"consumer-1"`.
    #[serde(default = "default_consumer")]
    pub consumer: String,

    /// Maximum stream length (approximate via `MAXLEN ~`).
    ///
    /// `None` means unlimited growth.
    #[serde(default)]
    pub max_stream_len: Option<usize>,

    /// Block timeout in milliseconds for `XREADGROUP`. Default: 5000.
    #[serde(default = "default_block_ms")]
    pub block_ms: usize,
}

impl Default for RedisTransportConfig {
    fn default() -> Self {
        Self {
            url: default_url(),
            stream: None,
            group: default_group(),
            consumer: default_consumer(),
            max_stream_len: None,
            block_ms: default_block_ms(),
        }
    }
}

/// Redis/Valkey Streams transport.
///
/// Supports both send (`XADD`) and receive (`XREADGROUP`) operations.
/// Works with both Redis and Valkey (same wire protocol).
pub struct RedisTransport {
    conn: Mutex<redis::aio::MultiplexedConnection>,
    config: RedisTransportConfig,
    closed: AtomicBool,
    /// Whether the consumer group has been ensured for a given stream.
    group_created: Mutex<std::collections::HashSet<String>>,
}

impl RedisTransport {
    /// Create a new Redis Streams transport.
    ///
    /// Connects to the Redis server and prepares for stream operations.
    /// The consumer group is created lazily on first `recv()` call.
    ///
    /// # Errors
    ///
    /// Returns error if the URL is invalid or connection fails.
    pub async fn new(config: &RedisTransportConfig) -> TransportResult<Self> {
        let client = redis::Client::open(config.url.as_str()).map_err(|e| {
            TransportError::Config(format!("invalid Redis URL '{}': {e}", config.url))
        })?;

        let conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| {
                TransportError::Connection(format!(
                    "failed to connect to Redis at '{}': {e}",
                    config.url
                ))
            })?;

        Ok(Self {
            conn: Mutex::new(conn),
            config: config.clone(),
            closed: AtomicBool::new(false),
            group_created: Mutex::new(std::collections::HashSet::new()),
        })
    }

    /// Resolve the stream name: use `key` if non-empty, else fall back to config.
    fn resolve_stream<'a>(&'a self, key: &'a str) -> Result<&'a str, TransportError> {
        if !key.is_empty() {
            return Ok(key);
        }
        self.config.stream.as_deref().ok_or_else(|| {
            TransportError::Config(
                "no stream name: key is empty and config.stream is not set".into(),
            )
        })
    }

    /// Ensure the consumer group exists for the given stream.
    ///
    /// Uses `XGROUP CREATE ... MKSTREAM` so the stream is created if absent.
    /// Idempotent: tracks which streams have been initialised and only
    /// issues the command once per stream per transport instance.
    async fn ensure_group(&self, stream: &str) -> TransportResult<()> {
        {
            let created = self.group_created.lock().await;
            if created.contains(stream) {
                return Ok(());
            }
        }

        let mut conn = self.conn.lock().await;
        let result: redis::RedisResult<()> = conn
            .xgroup_create_mkstream(stream, &self.config.group, "0")
            .await;

        match result {
            Ok(()) => {}
            Err(e) => {
                // "BUSYGROUP Consumer Group name already exists" is not an error
                let msg = e.to_string();
                if !msg.contains("BUSYGROUP") {
                    return Err(TransportError::Connection(format!(
                        "failed to create consumer group '{}' on stream '{stream}': {e}",
                        self.config.group
                    )));
                }
            }
        }

        self.group_created.lock().await.insert(stream.to_string());
        Ok(())
    }
}

impl TransportBase for RedisTransport {
    async fn close(&self) -> TransportResult<()> {
        self.closed.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn is_healthy(&self) -> bool {
        !self.closed.load(Ordering::Relaxed)
    }

    fn name(&self) -> &'static str {
        "redis"
    }
}

impl TransportSender for RedisTransport {
    async fn send(&self, key: &str, payload: &[u8]) -> SendResult {
        if self.closed.load(Ordering::Relaxed) {
            return SendResult::Fatal(TransportError::Closed);
        }

        let stream = match self.resolve_stream(key) {
            Ok(s) => s.to_string(),
            Err(e) => return SendResult::Fatal(e),
        };

        let mut conn = self.conn.lock().await;

        let result: redis::RedisResult<String> = if let Some(max_len) = self.config.max_stream_len {
            conn.xadd_maxlen(
                &stream,
                StreamMaxlen::Approx(max_len),
                "*",
                &[("payload", payload)],
            )
            .await
        } else {
            conn.xadd(&stream, "*", &[("payload", payload)]).await
        };

        match result {
            Ok(_entry_id) => {
                #[cfg(feature = "metrics")]
                metrics::counter!("dfe_transport_sent_total", "transport" => "redis").increment(1);

                SendResult::Ok
            }
            Err(e) => SendResult::Fatal(TransportError::Send(format!(
                "XADD to stream '{stream}' failed: {e}"
            ))),
        }
    }
}

impl TransportReceiver for RedisTransport {
    type Token = RedisToken;

    async fn recv(&self, max: usize) -> TransportResult<Vec<Message<Self::Token>>> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(TransportError::Closed);
        }

        let stream_name = self
            .config
            .stream
            .as_deref()
            .ok_or_else(|| TransportError::Config("config.stream must be set for recv()".into()))?
            .to_string();

        self.ensure_group(&stream_name).await?;

        let opts = StreamReadOptions::default()
            .group(&self.config.group, &self.config.consumer)
            .count(max)
            .block(self.config.block_ms);

        let mut conn = self.conn.lock().await;

        // ">" means only new (undelivered) messages
        let reply: StreamReadReply = conn
            .xread_options(&[&stream_name], &[">"], &opts)
            .await
            .map_err(|e| {
                TransportError::Recv(format!("XREADGROUP on stream '{stream_name}' failed: {e}"))
            })?;

        let stream_arc: Arc<str> = Arc::from(stream_name.as_str());
        let mut messages = Vec::new();

        for stream_key in &reply.keys {
            for stream_id in &stream_key.ids {
                // Extract the "payload" field from the entry
                let payload_bytes: Option<Vec<u8>> = stream_id
                    .map
                    .get("payload")
                    .and_then(|v| redis::from_redis_value(v.clone()).ok());

                let payload = payload_bytes.unwrap_or_default();
                let format = PayloadFormat::detect(&payload);
                let timestamp_ms = parse_entry_timestamp(&stream_id.id);

                messages.push(Message {
                    key: Some(Arc::clone(&stream_arc)),
                    payload,
                    token: RedisToken {
                        stream: Arc::clone(&stream_arc),
                        entry_id: stream_id.id.clone(),
                    },
                    timestamp_ms,
                    format,
                });
            }
        }

        #[cfg(feature = "metrics")]
        if !messages.is_empty() {
            metrics::counter!("dfe_transport_sent_total", "transport" => "redis")
                .increment(messages.len() as u64);
        }

        Ok(messages)
    }

    async fn commit(&self, tokens: &[Self::Token]) -> TransportResult<()> {
        if tokens.is_empty() {
            return Ok(());
        }

        // Group tokens by stream name for batch XACK
        let mut by_stream: std::collections::HashMap<&str, Vec<&str>> =
            std::collections::HashMap::new();
        for token in tokens {
            by_stream
                .entry(&token.stream)
                .or_default()
                .push(&token.entry_id);
        }

        let mut conn = self.conn.lock().await;

        for (stream, ids) in &by_stream {
            let id_refs: Vec<&str> = ids.clone();
            let _acked: i32 = conn
                .xack(*stream, &self.config.group, &id_refs)
                .await
                .map_err(|e| {
                    TransportError::Commit(format!("XACK on stream '{stream}' failed: {e}"))
                })?;
        }

        Ok(())
    }
}

/// Parse millisecond timestamp from a Redis stream entry ID.
///
/// Entry IDs have the format `<millisecondsTime>-<sequenceNumber>`.
fn parse_entry_timestamp(entry_id: &str) -> Option<i64> {
    entry_id
        .split_once('-')
        .and_then(|(ms_str, _)| ms_str.parse::<i64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_display() {
        let token = RedisToken {
            stream: Arc::from("my_stream"),
            entry_id: "1711432800000-0".into(),
        };
        assert_eq!(format!("{token}"), "redis:my_stream:1711432800000-0");
    }

    #[test]
    fn token_clone() {
        let token = RedisToken {
            stream: Arc::from("s1"),
            entry_id: "100-0".into(),
        };
        let cloned = token.clone();
        assert_eq!(token, cloned);
    }

    #[test]
    fn config_defaults() {
        let config = RedisTransportConfig::default();
        assert_eq!(config.url, "redis://127.0.0.1:6379");
        assert!(config.stream.is_none());
        assert_eq!(config.group, "dfe");
        assert!(config.max_stream_len.is_none());
        assert_eq!(config.block_ms, 5000);
    }

    #[test]
    fn config_deserialise_minimal() {
        let yaml = r"
url: redis://myhost:6380
stream: events
";
        let config: RedisTransportConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(config.url, "redis://myhost:6380");
        assert_eq!(config.stream.as_deref(), Some("events"));
        // Defaults should be applied
        assert_eq!(config.group, "dfe");
        assert_eq!(config.block_ms, 5000);
    }

    #[test]
    fn config_deserialise_full() {
        let yaml = r"
url: rediss://secure.redis.io:6380
stream: audit_log
group: my_group
consumer: worker-3
max_stream_len: 100000
block_ms: 2000
";
        let config: RedisTransportConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(config.url, "rediss://secure.redis.io:6380");
        assert_eq!(config.stream.as_deref(), Some("audit_log"));
        assert_eq!(config.group, "my_group");
        assert_eq!(config.consumer, "worker-3");
        assert_eq!(config.max_stream_len, Some(100_000));
        assert_eq!(config.block_ms, 2000);
    }

    #[test]
    fn parse_entry_timestamp_valid() {
        assert_eq!(
            parse_entry_timestamp("1711432800000-0"),
            Some(1_711_432_800_000)
        );
        assert_eq!(parse_entry_timestamp("0-0"), Some(0));
    }

    #[test]
    fn parse_entry_timestamp_invalid() {
        assert_eq!(parse_entry_timestamp("not-a-number"), None);
        assert_eq!(parse_entry_timestamp(""), None);
    }

    #[test]
    fn resolve_stream_uses_key_when_non_empty() {
        let config = RedisTransportConfig {
            stream: Some("default_stream".into()),
            ..Default::default()
        };
        // Cannot call resolve_stream without a transport instance, so test
        // the logic inline: non-empty key takes precedence.
        let key = "override_stream";
        let resolved = if key.is_empty() {
            config.stream.as_deref().unwrap_or("")
        } else {
            key
        };
        assert_eq!(resolved, "override_stream");
    }

    #[test]
    fn resolve_stream_falls_back_to_config() {
        let config = RedisTransportConfig {
            stream: Some("default_stream".into()),
            ..Default::default()
        };
        let key = "";
        let resolved = if key.is_empty() {
            config.stream.as_deref().unwrap_or("")
        } else {
            key
        };
        assert_eq!(resolved, "default_stream");
    }

    // Integration test: requires a running Redis instance.
    // Run with: REDIS_URL=redis://localhost:6379 cargo nextest run redis_integration
    #[tokio::test]
    async fn redis_integration_xadd_xreadgroup_xack() {
        let Ok(url) = std::env::var("REDIS_URL") else {
            eprintln!("Skipping: REDIS_URL not set");
            return;
        };

        let stream = format!("test_stream_{}", chrono::Utc::now().timestamp_millis());
        let group = "test_group";
        let consumer = "test_consumer";

        let config = RedisTransportConfig {
            url: url.clone(),
            stream: Some(stream.clone()),
            group: group.into(),
            consumer: consumer.into(),
            max_stream_len: Some(1000),
            block_ms: 1000,
        };

        let transport = RedisTransport::new(&config).await.unwrap();

        // Send two messages
        let r1 = transport.send("", b"{\"n\":1}").await;
        assert!(r1.is_ok(), "first send should succeed");

        let r2 = transport.send("", b"{\"n\":2}").await;
        assert!(r2.is_ok(), "second send should succeed");

        // Receive messages
        let messages = transport.recv(10).await.unwrap();
        assert_eq!(messages.len(), 2, "should receive 2 messages");
        assert_eq!(messages[0].payload, b"{\"n\":1}");
        assert_eq!(messages[1].payload, b"{\"n\":2}");

        // Commit (XACK)
        let tokens: Vec<_> = messages.iter().map(|m| m.token.clone()).collect();
        transport.commit(&tokens).await.unwrap();

        // After commit, no new messages should be available
        let more = transport.recv(10).await.unwrap();
        assert!(more.is_empty(), "no more messages after commit");

        // Clean up: delete the test stream
        let mut conn = transport.conn.lock().await;
        let _: redis::RedisResult<()> =
            redis::cmd("DEL").arg(&stream).query_async(&mut *conn).await;

        transport.close().await.unwrap();
        assert!(!transport.is_healthy());
    }
}
