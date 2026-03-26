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
    spool_bytes: Arc<AtomicU64>,
    circuit: Arc<CircuitBreaker>,
    codec: CompressionCodec,
    config: TieredSinkConfig,
    shutdown: Arc<Notify>,
    drain_handle: Option<JoinHandle<()>>,
    disk_available: Arc<std::sync::atomic::AtomicBool>,
    #[allow(dead_code)]
    disk_poller_handle: Option<JoinHandle<()>>,

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

        // Initialise spool counters from existing queue contents
        let (initial_count, initial_bytes) = spool_item_count_and_bytes(&config.spool_path);
        let spool_count = Arc::new(AtomicU64::new(initial_count));
        let spool_bytes = Arc::new(AtomicU64::new(initial_bytes));

        let circuit = Arc::new(CircuitBreaker::new(
            config.circuit_failure_threshold,
            config.circuit_reset_timeout(),
        ));
        let shutdown = Arc::new(Notify::new());
        let codec = config.compression;
        let disk_available = Arc::new(std::sync::atomic::AtomicBool::new(true));

        // Start disk-aware capacity poller if configured
        let disk_poller_handle = config.disk_aware.as_ref().map(|disk_cfg| {
            let spool_path = config.spool_path.clone();
            let disk_flag = Arc::clone(&disk_available);
            let shutdown_clone = Arc::clone(&shutdown);
            let poll_interval = std::time::Duration::from_secs(disk_cfg.poll_interval_secs);
            let max_usage = disk_cfg.max_usage_percent;

            tokio::spawn(disk_capacity_poller(
                spool_path,
                disk_flag,
                max_usage,
                poll_interval,
                shutdown_clone,
            ))
        });

        // Start drain task
        let drain_handle = tokio::spawn(drainer::drain_loop(
            Arc::clone(&sink),
            Arc::clone(&spool_receiver),
            Arc::clone(&spool_count),
            Arc::clone(&spool_bytes),
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
            spool_bytes,
            circuit,
            codec,
            config,
            shutdown,
            drain_handle: Some(drain_handle),
            disk_available,
            disk_poller_handle,
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
                    #[cfg(feature = "metrics")]
                    ::metrics::counter!("dfe_spool_hot_path_total").increment(1);
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
        #[cfg(feature = "metrics")]
        ::metrics::counter!("dfe_spool_cold_path_total").increment(1);
        Ok(())
    }

    /// Determine if we should attempt the hot path.
    async fn should_use_hot_path(&self) -> bool {
        let circuit_state = self.circuit.state().await;

        #[cfg(feature = "metrics")]
        ::metrics::gauge!("dfe_spool_circuit_state").set(match circuit_state {
            CircuitState::Closed => 0.0,
            CircuitState::HalfOpen => 1.0,
            CircuitState::Open => 2.0,
        });

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
                #[cfg(feature = "metrics")]
                ::metrics::counter!("dfe_spool_circuit_trips_total").increment(1);
                Err(TieredSinkError::Spool("sink unavailable".into()))
            }
            Ok(Err(SinkError::Fatal(e))) => {
                // Fatal error - propagate, don't spool
                Err(TieredSinkError::Sink(e.to_string()))
            }
            Err(_timeout) => {
                self.circuit.record_failure().await;
                #[cfg(feature = "metrics")]
                ::metrics::counter!("dfe_spool_circuit_trips_total").increment(1);
                Err(TieredSinkError::Spool("send timeout".into()))
            }
        }
    }

    /// Spool a message to disk.
    async fn spool_message(&self, data: &[u8]) -> Result<()> {
        // Check disk availability first
        if !self.disk_available.load(AtomicOrdering::Relaxed) {
            return Err(TieredSinkError::DiskUnavailable);
        }

        let compressed = self.codec.compress(data)?;
        let compressed_len = compressed.len() as u64;

        // Check item count limit
        if let Some(max_items) = self.config.max_spool_items {
            #[allow(clippy::cast_possible_truncation)]
            let current = self.spool_count.load(AtomicOrdering::Relaxed) as usize;
            if current >= max_items {
                return Err(TieredSinkError::SpoolFull(format!(
                    "max items {max_items} reached"
                )));
            }
        }

        // Check byte size limit
        if let Some(max_bytes) = self.config.max_spool_bytes {
            let current_bytes = self.spool_bytes.load(AtomicOrdering::Relaxed);
            if current_bytes + compressed_len > max_bytes {
                return Err(TieredSinkError::SpoolFull(format!(
                    "max spool bytes {max_bytes} reached (current: {current_bytes}, \
                     new message: {compressed_len})"
                )));
            }
        }

        let mut sender = self.spool_sender.lock().await;
        sender
            .send(compressed)
            .await
            .map_err(|e| TieredSinkError::Spool(e.to_string()))?;

        self.spool_count.fetch_add(1, AtomicOrdering::Relaxed);
        self.spool_bytes
            .fetch_add(compressed_len, AtomicOrdering::Relaxed);

        #[cfg(feature = "metrics")]
        {
            ::metrics::gauge!("dfe_spool_messages")
                .set(self.spool_count.load(AtomicOrdering::Relaxed) as f64);
            ::metrics::gauge!("dfe_spool_bytes")
                .set(self.spool_bytes.load(AtomicOrdering::Relaxed) as f64);
        }

        #[cfg(feature = "logger")]
        tracing::debug!(
            spool_items = self.spool_count.load(AtomicOrdering::Relaxed),
            spool_bytes = self.spool_bytes.load(AtomicOrdering::Relaxed),
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

    /// Get the approximate number of bytes currently in the spool.
    #[must_use]
    pub fn spool_bytes(&self) -> u64 {
        self.spool_bytes.load(AtomicOrdering::Relaxed)
    }

    /// Check if disk is available for spooling.
    #[must_use]
    pub fn is_disk_available(&self) -> bool {
        self.disk_available.load(AtomicOrdering::Relaxed)
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

/// Count existing items and sum payload bytes in a yaque queue directory.
///
/// yaque stores messages as `[4-byte Hamming header][payload]` in segment files
/// named `<n>.q`. The receiver position is persisted in `recv-metadata`.
///
/// Returns `(item_count, payload_bytes)`.
fn spool_item_count_and_bytes(path: &std::path::Path) -> (u64, u64) {
    if !path.is_dir() {
        return (0, 0);
    }

    // Read receiver state from recv-metadata (two big-endian u64: segment, position)
    let recv_metadata_path = path.join("recv-metadata");
    let (recv_segment, recv_position) = if recv_metadata_path.exists() {
        std::fs::read(&recv_metadata_path)
            .ok()
            .and_then(|data| {
                if data.len() >= 16 {
                    let segment = u64::from_be_bytes(data[0..8].try_into().ok()?);
                    let position = u64::from_be_bytes(data[8..16].try_into().ok()?);
                    Some((segment, position))
                } else {
                    None
                }
            })
            .unwrap_or((0, 0))
    } else {
        (0, 0)
    };

    // Collect segment files at or after the receiver position
    let mut segments: Vec<u64> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let file_path = entry.path();
            if file_path.extension().and_then(|e| e.to_str()) == Some("q")
                && let Some(seg_num) = file_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse::<u64>().ok())
                && seg_num >= recv_segment
            {
                segments.push(seg_num);
            }
        }
    }
    segments.sort_unstable();

    let header_eof: [u8; 4] = [255, 255, 255, 255];
    let mut count = 0u64;
    let mut bytes = 0u64;

    for &seg_num in &segments {
        let seg_path = path.join(format!("{seg_num}.q"));
        let Ok(file_data) = std::fs::read(&seg_path) else {
            continue;
        };

        #[allow(clippy::cast_possible_truncation)]
        let start = if seg_num == recv_segment {
            recv_position as usize
        } else {
            0
        };

        let mut pos = start;
        while pos + 4 <= file_data.len() {
            let header_bytes: [u8; 4] = file_data[pos..pos + 4].try_into().unwrap_or([0; 4]);
            if header_bytes == header_eof {
                break;
            }
            let encoded = u32::from_be_bytes(header_bytes);
            let payload_len = (encoded & 0x03_FF_FF_FF) as usize;
            pos += 4 + payload_len;
            if pos <= file_data.len() {
                count += 1;
                bytes += payload_len as u64;
            }
        }
    }

    (count, bytes)
}

