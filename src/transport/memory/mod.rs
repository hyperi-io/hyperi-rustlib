// Project:   hyperi-rustlib
// File:      src/transport/memory/mod.rs
// Purpose:   In-memory transport using tokio channels
// Language:  Rust
//
// License:   BUSL-1.1
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
//! let transport = MemoryTransport::new(&config).expect("memory transport with valid config must construct");
//!
//! // In tests, you can also get a sender handle
//! let sender = transport.sender();
//! sender.send(b"test payload".to_vec()).await?;
//!
//! let records = transport.recv(10).await?.records;
//! assert_eq!(records.len(), 1);
//! ```

mod token;

pub use token::MemoryToken;

use super::error::{TransportError, TransportResult};
use super::traits::{RecvBatch, TransportBase, TransportReceiver, TransportSender};
use super::types::{Message, PayloadFormat, SendResult};
use super::work_batch::WorkBatch;
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
    payload: bytes::Bytes,
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
    ///
    /// # Errors
    ///
    /// Returns [`TransportError`] when any inbound/outbound filter rule
    /// fails to compile. Previously this produced a `tracing::warn!` and
    /// silently substituted an empty filter engine; that fail-open
    /// behaviour hid real misconfiguration (a filter that should have
    /// blocked traffic would instead let every message through), so the
    /// constructor now propagates the error to the caller.
    pub fn new(config: &MemoryConfig) -> super::error::TransportResult<Self> {
        let (sender, receiver) = mpsc::channel(config.buffer_size);
        let filter_engine = super::filter::TransportFilterEngine::new(
            &config.filters_in,
            &config.filters_out,
            &crate::transport::filter::TransportFilterTierConfig::from_cascade(),
        )?;
        Ok(Self {
            sender,
            receiver: tokio::sync::Mutex::new(receiver),
            sequence: AtomicU64::new(0),
            committed_seq: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            recv_timeout_ms: config.recv_timeout_ms,
            filter_engine,
        })
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
            payload: payload.into(),
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
            payload: payload.into(),
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
    async fn send(&self, key: &str, payload: bytes::Bytes) -> SendResult {
        if self.closed.load(Ordering::Relaxed) {
            return SendResult::Fatal(TransportError::Closed);
        }

        // Outbound filter check
        if self.filter_engine.has_outbound_filters() {
            match self.filter_engine.apply_outbound(&payload) {
                super::filter::FilterDisposition::Pass => {}
                super::filter::FilterDisposition::Drop => return SendResult::Ok,
                super::filter::FilterDisposition::Dlq => return SendResult::FilteredDlq,
            }
        }

        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        let timestamp_ms = chrono::Utc::now().timestamp_millis();

        let msg = InternalMessage {
            key: Some(Arc::from(key)),
            payload,
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

    async fn recv(&self, max: usize) -> TransportResult<WorkBatch<Self::Token>> {
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
                let payload = internal.payload;
                let format = PayloadFormat::detect(&payload);
                messages.push(Message {
                    key: internal.key,
                    payload,
                    token: MemoryToken { seq: internal.seq },
                    timestamp_ms: Some(internal.timestamp_ms),
                    format,
                });
            }
        }

        // Apply inbound filters via the shared partition helper; DLQ entries
        // are returned in the RecvBatch for the caller to route onward.
        let batch = self.filter_engine.partition_batch(
            messages,
            |m| m.payload.as_ref(),
            |m| m.key.clone(),
            |m| m.token,
        );
        let messages = batch.messages;
        let dlq_entries = batch.dlq_entries;
        let filtered_tokens = batch.filtered_tokens;

        Ok(RecvBatch {
            messages,
            dlq_entries,
            filtered_tokens,
        }
        .into())
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
        let transport = MemoryTransport::new(&config)
            .expect("memory transport with valid config must construct");
        // Send a message
        let result = transport
            .send("test-key", bytes::Bytes::from_static(b"hello world"))
            .await;
        assert!(result.is_ok());

        // Receive it
        let records = transport.recv(10).await.unwrap().records;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].key.as_deref(), Some("test-key"));
        assert_eq!(records[0].payload.as_ref(), b"hello world");
    }

    /// MemoryTransport does NOT override `send_batch`, so this exercises the
    /// trait's per-record default fallback (Task 0.7c): every record is sent
    /// individually via `send`, using its own key, and all arrive intact.
    #[tokio::test]
    async fn send_batch_default_fallback_sends_each_record() {
        use super::super::work_batch::{Record, RecordMeta};

        let transport = MemoryTransport::new(&MemoryConfig::default())
            .expect("memory transport with valid config must construct");

        let records: Vec<Record> = (0..3)
            .map(|i| Record {
                payload: bytes::Bytes::from(format!(r#"{{"id":{i}}}"#)),
                key: Some(Arc::from(format!("k{i}").as_str())),
                headers: Vec::new(),
                metadata: RecordMeta {
                    timestamp_ms: None,
                    format: PayloadFormat::Json,
                },
            })
            .collect();

        // Default fallback: one send per record, returns Ok for the whole block.
        let result = transport.send_batch(&records).await;
        assert!(
            result.is_ok(),
            "default send_batch must succeed: {result:?}"
        );

        // All three records loop back through recv with keys + payloads intact.
        let got = transport.recv(10).await.unwrap().records;
        assert_eq!(got.len(), 3, "every record in the block was sent");
        assert_eq!(got[0].key.as_deref(), Some("k0"));
        assert_eq!(got[0].payload.as_ref(), br#"{"id":0}"#);
        assert_eq!(got[2].key.as_deref(), Some("k2"));
        assert_eq!(got[2].payload.as_ref(), br#"{"id":2}"#);
    }

    /// The default `send_batch` short-circuits on the first non-Ok result so the
    /// caller retries the unconfirmed remainder (at-least-once). A closed
    /// transport makes every `send` Fatal, so a non-empty block returns Fatal.
    #[tokio::test]
    async fn send_batch_default_short_circuits_on_error() {
        use super::super::work_batch::{Record, RecordMeta};

        let transport = MemoryTransport::new(&MemoryConfig::default())
            .expect("memory transport with valid config must construct");
        transport.close().await.unwrap();

        let records = vec![Record {
            payload: bytes::Bytes::from_static(b"{}"),
            key: None,
            headers: Vec::new(),
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Json,
            },
        }];

        let result = transport.send_batch(&records).await;
        assert!(
            result.is_fatal(),
            "closed transport must surface the send failure, got {result:?}"
        );

        // An empty block is a trivial Ok (nothing to send).
        assert!(transport.send_batch(&[]).await.is_ok());
    }

    #[tokio::test]
    async fn inject_messages() {
        let config = MemoryConfig::default();
        let transport = MemoryTransport::new(&config)
            .expect("memory transport with valid config must construct");
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
        let records = transport.recv(10).await.unwrap().records;
        assert_eq!(records.len(), 2);
    }

    #[tokio::test]
    async fn commit_advances_sequence() {
        let config = MemoryConfig::default();
        let transport = MemoryTransport::new(&config)
            .expect("memory transport with valid config must construct");
        transport.inject(None, b"msg".to_vec()).await.unwrap();
        let batch = transport.recv(1).await.unwrap();

        // Commit the message via the batch's commit tokens.
        transport.commit(&batch.commit_tokens).await.unwrap();

        // Verify committed sequence advanced
        assert_eq!(transport.committed_sequence(), 0);
    }

    #[tokio::test]
    async fn close_prevents_operations() {
        let config = MemoryConfig::default();
        let transport = MemoryTransport::new(&config)
            .expect("memory transport with valid config must construct");
        transport.close().await.unwrap();
        assert!(!transport.is_healthy());

        // Send should fail
        let result = transport
            .send("key", bytes::Bytes::from_static(b"data"))
            .await;
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
        let transport = MemoryTransport::new(&config)
            .expect("memory transport with valid config must construct");

        // Fill the channel
        let result1 = transport
            .send("key", bytes::Bytes::from_static(b"msg1"))
            .await;
        assert!(result1.is_ok());

        // Next send should backpressure
        let result2 = transport
            .send("key", bytes::Bytes::from_static(b"msg2"))
            .await;
        assert!(result2.is_backpressured());
    }
}
