// Project:   hyperi-rustlib
// File:      src/worker/accumulator.rs
// Purpose:   Bounded batch accumulator with time/count/bytes drain thresholds
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Bounded batch accumulator for DFE pipeline batching.
//!
//! Accumulates items from multiple producers (HTTP handlers, gRPC handlers, etc.)
//! and drains them as batches when any threshold is met:
//! - Item count reaches `max_items`
//! - Byte count reaches `max_bytes`
//! - Time since last drain reaches `max_wait`
//!
//! Bounded -- pushers get an error when the channel is full (backpressure).
//! Shutdown-safe -- `drain_remaining()` flushes buffered items.
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::worker::BatchAccumulator;
//! use std::time::Duration;
//!
//! let (acc, mut drainer) = BatchAccumulator::new(
//!     1000,                        // channel capacity (backpressure bound)
//!     100,                         // max items per batch
//!     1024 * 1024,                 // max bytes per batch (1MB)
//!     Duration::from_millis(10),   // max wait before flush
//! );
//!
//! // Producers push (from HTTP handlers, etc.)
//! acc.push(payload, payload.len()).await?;
//!
//! // Consumer drains batches (background task)
//! loop {
//!     let batch = drainer.next_batch().await;
//!     if batch.is_empty() { break; } // shutdown
//!     process_batch(&batch);
//! }
//! ```

use std::time::Duration;

use tokio::sync::mpsc;

/// Accumulator configuration.
#[derive(Debug, Clone)]
pub struct AccumulatorConfig {
    /// Channel capacity (bounded -- pushers get error when full).
    pub channel_capacity: usize,
    /// Maximum items per batch before auto-drain.
    pub max_items: usize,
    /// Maximum accumulated bytes per batch before auto-drain.
    pub max_bytes: usize,
    /// Maximum time since last drain before auto-flush.
    pub max_wait: Duration,
}

impl Default for AccumulatorConfig {
    fn default() -> Self {
        Self {
            channel_capacity: 10_000,
            max_items: 100,
            max_bytes: 1024 * 1024, // 1MB
            max_wait: Duration::from_millis(10),
        }
    }
}

/// Push handle -- cloneable, used by producers to send items into the accumulator.
#[derive(Clone)]
pub struct BatchAccumulator<T> {
    tx: mpsc::Sender<(T, usize)>, // (item, byte_size)
}

/// Drain handle -- used by a single consumer to receive batches.
pub struct BatchDrainer<T> {
    rx: mpsc::Receiver<(T, usize)>,
    config: AccumulatorConfig,
    buffer: Vec<T>,
    buffer_bytes: usize,
}

/// Error when the accumulator channel is full (backpressure).
#[derive(Debug, thiserror::Error)]
#[error("accumulator full -- backpressure active ({capacity} items buffered)")]
pub struct AccumulatorFull {
    pub capacity: usize,
}

impl<T: Send + 'static> BatchAccumulator<T> {
    /// Create a new accumulator + drainer pair.
    ///
    /// Returns `(push_handle, drain_handle)`. The push handle is `Clone` for
    /// sharing across HTTP/gRPC handlers. The drain handle is used by a single
    /// background task to receive batches.
    #[must_use]
    pub fn new(config: AccumulatorConfig) -> (Self, BatchDrainer<T>) {
        let (tx, rx) = mpsc::channel(config.channel_capacity);
        let drainer = BatchDrainer {
            rx,
            buffer: Vec::with_capacity(config.max_items),
            buffer_bytes: 0,
            config: config.clone(),
        };
        (Self { tx }, drainer)
    }

    /// Push an item into the accumulator.
    ///
    /// `byte_size` is used for the bytes threshold. Pass `payload.len()`.
    ///
    /// # Errors
    ///
    /// Returns `AccumulatorFull` if the channel is at capacity (backpressure).
    pub async fn push(&self, item: T, byte_size: usize) -> Result<(), AccumulatorFull> {
        self.tx
            .try_send((item, byte_size))
            .map_err(|_| AccumulatorFull {
                // `max_capacity()` is the configured channel size. `capacity()`
                // is *remaining* permits (~0 here, since we only build this
                // error when the channel is full), which rendered a misleading
                // "(0 items buffered)" diagnostic.
                capacity: self.tx.max_capacity(),
            })
    }

    /// Check if the accumulator has been closed (drainer dropped).
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// Drain accumulated [`Record`]s into a [`WorkBatch`] for push-ingest sources.
///
/// Push-ingest transports (HTTP, gRPC) accumulate [`Record`]s via a
/// `BatchAccumulator<Record>`, then bridge the drained block to the engine's
/// canonical [`WorkBatch`] currency. The `commit_tokens` are supplied by the
/// caller because the push source owns the ack (an HTTP responder, a gRPC
/// stream slot) -- the accumulator carries only the payload records.
///
/// [`Record`]: crate::transport::Record
/// [`WorkBatch`]: crate::transport::WorkBatch
#[cfg(feature = "transport")]
#[must_use]
pub fn records_into_work_batch<T: crate::transport::CommitToken>(
    records: Vec<crate::transport::Record>,
    commit_tokens: Vec<T>,
) -> crate::transport::WorkBatch<T> {
    crate::transport::WorkBatch::new(records, commit_tokens)
}

