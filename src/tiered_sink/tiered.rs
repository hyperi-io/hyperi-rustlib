// Project:   hyperi-rustlib
// File:      src/tiered_sink/tiered.rs
// Purpose:   TieredSink implementation
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! TieredSink implementation.

use crate::tiered_sink::{
    CircuitBreaker, CircuitState, CompressionCodec, OrderingMode, Result, Sink, SinkError,
    TieredSinkConfig, TieredSinkError, drainer,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use yaque::{Receiver, Sender};

/// A tiered sink with automatic disk spillover.
///
/// Wraps any `Sink` implementation and automatically spills messages to disk
/// when the primary sink is unavailable or backpressuring. A background task
/// drains spooled messages back to the primary when it recovers.
pub struct TieredSink<S: Sink> {
    sink: Arc<S>,
    spool_sender: Arc<Mutex<Sender>>,
    /// Receiver is owned by TieredSink but accessed via Arc clone by drainer task
    #[allow(dead_code)]
    spool_receiver: Arc<Mutex<Receiver>>,
    spool_count: Arc<AtomicU64>,
    circuit: Arc<CircuitBreaker>,
    codec: CompressionCodec,
    config: TieredSinkConfig,
    shutdown: Arc<Notify>,
    drain_handle: Option<JoinHandle<()>>,

    // Metrics
    hot_path_count: AtomicU64,
    cold_path_count: AtomicU64,
}

impl<S: Sink> TieredSink<S> {
    /// Create a new TieredSink wrapping the given sink.
    ///
    /// # Errors
    ///
    /// Returns an error if the spool file cannot be opened.
    pub async fn new(sink: S, config: TieredSinkConfig) -> Result<Self> {
        let (sender, receiver) =
            yaque::channel(&config.spool_path).map_err(|e| TieredSinkError::SpoolOpen {
                path: config.spool_path.display().to_string(),
                message: e.to_string(),
            })?;

        let sink = Arc::new(sink);
        let spool_sender = Arc::new(Mutex::new(sender));
        let spool_receiver = Arc::new(Mutex::new(receiver));
        let spool_count = Arc::new(AtomicU64::new(0));
        let circuit = Arc::new(CircuitBreaker::new(
            config.circuit_failure_threshold,
            config.circuit_reset_timeout(),
        ));
        let shutdown = Arc::new(Notify::new());
        let codec = config.compression;

        // Start drain task
        let drain_handle = tokio::spawn(drainer::drain_loop(
            Arc::clone(&sink),
            Arc::clone(&spool_receiver),
            Arc::clone(&spool_count),
            Arc::clone(&circuit),
            codec,
            config.drain_strategy,
            config.drain_interval(),
            Arc::clone(&shutdown),
        ));

        Ok(Self {
            sink,
            spool_sender,
            spool_receiver,
            spool_count,
            circuit,
            codec,
            config,
            shutdown,
            drain_handle: Some(drain_handle),
            hot_path_count: AtomicU64::new(0),
            cold_path_count: AtomicU64::new(0),
        })
    }

    /// Send a message through the tiered sink.
    ///
    /// The message goes through the hot path (direct to sink) if:
    /// - Circuit is closed AND ordering mode is Interleaved
    /// - OR circuit is closed AND ordering is StrictFifo AND spool is empty
    ///
    /// Otherwise, the message is spooled to disk.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The sink returns a fatal error
    /// - The spool is full
    /// - Compression fails
    pub async fn send(&self, data: &[u8]) -> Result<()> {
        // Check if we should use hot path
        let use_hot_path = self.should_use_hot_path().await;

        if use_hot_path {
            match self.try_hot_path(data).await {
                Ok(()) => {
                    self.hot_path_count.fetch_add(1, AtomicOrdering::Relaxed);
                    return Ok(());
                }
                Err(TieredSinkError::Sink(_)) => {
                    // Fatal error, don't spool
                    return Err(TieredSinkError::Sink("fatal sink error".into()));
                }
                Err(_) => {
                    // Retryable error, fall through to spool
                }
            }
        }

        // Cold path: spool to disk
        self.spool_message(data).await?;
        self.cold_path_count.fetch_add(1, AtomicOrdering::Relaxed);
        Ok(())
    }

    /// Determine if we should attempt the hot path.
    async fn should_use_hot_path(&self) -> bool {
        let circuit_state = self.circuit.state().await;

        match circuit_state {
            CircuitState::Open => false,
            CircuitState::Closed | CircuitState::HalfOpen => match self.config.ordering {
                OrderingMode::Interleaved => true,
                OrderingMode::StrictFifo => {
                    // Only use hot path if spool is empty
                    self.spool_count.load(AtomicOrdering::Relaxed) == 0
                }
            },
        }
    }

    /// Try to send via hot path (direct to sink).
    async fn try_hot_path(&self, data: &[u8]) -> Result<()> {
        let send_timeout = self.config.send_timeout_duration();

        match timeout(send_timeout, self.sink.try_send(data)).await {
            Ok(Ok(())) => {
                self.circuit.record_success().await;
                Ok(())
            }
            Ok(Err(SinkError::Full)) => {
                // Backpressure, don't count as failure for circuit
                Err(TieredSinkError::Spool("sink full".into()))
            }
            Ok(Err(SinkError::Unavailable)) => {
                self.circuit.record_failure().await;
                Err(TieredSinkError::Spool("sink unavailable".into()))
            }
            Ok(Err(SinkError::Fatal(e))) => {
                // Fatal error - propagate, don't spool
                Err(TieredSinkError::Sink(e.to_string()))
            }
            Err(_timeout) => {
                self.circuit.record_failure().await;
                Err(TieredSinkError::Spool("send timeout".into()))
            }
        }
    }

    /// Spool a message to disk.
    async fn spool_message(&self, data: &[u8]) -> Result<()> {
        let compressed = self.codec.compress(data)?;

        // Check limits
        if let Some(max_items) = self.config.max_spool_items {
            #[allow(clippy::cast_possible_truncation)]
            let current = self.spool_count.load(AtomicOrdering::Relaxed) as usize;
            if current >= max_items {
                return Err(TieredSinkError::SpoolFull(format!(
                    "max items {max_items} reached"
                )));
            }
        }

        let mut sender = self.spool_sender.lock().await;
        sender
            .send(compressed)
            .await
            .map_err(|e| TieredSinkError::Spool(e.to_string()))?;

        self.spool_count.fetch_add(1, AtomicOrdering::Relaxed);

        #[cfg(feature = "logger")]
        tracing::debug!(
            spool_size = self.spool_count.load(AtomicOrdering::Relaxed),
            "Message spooled to disk"
        );

        Ok(())
    }

    /// Get the number of messages currently in the spool.
    #[allow(clippy::cast_possible_truncation)]
    pub async fn spool_len(&self) -> usize {
        self.spool_count.load(AtomicOrdering::Relaxed) as usize
    }

    /// Check if the spool is empty.
    pub async fn spool_is_empty(&self) -> bool {
        self.spool_count.load(AtomicOrdering::Relaxed) == 0
    }

    /// Get the current circuit breaker state.
    pub async fn circuit_state(&self) -> CircuitState {
        self.circuit.state().await
    }

    /// Get hot path message count.
    #[must_use]
    pub fn hot_path_count(&self) -> u64 {
        self.hot_path_count.load(AtomicOrdering::Relaxed)
    }

    /// Get cold path (spooled) message count.
    #[must_use]
    pub fn cold_path_count(&self) -> u64 {
        self.cold_path_count.load(AtomicOrdering::Relaxed)
    }

    /// Get a reference to the underlying sink.
    pub fn inner(&self) -> &S {
        &self.sink
    }

    /// Manually reset the circuit breaker.
    pub async fn reset_circuit(&self) {
        self.circuit.reset().await;
    }

    /// Shutdown the drain task gracefully.
    pub async fn shutdown(mut self) {
        self.shutdown.notify_one();
        if let Some(handle) = self.drain_handle.take() {
            let _ = handle.await;
        }
    }
}

impl<S: Sink> Drop for TieredSink<S> {
    fn drop(&mut self) {
        self.shutdown.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use tempfile::tempdir;

    #[derive(Debug)]
    struct TestError(String);

    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    impl std::error::Error for TestError {}

    struct TestSink {
        available: AtomicBool,
        received: Mutex<Vec<Vec<u8>>>,
    }

    impl TestSink {
        fn new() -> Self {
            Self {
                available: AtomicBool::new(true),
                received: Mutex::new(Vec::new()),
            }
        }

        fn set_available(&self, available: bool) {
            self.available.store(available, AtomicOrdering::SeqCst);
        }

        async fn received_count(&self) -> usize {
            self.received.lock().await.len()
        }
    }

    impl Sink for TestSink {
        type Error = TestError;

        async fn try_send(&self, data: &[u8]) -> std::result::Result<(), SinkError<Self::Error>> {
            if self.available.load(AtomicOrdering::SeqCst) {
                self.received.lock().await.push(data.to_vec());
                Ok(())
            } else {
                Err(SinkError::Unavailable)
            }
        }
    }

    #[tokio::test]
    async fn test_hot_path_when_available() {
        let dir = tempdir().unwrap();
        let spool_path = dir.path().join("test-queue");

        let sink = TestSink::new();
        let config = TieredSinkConfig::new(&spool_path);

        let tiered = TieredSink::new(sink, config).await.unwrap();

        tiered.send(b"hello").await.unwrap();

        assert_eq!(tiered.hot_path_count(), 1);
        assert_eq!(tiered.cold_path_count(), 0);
        assert!(tiered.spool_is_empty().await);
        assert_eq!(tiered.inner().received_count().await, 1);

        tiered.shutdown().await;
    }

    #[tokio::test]
    async fn test_cold_path_when_unavailable() {
        let dir = tempdir().unwrap();
        let spool_path = dir.path().join("test-queue");

        let sink = TestSink::new();
        sink.set_available(false);

        let mut config = TieredSinkConfig::new(&spool_path);
        config.circuit_failure_threshold = 1; // Open circuit quickly

        let tiered = TieredSink::new(sink, config).await.unwrap();

        // First message triggers circuit open
        tiered.send(b"hello").await.unwrap();

        // Should have spooled
        assert_eq!(tiered.cold_path_count(), 1);
        assert!(!tiered.spool_is_empty().await);
        assert_eq!(tiered.inner().received_count().await, 0);

        tiered.shutdown().await;
    }

    #[tokio::test]
    async fn test_circuit_opens_after_failures() {
        let dir = tempdir().unwrap();
        let spool_path = dir.path().join("test-queue");

        let sink = TestSink::new();
        sink.set_available(false);

        let mut config = TieredSinkConfig::new(&spool_path);
        config.circuit_failure_threshold = 3;

        let tiered = TieredSink::new(sink, config).await.unwrap();

        // First two fail but circuit stays closed
        tiered.send(b"1").await.unwrap();
        tiered.send(b"2").await.unwrap();
        assert_eq!(tiered.circuit_state().await, CircuitState::Closed);

        // Third failure opens circuit
        tiered.send(b"3").await.unwrap();
        assert_eq!(tiered.circuit_state().await, CircuitState::Open);

        tiered.shutdown().await;
    }

    #[tokio::test]
    async fn test_drain_recovers_messages() {
        let dir = tempdir().unwrap();
        let spool_path = dir.path().join("test-queue");

        let sink = TestSink::new();
        sink.set_available(false);

        let mut config = TieredSinkConfig::new(&spool_path);
        config.circuit_failure_threshold = 1;
        config.circuit_reset_timeout_ms = 50;
        config.drain_interval_ms = 10;

        let tiered = TieredSink::new(sink, config).await.unwrap();

        // Spool a message
        tiered.send(b"recover me").await.unwrap();
        assert_eq!(tiered.spool_len().await, 1);

        // Make sink available and wait for drain
        tiered.inner().set_available(true);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Should have drained
        assert!(tiered.spool_is_empty().await);
        assert_eq!(tiered.inner().received_count().await, 1);

        tiered.shutdown().await;
    }

    #[tokio::test]
    async fn test_strict_fifo_waits_for_drain() {
        let dir = tempdir().unwrap();
        let spool_path = dir.path().join("test-queue");

        let sink = TestSink::new();
        sink.set_available(false);

        let mut config = TieredSinkConfig::new(&spool_path);
        config.ordering = OrderingMode::StrictFifo;
        config.circuit_failure_threshold = 1;

        let tiered = TieredSink::new(sink, config).await.unwrap();

        // First message gets spooled due to unavailable sink
        tiered.send(b"first message").await.unwrap();
        assert_eq!(tiered.spool_len().await, 1);

        // Make sink available again
        tiered.inner().set_available(true);
        tiered.reset_circuit().await;

        // In StrictFifo mode, new messages should spool while spool is non-empty
        tiered.send(b"new message").await.unwrap();

        // Should still be 2 messages in spool (strict FIFO queues new messages behind old)
        assert_eq!(tiered.spool_len().await, 2);
        assert_eq!(tiered.cold_path_count(), 2);

        tiered.shutdown().await;
    }
}
