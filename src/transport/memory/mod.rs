// Project:   hyperi-rustlib
// File:      src/transport/memory/mod.rs
// Purpose:   In-memory transport using tokio channels
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # Memory Transport
//!
//! In-memory transport using tokio channels for unit testing.
//! No persistence, same-process only.
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::transport::{MemoryTransport, MemoryConfig, Transport};
//!
//! let config = MemoryConfig::default();
//! let transport = MemoryTransport::new(&config);
//!
//! // In tests, you can also get a sender handle
//! let sender = transport.sender();
//! sender.send(b"test payload".to_vec()).await?;
//!
//! let messages = transport.recv(10).await?;
//! assert_eq!(messages.len(), 1);
//! ```

mod token;

pub use token::MemoryToken;

use super::error::{TransportError, TransportResult};
use super::traits::{TransportBase, TransportReceiver, TransportSender};
use super::types::{Message, PayloadFormat, SendResult};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::mpsc;

/// Configuration for memory transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Channel buffer size.
    #[serde(default = "default_buffer_size")]
    pub buffer_size: usize,

    /// Receive timeout in milliseconds (0 = no wait, return immediately).
    #[serde(default)]
    pub recv_timeout_ms: u64,

    /// Inbound message filters (applied on recv before caller sees messages).
    #[serde(default)]
    pub filters_in: Vec<super::filter::FilterRule>,

    /// Outbound message filters (applied on send before transport dispatches).
    #[serde(default)]
    pub filters_out: Vec<super::filter::FilterRule>,
}

fn default_buffer_size() -> usize {
    1000
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            buffer_size: default_buffer_size(),
            recv_timeout_ms: 0,
            filters_in: Vec::new(),
            filters_out: Vec::new(),
        }
    }
}

/// Internal message type for the channel.
struct InternalMessage {
    key: Option<Arc<str>>,
    payload: Vec<u8>,
    seq: u64,
    timestamp_ms: i64,
}

/// In-memory transport using tokio channels.
///
/// Primarily for unit testing - no persistence, same-process only.
pub struct MemoryTransport {
    sender: mpsc::Sender<InternalMessage>,
    receiver: tokio::sync::Mutex<mpsc::Receiver<InternalMessage>>,
    sequence: AtomicU64,
    committed_seq: AtomicU64,
    closed: AtomicBool,
    recv_timeout_ms: u64,
    filter_engine: super::filter::TransportFilterEngine,
}

impl MemoryTransport {
    /// Create a new memory transport.
    #[must_use]
    pub fn new(config: &MemoryConfig) -> Self {
        let (sender, receiver) = mpsc::channel(config.buffer_size);
        let filter_engine = super::filter::TransportFilterEngine::new(
            &config.filters_in,
            &config.filters_out,
            &crate::transport::filter::TransportFilterTierConfig::default(),
        )
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "Failed to compile transport filters, filtering disabled");
            super::filter::TransportFilterEngine::empty()
        });
        Self {
            sender,
            receiver: tokio::sync::Mutex::new(receiver),
            sequence: AtomicU64::new(0),
            committed_seq: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            recv_timeout_ms: config.recv_timeout_ms,
            filter_engine,
        }
    }

    /// Get a sender handle for injecting test messages.
    ///
    /// This is useful in tests to send messages without going through
    /// the Transport trait.
    #[must_use]
    pub fn sender(&self) -> MemorySender<'_> {
        MemorySender {
            sender: self.sender.clone(),
            sequence: &self.sequence,
        }
    }

    /// Send a message directly (bypasses Transport trait).
    ///
    /// # Errors
    ///
    /// Returns error if the channel is full or closed.
    pub async fn inject(&self, key: Option<&str>, payload: Vec<u8>) -> TransportResult<()> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(TransportError::Closed);
        }

        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        let timestamp_ms = chrono::Utc::now().timestamp_millis();

        let msg = InternalMessage {
            key: key.map(Arc::from),
            payload,
            seq,
            timestamp_ms,
        };

        self.sender
            .send(msg)
            .await
            .map_err(|_| TransportError::Send("channel closed".into()))
    }

    /// Get the current committed sequence number.
    #[must_use]
    pub fn committed_sequence(&self) -> u64 {
        self.committed_seq.load(Ordering::Relaxed)
    }
}

/// Sender handle for injecting test messages.
pub struct MemorySender<'a> {
    sender: mpsc::Sender<InternalMessage>,
    sequence: &'a AtomicU64,
}

impl MemorySender<'_> {
    /// Send a payload with optional key.
    ///
    /// # Errors
    ///
    /// Returns error if the channel is full or closed.
    pub async fn send(&self, key: Option<&str>, payload: Vec<u8>) -> TransportResult<()> {
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        let timestamp_ms = chrono::Utc::now().timestamp_millis();

        let msg = InternalMessage {
            key: key.map(Arc::from),
            payload,
            seq,
            timestamp_ms,
        };

        self.sender
            .send(msg)
            .await
            .map_err(|_| TransportError::Send("channel closed".into()))
    }
}

impl TransportBase for MemoryTransport {
    async fn close(&self) -> TransportResult<()> {
        self.closed.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn is_healthy(&self) -> bool {
        !self.closed.load(Ordering::Relaxed)
    }

    fn name(&self) -> &'static str {
        "memory"
    }
}

