// Project:   hyperi-rustlib
// File:      src/transport/zenoh/mod.rs
// Purpose:   Zenoh transport implementation
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # Zenoh Transport
//!
//! Low-latency pub/sub transport using Eclipse Zenoh.
//! Supports shared memory for same-node communication (~30µs latency).
//!
//! ## Features
//!
//! - Peer mode for brokerless mesh networking
//! - Client mode for router-based deployments
//! - Shared memory optimization for same-node
//! - Reliable in-flight delivery (not persisted)
//!
//! ## Reliability Note
//!
//! Zenoh provides reliable delivery for in-flight messages but has **no persistence**.
//! If a node crashes, in-flight messages are lost. Use Kafka for production workloads
//! requiring at-least-once delivery with replay.
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::transport::{ZenohTransport, ZenohConfig, Transport};
//!
//! // Peer mode (no routers needed)
//! let config = ZenohConfig::peer(vec!["events/**".to_string()]);
//! let transport = ZenohTransport::new(&config).await?;
//!
//! loop {
//!     let messages = transport.recv(100).await?;
//!     for msg in &messages {
//!         // Process message...
//!     }
//!     // Commit is a no-op for Zenoh (no persistence)
//!     transport.commit(&[]).await?;
//! }
//! ```

mod config;
mod token;

pub use config::{ZenohConfig, ZenohCongestionControl, ZenohMode, ZenohReliability};
pub use token::ZenohToken;

use super::error::{TransportError, TransportResult};
use super::traits::Transport;
use super::types::{Message, PayloadFormat, SendResult};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// Zenoh transport for low-latency pub/sub.
pub struct ZenohTransport {
    session: zenoh::Session,
    receiver: tokio::sync::Mutex<mpsc::Receiver<Message<ZenohToken>>>,
    key_cache: RwLock<HashMap<String, Arc<str>>>,
    closed: AtomicBool,
    recv_timeout_ms: u64,
    // Keep subscribers alive
    _subscribers: Vec<zenoh::pubsub::Subscriber<()>>,
}

impl ZenohTransport {
    /// Create a new Zenoh transport.
    ///
    /// # Errors
    ///
    /// Returns error if Zenoh session creation fails.
    pub async fn new(config: &ZenohConfig) -> TransportResult<Self> {
        // Build Zenoh config using JSON5 (Zenoh 1.x API)
        let json5_config = config.to_json5();
        let zenoh_config = zenoh::Config::from_json5(&json5_config)
            .map_err(|e| TransportError::Config(format!("Failed to parse Zenoh config: {e}")))?;

        // Open session
        let session = zenoh::open(zenoh_config)
            .await
            .map_err(|e| TransportError::Connection(format!("Failed to open session: {e}")))?;

        // Create channel for receiving messages
        let (sender, receiver) = mpsc::channel(config.recv_buffer_size);

        // Subscribe to key expressions
        let mut subscribers = Vec::new();
        let sequence = Arc::new(AtomicU64::new(0));

        for key_expr in &config.subscribe {
            let sender = sender.clone();
            let seq_ref = sequence.clone();
            let key_arc: Arc<str> = Arc::from(key_expr.as_str());

            let subscriber = session
                .declare_subscriber(key_expr)
                .callback(move |sample| {
                    let seq = seq_ref.fetch_add(1, Ordering::Relaxed);
                    let payload = sample.payload().to_bytes().to_vec();
                    let format = PayloadFormat::detect(&payload);
                    let timestamp = sample.timestamp().map(|t| t.get_time().as_u64());

                    let msg = Message {
                        key: Some(key_arc.clone()),
                        payload,
                        token: ZenohToken::new(key_arc.clone(), timestamp, seq),
                        timestamp_ms: timestamp
                            .map(|t| i64::try_from(t / 1_000_000).unwrap_or(i64::MAX)),
                        format,
                    };

                    // Non-blocking send (drop if full)
                    let _ = sender.try_send(msg);
                })
                .await
                .map_err(|e| TransportError::Connection(format!("Failed to subscribe: {e}")))?;

            subscribers.push(subscriber);
        }

        Ok(Self {
            session,
            receiver: tokio::sync::Mutex::new(receiver),
            key_cache: RwLock::new(HashMap::new()),
            closed: AtomicBool::new(false),
            recv_timeout_ms: config.recv_timeout_ms,
            _subscribers: subscribers,
        })
    }

    /// Get or create cached Arc<str> for key expression.
    #[allow(dead_code)]
    async fn get_key_arc(&self, key: &str) -> Arc<str> {
        // Fast path: read lock
        {
            let cache = self.key_cache.read().await;
            if let Some(arc) = cache.get(key) {
                return arc.clone();
            }
        }

        // Slow path: write lock
        let mut cache = self.key_cache.write().await;
        cache
            .entry(key.to_string())
            .or_insert_with(|| Arc::from(key))
            .clone()
    }
}

#[async_trait]
impl Transport for ZenohTransport {
    type Token = ZenohToken;

    async fn send(&self, key: &str, payload: &[u8]) -> SendResult {
        if self.closed.load(Ordering::Relaxed) {
            return SendResult::Fatal(TransportError::Closed);
        }

        match self.session.put(key, payload).await {
            Ok(()) => SendResult::Ok,
            Err(e) => SendResult::Fatal(TransportError::Send(e.to_string())),
        }
    }

    async fn recv(&self, max: usize) -> TransportResult<Vec<Message<Self::Token>>> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(TransportError::Closed);
        }

        let mut receiver = self.receiver.lock().await;
        let mut messages = Vec::with_capacity(max.min(100));

        for _ in 0..max {
            let result = if self.recv_timeout_ms == 0 {
                // Non-blocking
                match receiver.try_recv() {
                    Ok(msg) => Some(msg),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        return Err(TransportError::Closed);
                    }
                }
            } else if messages.is_empty() {
                // First message: wait with timeout
                match tokio::time::timeout(
                    std::time::Duration::from_millis(self.recv_timeout_ms),
                    receiver.recv(),
                )
                .await
                {
                    Ok(Some(msg)) => Some(msg),
                    Ok(None) => return Err(TransportError::Closed),
                    Err(_) => break, // Timeout
                }
            } else {
                // Subsequent: non-blocking
                match receiver.try_recv() {
                    Ok(msg) => Some(msg),
                    Err(_) => break,
                }
            };

            if let Some(msg) = result {
                messages.push(msg);
            }
        }

        Ok(messages)
    }

    async fn commit(&self, _tokens: &[Self::Token]) -> TransportResult<()> {
        // Zenoh has no persistence - commit is a no-op
        // The tokens could be used for application-level tracking if needed
        Ok(())
    }

    async fn close(&self) -> TransportResult<()> {
        self.closed.store(true, Ordering::Relaxed);
        self.session
            .close()
            .await
            .map_err(|e| TransportError::Internal(format!("Failed to close session: {e}")))?;
        Ok(())
    }

    fn is_healthy(&self) -> bool {
        !self.closed.load(Ordering::Relaxed)
    }

    fn name(&self) -> &'static str {
        "zenoh"
    }
}
