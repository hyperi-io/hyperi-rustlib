// Project:   hyperi-rustlib
// File:      src/transport/pipe.rs
// Purpose:   Unix pipe transport (stdin/stdout)
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # Pipe Transport
//!
//! Reads from stdin and writes to stdout for Unix pipeline composition.
//! Newline-delimited: each line is one message.
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::transport::{PipeTransport, PipeTransportConfig};
//!
//! let config = PipeTransportConfig::default();
//! let transport = PipeTransport::new(&config);
//!
//! // Send writes payload + newline to stdout
//! transport.send("ignored", b"hello world").await;
//!
//! // Recv reads lines from stdin
//! let messages = transport.recv(10).await?;
//! ```

use super::error::{TransportError, TransportResult};
use super::traits::{CommitToken, TransportBase, TransportReceiver, TransportSender};
use super::types::{Message, PayloadFormat, SendResult};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Commit token for pipe transport.
///
/// Contains a monotonic sequence number. Commit is a no-op
/// because stdin is a forward-only stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PipeToken {
    /// Message sequence number.
    pub seq: u64,
}

impl CommitToken for PipeToken {}

impl std::fmt::Display for PipeToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pipe:{}", self.seq)
    }
}

/// Configuration for pipe transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipeTransportConfig {
    /// Receive timeout in milliseconds (0 = block until data). Default: 100.
    #[serde(default = "default_recv_timeout_ms")]
    pub recv_timeout_ms: u64,
}

fn default_recv_timeout_ms() -> u64 {
    100
}

impl Default for PipeTransportConfig {
    fn default() -> Self {
        Self {
            recv_timeout_ms: default_recv_timeout_ms(),
        }
    }
}

impl PipeTransportConfig {
    /// Load from the config cascade under the `transport.pipe` key.
    #[must_use]
    pub fn from_cascade() -> Self {
        #[cfg(feature = "config")]
        {
            if let Some(cfg) = crate::config::try_get()
                && let Ok(tc) = cfg.unmarshal_key_registered::<Self>("transport.pipe")
            {
                return tc;
            }
        }
        Self::default()
    }
}

/// Unix pipe transport (stdin/stdout).
///
/// Send writes newline-delimited payloads to stdout.
/// Receive reads lines from stdin, each becoming a message.
/// Commit is a no-op (stdin cannot be rewound).
pub struct PipeTransport {
    stdin: tokio::sync::Mutex<BufReader<tokio::io::Stdin>>,
    stdout: tokio::sync::Mutex<tokio::io::Stdout>,
    sequence: AtomicU64,
    closed: Arc<AtomicBool>,
    recv_timeout_ms: u64,
}

impl PipeTransport {
    /// Create a new pipe transport.
    #[must_use]
    pub fn new(config: &PipeTransportConfig) -> Self {
        #[cfg(feature = "logger")]
        tracing::info!(
            recv_timeout_ms = config.recv_timeout_ms,
            "Pipe transport opened"
        );

        let closed = Arc::new(AtomicBool::new(false));

        #[cfg(feature = "health")]
        {
            let h = Arc::clone(&closed);
            crate::health::HealthRegistry::register("transport:pipe", move || {
                if h.load(Ordering::Relaxed) {
                    crate::health::HealthStatus::Unhealthy
                } else {
                    crate::health::HealthStatus::Healthy
                }
            });
        }

        Self {
            stdin: tokio::sync::Mutex::new(BufReader::new(tokio::io::stdin())),
            stdout: tokio::sync::Mutex::new(tokio::io::stdout()),
            sequence: AtomicU64::new(0),
            closed,
            recv_timeout_ms: config.recv_timeout_ms,
        }
    }
}

impl TransportBase for PipeTransport {
    async fn close(&self) -> TransportResult<()> {
        self.closed.store(true, Ordering::Relaxed);

        // Flush stdout before closing
        let mut stdout = self.stdout.lock().await;
        stdout
            .flush()
            .await
            .map_err(|e| TransportError::Internal(format!("stdout flush failed: {e}")))?;

        Ok(())
    }

    fn is_healthy(&self) -> bool {
        !self.closed.load(Ordering::Relaxed)
    }

    fn name(&self) -> &'static str {
        "pipe"
    }
}

impl TransportSender for PipeTransport {
    async fn send(&self, _key: &str, payload: &[u8]) -> SendResult {
        if self.closed.load(Ordering::Relaxed) {
            return SendResult::Fatal(TransportError::Closed);
        }

        let mut stdout = self.stdout.lock().await;

        // Write payload + newline
        if let Err(e) = stdout.write_all(payload).await {
            return SendResult::Fatal(TransportError::Send(format!("stdout write failed: {e}")));
        }
        if let Err(e) = stdout.write_all(b"\n").await {
            return SendResult::Fatal(TransportError::Send(format!(
                "stdout newline write failed: {e}"
            )));
        }
        if let Err(e) = stdout.flush().await {
            return SendResult::Fatal(TransportError::Send(format!("stdout flush failed: {e}")));
        }

        #[cfg(feature = "logger")]
        tracing::debug!(
            bytes = payload.len(),
            "Pipe transport: message sent to stdout"
        );

        #[cfg(feature = "metrics")]
        metrics::counter!("dfe_transport_sent_total", "transport" => "pipe").increment(1);

        SendResult::Ok
    }
}