/// Check available disk space using `statvfs`.
///
/// Returns `(total_bytes, available_bytes)` for the filesystem containing `path`.
///
/// # Safety
///
/// Calls `libc::statvfs` which is unsafe but well-defined when given a valid path.
#[allow(unsafe_code)]
fn check_disk_space(path: &std::path::Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    let c_path = CString::new(path.to_string_lossy().as_bytes()).ok()?;

    // SAFETY: zeroed statvfs is a valid initialisation for the struct
    // before passing to libc::statvfs which fills all fields.
    // c_path is a valid null-terminated C string pointing to an existing
    // filesystem path, and stat is a properly-sized statvfs struct.
    #[allow(unsafe_code)]
    let stat = unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        let result = libc::statvfs(c_path.as_ptr(), &raw mut stat);
        if result != 0 {
            return None;
        }
        stat
    };

    let block_size = stat.f_frsize;
    let total = stat.f_blocks * block_size;
    let available = stat.f_bavail * block_size;
    Some((total, available))
}

/// Background poller that checks disk usage and sets a flag.
async fn disk_capacity_poller(
    spool_path: std::path::PathBuf,
    disk_available: Arc<std::sync::atomic::AtomicBool>,
    max_usage_percent: f64,
    poll_interval: std::time::Duration,
    shutdown: Arc<Notify>,
) {
    loop {
        tokio::select! {
            () = shutdown.notified() => {
                #[cfg(feature = "logger")]
                tracing::debug!("Disk capacity poller shutting down");
                return;
            }
            () = tokio::time::sleep(poll_interval) => {}
        }

        let disk_space = check_disk_space(&spool_path);

        #[cfg(feature = "metrics")]
        if let Some((total, avail)) = disk_space {
            ::metrics::gauge!("dfe_spool_disk_available_bytes").set(avail as f64);
            ::metrics::gauge!("dfe_spool_disk_total_bytes").set(total as f64);
        }

        let available = disk_space.is_none_or(|(total, avail)| {
            if total == 0 {
                return true;
            }
            let used_ratio = 1.0 - (avail as f64 / total as f64);
            let ok = used_ratio < max_usage_percent;
            #[cfg(feature = "logger")]
            if !ok {
                tracing::warn!(
                    used_percent = format!("{:.1}%", used_ratio * 100.0),
                    threshold = format!("{:.1}%", max_usage_percent * 100.0),
                    "Disk usage exceeds threshold, pausing spool writes"
                );
            }
            ok
        });

        disk_available.store(available, std::sync::atomic::Ordering::Relaxed);
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

    #[tokio::test]
    async fn test_max_spool_bytes_enforced() {
        let dir = tempdir().unwrap();
        let spool_path = dir.path().join("test-bytes-limit");

        let sink = TestSink::new();
        sink.set_available(false);

        let mut config = TieredSinkConfig::new(&spool_path);
        config.circuit_failure_threshold = 1;
        // Set a very small byte limit
        config.max_spool_bytes = Some(50);

        let tiered = TieredSink::new(sink, config).await.unwrap();

        // First message should spool (compressed size fits)
        tiered.send(b"small").await.unwrap();
        assert_eq!(tiered.cold_path_count(), 1);
        assert!(tiered.spool_bytes() > 0);

        // Keep sending until we hit the limit
        let mut hit_limit = false;
        for _ in 0..100 {
            match tiered.send(b"more data here").await {
                Ok(()) => {}
                Err(TieredSinkError::SpoolFull(msg)) => {
                    assert!(msg.contains("max spool bytes"));
                    hit_limit = true;
                    break;
                }
                Err(e) => panic!("unexpected error: {e}"),
            }
        }
        assert!(hit_limit, "should have hit spool byte limit");

        tiered.shutdown().await;
    }

    #[tokio::test]
    async fn test_spool_bytes_decremented_on_drain() {
        let dir = tempdir().unwrap();
        let spool_path = dir.path().join("test-bytes-drain");

        let sink = TestSink::new();
        sink.set_available(false);

        let mut config = TieredSinkConfig::new(&spool_path);
        config.circuit_failure_threshold = 1;
        config.circuit_reset_timeout_ms = 50;
        config.drain_interval_ms = 10;

        let tiered = TieredSink::new(sink, config).await.unwrap();

        // Spool some messages
        tiered.send(b"drain me").await.unwrap();
        tiered.send(b"drain me too").await.unwrap();
        let bytes_after_spool = tiered.spool_bytes();
        assert!(bytes_after_spool > 0);

        // Make sink available and wait for drain
        tiered.inner().set_available(true);
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        // Bytes should be decremented
        assert_eq!(tiered.spool_bytes(), 0);
        assert!(tiered.spool_is_empty().await);

        tiered.shutdown().await;
    }

    #[tokio::test]
    async fn test_spool_count_initialised_from_existing_queue() {
        let dir = tempdir().unwrap();
        let spool_path = dir.path().join("test-init-count");

        // Phase 1: Create a TieredSink, spool messages, then drop
        {
            let sink = TestSink::new();
            sink.set_available(false);

            let mut config = TieredSinkConfig::new(&spool_path);
            config.circuit_failure_threshold = 1;

            let tiered = TieredSink::new(sink, config).await.unwrap();

            tiered.send(b"message 1").await.unwrap();
            tiered.send(b"message 2").await.unwrap();
            tiered.send(b"message 3").await.unwrap();
            assert_eq!(tiered.spool_len().await, 3);

            tiered.shutdown().await;
        }

        // Phase 2: Re-open — spool_count should reflect existing items
        {
            let sink = TestSink::new();
            let config = TieredSinkConfig::new(&spool_path);
            let tiered = TieredSink::new(sink, config).await.unwrap();

            assert_eq!(tiered.spool_len().await, 3);
            assert!(tiered.spool_bytes() > 0);

            tiered.shutdown().await;
        }
    }

    #[tokio::test]
    async fn test_disk_available_flag() {
        let dir = tempdir().unwrap();
        let spool_path = dir.path().join("test-disk-flag");

        let sink = TestSink::new();
        sink.set_available(false);

        let mut config = TieredSinkConfig::new(&spool_path);
        config.circuit_failure_threshold = 1;

        let tiered = TieredSink::new(sink, config).await.unwrap();

        // By default, disk should be available
        assert!(tiered.is_disk_available());

        // Manually set flag to false to simulate full disk
        tiered.disk_available.store(false, AtomicOrdering::Relaxed);

        let result = tiered.send(b"should fail").await;
        assert!(matches!(result, Err(TieredSinkError::DiskUnavailable)));

        tiered.shutdown().await;
    }
}