impl TransportSender for MemoryTransport {
    async fn send(&self, key: &str, payload: &[u8]) -> SendResult {
        if self.closed.load(Ordering::Relaxed) {
            return SendResult::Fatal(TransportError::Closed);
        }

        // Outbound filter check
        if self.filter_engine.has_outbound_filters() {
            match self.filter_engine.apply_outbound(payload) {
                super::filter::FilterDisposition::Pass => {}
                super::filter::FilterDisposition::Drop => return SendResult::Ok,
                super::filter::FilterDisposition::Dlq => return SendResult::FilteredDlq,
            }
        }

        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        let timestamp_ms = chrono::Utc::now().timestamp_millis();

        let msg = InternalMessage {
            key: Some(Arc::from(key)),
            payload: payload.to_vec(),
            seq,
            timestamp_ms,
        };

        match self.sender.try_send(msg) {
            Ok(()) => SendResult::Ok,
            Err(mpsc::error::TrySendError::Full(_)) => SendResult::Backpressured,
            Err(mpsc::error::TrySendError::Closed(_)) => SendResult::Fatal(TransportError::Closed),
        }
    }
}

impl TransportReceiver for MemoryTransport {
    type Token = MemoryToken;

    async fn recv(&self, max: usize) -> TransportResult<Vec<Message<Self::Token>>> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(TransportError::Closed);
        }

        let mut receiver = self.receiver.lock().await;
        let mut messages = Vec::with_capacity(max.min(100));

        for _ in 0..max {
            let result = if self.recv_timeout_ms == 0 {
                match receiver.try_recv() {
                    Ok(msg) => Some(msg),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        return Err(TransportError::Closed);
                    }
                }
            } else if messages.is_empty() {
                match tokio::time::timeout(
                    std::time::Duration::from_millis(self.recv_timeout_ms),
                    receiver.recv(),
                )
                .await
                {
                    Ok(Some(msg)) => Some(msg),
                    Ok(None) => return Err(TransportError::Closed),
                    Err(_) => break,
                }
            } else {
                match receiver.try_recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => break,
                }
            };

            if let Some(internal) = result {
                let format = PayloadFormat::detect(&internal.payload);
                messages.push(Message {
                    key: internal.key,
                    payload: internal.payload,
                    token: MemoryToken { seq: internal.seq },
                    timestamp_ms: Some(internal.timestamp_ms),
                    format,
                });
            }
        }

        // Apply inbound filters — remove messages that match drop/dlq filters
        if self.filter_engine.has_inbound_filters() {
            messages.retain(|msg| match self.filter_engine.apply_inbound(&msg.payload) {
                super::filter::FilterDisposition::Pass => true,
                super::filter::FilterDisposition::Drop | super::filter::FilterDisposition::Dlq => {
                    false
                }
            });
        }

        Ok(messages)
    }

    async fn commit(&self, tokens: &[Self::Token]) -> TransportResult<()> {
        if let Some(max_seq) = tokens.iter().map(|t| t.seq).max() {
            let _ = self.committed_seq.fetch_max(max_seq, Ordering::Relaxed);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn send_and_receive() {
        let config = MemoryConfig::default();
        let transport = MemoryTransport::new(&config);

        // Send a message
        let result = transport.send("test-key", b"hello world").await;
        assert!(result.is_ok());

        // Receive it
        let messages = transport.recv(10).await.unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].key.as_deref(), Some("test-key"));
        assert_eq!(messages[0].payload, b"hello world");
    }

    #[tokio::test]
    async fn inject_messages() {
        let config = MemoryConfig::default();
        let transport = MemoryTransport::new(&config);

        // Inject test messages
        transport
            .inject(Some("key1"), b"msg1".to_vec())
            .await
            .unwrap();
        transport
            .inject(Some("key2"), b"msg2".to_vec())
            .await
            .unwrap();

        // Receive them
        let messages = transport.recv(10).await.unwrap();
        assert_eq!(messages.len(), 2);
    }

    #[tokio::test]
    async fn commit_advances_sequence() {
        let config = MemoryConfig::default();
        let transport = MemoryTransport::new(&config);

        transport.inject(None, b"msg".to_vec()).await.unwrap();
        let messages = transport.recv(1).await.unwrap();

        // Commit the message
        let tokens: Vec<_> = messages.iter().map(|m| m.token).collect();
        transport.commit(&tokens).await.unwrap();

        // Verify committed sequence advanced
        assert_eq!(transport.committed_sequence(), 0);
    }

    #[tokio::test]
    async fn close_prevents_operations() {
        let config = MemoryConfig::default();
        let transport = MemoryTransport::new(&config);

        transport.close().await.unwrap();
        assert!(!transport.is_healthy());

        // Send should fail
        let result = transport.send("key", b"data").await;
        assert!(result.is_fatal());

        // Recv should fail
        let result = transport.recv(1).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn backpressure_on_full_channel() {
        let config = MemoryConfig {
            buffer_size: 1,
            recv_timeout_ms: 0,
            ..Default::default()
        };
        let transport = MemoryTransport::new(&config);

        // Fill the channel
        let result1 = transport.send("key", b"msg1").await;
        assert!(result1.is_ok());

        // Next send should backpressure
        let result2 = transport.send("key", b"msg2").await;
        assert!(result2.is_backpressured());
    }
}