impl<T> BatchDrainer<T> {
    /// Wait for the next batch.
    ///
    /// Blocks until any threshold is met (items, bytes, or time). Returns
    /// an empty vec when the channel is closed (all producers dropped = shutdown).
    pub async fn next_batch(&mut self) -> Vec<T> {
        // If buffer already meets a threshold, drain immediately
        if self.threshold_met() {
            return self.take_buffer();
        }

        // Wait for items against a FIXED deadline. The deadline is set once per
        // accumulation window (not recreated per arriving item), so trickle
        // traffic -- items arriving every < max_wait -- cannot defer the flush
        // indefinitely. The prior code created a fresh `sleep(max_wait)` on every
        // loop iteration, so each arrival reset the timer and the first buffered
        // item's latency was unbounded, breaking the documented "time since last
        // drain reaches max_wait" guarantee.
        let sleep = tokio::time::sleep(self.config.max_wait);
        tokio::pin!(sleep);

        loop {
            tokio::select! {
                biased;

                // Time threshold -- flush whatever we have
                () = &mut sleep => {
                    if self.buffer.is_empty() {
                        // No items at all -- re-arm a fresh window and keep
                        // waiting (don't return an empty batch).
                        sleep
                            .as_mut()
                            .reset(tokio::time::Instant::now() + self.config.max_wait);
                        continue;
                    }
                    return self.take_buffer();
                }

                // New item arrived
                item = self.rx.recv() => {
                    match item {
                        Some((val, size)) => {
                            self.buffer_bytes += size;
                            self.buffer.push(val);
                            if self.threshold_met() {
                                return self.take_buffer();
                            }
                        }
                        None => {
                            // Channel closed -- drain remaining
                            return self.take_buffer();
                        }
                    }
                }
            }
        }
    }

    /// Drain any remaining buffered items (for graceful shutdown).
    pub fn drain_remaining(&mut self) -> Vec<T> {
        // Drain channel
        while let Ok((val, size)) = self.rx.try_recv() {
            self.buffer_bytes += size;
            self.buffer.push(val);
        }
        self.take_buffer()
    }

    fn threshold_met(&self) -> bool {
        self.buffer.len() >= self.config.max_items || self.buffer_bytes >= self.config.max_bytes
    }