impl TransportReceiver for PipeTransport {
    type Token = PipeToken;

    async fn recv(&self, max: usize) -> TransportResult<Vec<Message<Self::Token>>> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(TransportError::Closed);
        }

        let mut stdin = self.stdin.lock().await;
        let mut messages = Vec::with_capacity(max.min(100));
        let mut line_buf = String::new();

        for _ in 0..max {
            line_buf.clear();

            let read_result = if self.recv_timeout_ms == 0 {
                // Block until data arrives
                stdin.read_line(&mut line_buf).await
            } else if messages.is_empty() {
                // First message: wait up to timeout
                match tokio::time::timeout(
                    std::time::Duration::from_millis(self.recv_timeout_ms),
                    stdin.read_line(&mut line_buf),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => break, // Timeout, return what we have (empty)
                }
            } else {
                // Subsequent messages: non-blocking attempt via short timeout
                match tokio::time::timeout(
                    std::time::Duration::from_millis(1),
                    stdin.read_line(&mut line_buf),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => break, // No more data ready
                }
            };

            match read_result {
                Ok(0) => {
                    // EOF on stdin
                    if messages.is_empty() {
                        return Err(TransportError::Closed);
                    }
                    break;
                }
                Ok(_) => {
                    // Strip trailing newline
                    let payload = line_buf.trim_end_matches('\n').trim_end_matches('\r');
                    if payload.is_empty() {
                        continue;
                    }

                    let payload_bytes = payload.as_bytes().to_vec();
                    let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
                    let format = PayloadFormat::detect(&payload_bytes);
                    let timestamp_ms = chrono::Utc::now().timestamp_millis();

                    messages.push(Message {
                        key: None,
                        payload: payload_bytes,
                        token: PipeToken { seq },
                        timestamp_ms: Some(timestamp_ms),
                        format,
                    });

                    #[cfg(feature = "metrics")]
                    metrics::counter!("dfe_transport_received_total", "transport" => "pipe")
                        .increment(1);
                }
                Err(e) => {
                    return Err(TransportError::Recv(format!("stdin read failed: {e}")));
                }
            }
        }

        #[cfg(feature = "logger")]
        if !messages.is_empty() {
            tracing::debug!(
                lines = messages.len(),
                "Pipe transport: batch received from stdin"
            );
        }

        Ok(messages)
    }

    async fn commit(&self, _tokens: &[Self::Token]) -> TransportResult<()> {
        // No-op: stdin is a forward-only stream, cannot rewind or acknowledge
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_display() {
        let token = PipeToken { seq: 42 };
        assert_eq!(token.to_string(), "pipe:42");
    }

    #[test]
    fn token_as_str() {
        let token = PipeToken { seq: 7 };
        assert_eq!(token.as_str(), "pipe:7");
    }

    #[test]
    fn token_clone() {
        let token = PipeToken { seq: 99 };
        let cloned = token;
        assert_eq!(token, cloned);
    }

    #[test]
    fn config_defaults() {
        let config = PipeTransportConfig::default();
        assert_eq!(config.recv_timeout_ms, 100);
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = PipeTransportConfig {
            recv_timeout_ms: 500,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: PipeTransportConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.recv_timeout_ms, 500);
    }

    #[test]
    fn config_serde_default_fields() {
        // Empty JSON should use defaults
        let parsed: PipeTransportConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed.recv_timeout_ms, 100);
    }

    #[tokio::test]
    async fn new_transport_is_healthy() {
        let config = PipeTransportConfig::default();
        let transport = PipeTransport::new(&config);
        assert!(transport.is_healthy());
        assert_eq!(transport.name(), "pipe");
    }

    #[tokio::test]
    async fn close_marks_unhealthy() {
        let config = PipeTransportConfig::default();
        let transport = PipeTransport::new(&config);

        transport.close().await.unwrap();
        assert!(!transport.is_healthy());
    }

    #[tokio::test]
    async fn send_after_close_returns_fatal() {
        let config = PipeTransportConfig::default();
        let transport = PipeTransport::new(&config);

        transport.close().await.unwrap();
        let result = transport.send("key", b"data").await;
        assert!(result.is_fatal());
    }

    #[tokio::test]
    async fn recv_after_close_returns_error() {
        let config = PipeTransportConfig::default();
        let transport = PipeTransport::new(&config);

        transport.close().await.unwrap();
        let result = transport.recv(1).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn commit_is_noop() {
        let config = PipeTransportConfig::default();
        let transport = PipeTransport::new(&config);

        let tokens = vec![PipeToken { seq: 0 }, PipeToken { seq: 1 }];
        let result = transport.commit(&tokens).await;
        assert!(result.is_ok());
    }
}