    fn take_buffer(&mut self) -> Vec<T> {
        self.buffer_bytes = 0;
        std::mem::take(&mut self.buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_drain_on_item_count() {
        let config = AccumulatorConfig {
            channel_capacity: 100,
            max_items: 5,
            max_bytes: usize::MAX,
            max_wait: Duration::from_mins(1), // won't trigger
        };
        let (acc, mut drainer) = BatchAccumulator::new(config);

        // Push 5 items -- should trigger drain
        for i in 0..5 {
            acc.push(i, 1).await.unwrap();
        }

        let batch = drainer.next_batch().await;
        assert_eq!(batch.len(), 5);
        assert_eq!(batch, vec![0, 1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn test_drain_on_byte_threshold() {
        let config = AccumulatorConfig {
            channel_capacity: 100,
            max_items: 1000, // won't trigger
            max_bytes: 10,   // trigger at 10 bytes
            max_wait: Duration::from_mins(1),
        };
        let (acc, mut drainer) = BatchAccumulator::new(config);

        // Push items with size=3 each -- 4 items = 12 bytes > 10 threshold
        for i in 0..4 {
            acc.push(i, 3).await.unwrap();
        }

        let batch = drainer.next_batch().await;
        assert_eq!(batch.len(), 4);
    }

    #[tokio::test]
    async fn test_drain_on_time_threshold() {
        let config = AccumulatorConfig {
            channel_capacity: 100,
            max_items: 1000,
            max_bytes: usize::MAX,
            max_wait: Duration::from_millis(50), // 50ms
        };
        let (acc, mut drainer) = BatchAccumulator::new(config);

        // Push 2 items (below count/byte threshold)
        acc.push(1, 1).await.unwrap();
        acc.push(2, 1).await.unwrap();

        // Drain should fire after 50ms timeout
        let batch = drainer.next_batch().await;
        assert_eq!(batch.len(), 2);
    }

    #[tokio::test]
    async fn test_backpressure_when_full() {
        let config = AccumulatorConfig {
            channel_capacity: 3,
            max_items: 100,
            max_bytes: usize::MAX,
            max_wait: Duration::from_mins(1),
        };
        let (acc, _drainer) = BatchAccumulator::<i32>::new(config);

        // Fill to capacity
        acc.push(1, 1).await.unwrap();
        acc.push(2, 1).await.unwrap();
        acc.push(3, 1).await.unwrap();

        // Next push should fail (backpressure)
        let result = acc.push(4, 1).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_backpressure_error_reports_configured_capacity() {
        let config = AccumulatorConfig {
            channel_capacity: 3,
            max_items: 100,
            max_bytes: usize::MAX,
            max_wait: Duration::from_mins(1),
        };
        let (acc, _drainer) = BatchAccumulator::<i32>::new(config);

        acc.push(1, 1).await.unwrap();
        acc.push(2, 1).await.unwrap();
        acc.push(3, 1).await.unwrap();

        let err = acc.push(4, 1).await.expect_err("channel full -> error");
        // Reports the CONFIGURED capacity (3), not the remaining permits (0).
        assert_eq!(err.capacity, 3);
    }

    /// Items arriving steadily faster than `max_wait` must NOT defer the flush:
    /// the accumulation-window deadline is fixed per batch, so it fires even
    /// while items keep coming. Regression for the prior per-iteration
    /// `sleep(max_wait)` that was recreated on every arrival, resetting the
    /// timer indefinitely under trickle traffic. `start_paused` makes the clock
    /// deterministic.
    #[tokio::test(start_paused = true)]
    async fn test_trickle_traffic_flushes_on_fixed_deadline() {
        let config = AccumulatorConfig {
            channel_capacity: 100,
            max_items: 1000,       // never trips on count
            max_bytes: usize::MAX, // never trips on bytes
            max_wait: Duration::from_millis(100),
        };
        let (acc, mut drainer) = BatchAccumulator::<i32>::new(config);

        // One item every 40ms -- well under the 100ms window.
        tokio::spawn(async move {
            for i in 0..6 {
                acc.push(i, 1).await.unwrap();
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
        });

        let batch = drainer.next_batch().await;
        // The fixed 100ms deadline flushes the items buffered within the window
        // (those landing at 0/40/80ms), NOT all 6. Old reset-per-arrival
        // behaviour would only flush after the producer stopped.
        assert!(
            !batch.is_empty(),
            "should flush items buffered within the window"
        );
        assert!(
            batch.len() < 6,
            "expected a partial flush at the fixed deadline, got all {} items \
             (timer reset on each arrival?)",
            batch.len()
        );
    }

    #[tokio::test]
    async fn test_shutdown_drains_remaining() {
        let config = AccumulatorConfig {
            channel_capacity: 100,
            max_items: 1000,
            max_bytes: usize::MAX,
            max_wait: Duration::from_mins(1),
        };
        let (acc, mut drainer) = BatchAccumulator::new(config);

        acc.push(10, 1).await.unwrap();
        acc.push(20, 1).await.unwrap();

        // Drop the push handle (simulates shutdown)
        drop(acc);

        // next_batch should return remaining items
        let batch = drainer.next_batch().await;
        assert_eq!(batch, vec![10, 20]);

        // Subsequent call returns empty (channel closed)
        let batch = drainer.next_batch().await;
        assert!(batch.is_empty());
    }

    #[tokio::test]
    async fn test_multiple_batches() {
        let config = AccumulatorConfig {
            channel_capacity: 100,
            max_items: 3,
            max_bytes: usize::MAX,
            max_wait: Duration::from_mins(1),
        };
        let (acc, mut drainer) = BatchAccumulator::new(config);

        // Push 7 items -- should produce 2 full batches + 1 partial
        for i in 0..7 {
            acc.push(i, 1).await.unwrap();
        }
        drop(acc); // signal shutdown to drain the last partial

        let b1 = drainer.next_batch().await;
        assert_eq!(b1.len(), 3);

        let b2 = drainer.next_batch().await;
        assert_eq!(b2.len(), 3);

        let b3 = drainer.next_batch().await;
        assert_eq!(b3.len(), 1); // remaining partial batch

        let b4 = drainer.next_batch().await;
        assert!(b4.is_empty()); // channel closed
    }

    #[tokio::test]
    async fn test_push_handle_is_clone() {
        let config = AccumulatorConfig::default();
        let (acc, mut drainer) = BatchAccumulator::new(config);

        let acc2 = acc.clone();

        acc.push(1, 1).await.unwrap();
        acc2.push(2, 1).await.unwrap();

        drop(acc);
        drop(acc2);

        let batch = drainer.next_batch().await;
        assert_eq!(batch.len(), 2);
    }

    #[tokio::test]
    async fn test_drain_remaining_on_shutdown() {
        let config = AccumulatorConfig {
            channel_capacity: 100,
            max_items: 1000,
            max_bytes: usize::MAX,
            max_wait: Duration::from_mins(1),
        };
        let (acc, mut drainer) = BatchAccumulator::new(config);

        acc.push(1, 1).await.unwrap();
        acc.push(2, 1).await.unwrap();
        acc.push(3, 1).await.unwrap();
        drop(acc);

        let remaining = drainer.drain_remaining();
        assert_eq!(remaining, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn test_empty_drain_returns_empty() {
        let config = AccumulatorConfig::default();
        let (_acc, mut drainer) = BatchAccumulator::<i32>::new(config);

        let remaining = drainer.drain_remaining();
        assert!(remaining.is_empty());
    }

    /// Push-ingest helper: drained Records + supplied tokens become a WorkBatch.
    #[cfg(feature = "transport")]
    #[tokio::test]
    async fn test_records_drain_into_work_batch() {
        use crate::transport::{CommitToken, PayloadFormat, Record, RecordMeta};
        use bytes::Bytes;

        #[derive(Debug, Clone)]
        struct PushTok(u64);
        impl std::fmt::Display for PushTok {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "push-{}", self.0)
            }
        }
        impl CommitToken for PushTok {}

        let record = |payload: &'static [u8]| Record {
            payload: Bytes::from_static(payload),
            key: None,
            headers: vec![],
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Json,
            },
        };

        let config = AccumulatorConfig {
            channel_capacity: 100,
            max_items: 3,
            max_bytes: usize::MAX,
            max_wait: Duration::from_mins(1),
        };
        let (acc, mut drainer) = BatchAccumulator::<Record>::new(config);
        acc.push(record(b"{\"a\":1}"), 7).await.unwrap();
        acc.push(record(b"{\"b\":2}"), 7).await.unwrap();
        acc.push(record(b"{\"c\":3}"), 7).await.unwrap();

        let block = drainer.next_batch().await;
        assert_eq!(block.len(), 3);

        // Two source acks for a three-record block (push sources ack per request,
        // not per record) -- the helper must NOT tie token count to record count.
        let tokens = vec![PushTok(1), PushTok(2)];
        let wb = records_into_work_batch(block, tokens);
        assert_eq!(wb.record_count(), 3);
        assert_eq!(wb.commit_tokens.len(), 2);
        assert!(wb.dlq_entries.is_empty());
    }
}
