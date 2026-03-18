// Project:   hyperi-rustlib
// File:      src/tiered_sink/drainer.rs
// Purpose:   Background drain task for spooled messages
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Background drain task for spooled messages.

use crate::tiered_sink::{CircuitBreaker, DrainStrategy, Sink, SinkError};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify};
use yaque::Receiver;

/// Drainer state and rate tracking.
pub struct Drainer {
    strategy: DrainStrategy,
    current_rate: f64, // messages per second
    success_count: u64,
    failure_count: u64,
    last_adjustment: Instant,
}

impl Drainer {
    /// Create a new drainer with the given strategy.
    pub fn new(strategy: DrainStrategy) -> Self {
        let initial_rate = match strategy {
            DrainStrategy::Adaptive { initial_rate, .. } => initial_rate as f64,
            DrainStrategy::RateLimited { msgs_per_sec } => msgs_per_sec as f64,
            DrainStrategy::Greedy => f64::MAX,
        };

        Self {
            strategy,
            current_rate: initial_rate,
            success_count: 0,
            failure_count: 0,
            last_adjustment: Instant::now(),
        }
    }

    /// Record a successful drain.
    pub fn record_success(&mut self) {
        self.success_count += 1;
        self.maybe_adjust_rate();
    }

    /// Record a failed drain attempt.
    pub fn record_failure(&mut self) {
        self.failure_count += 1;
        self.maybe_adjust_rate();
    }

    /// Get the current delay between drain operations.
    pub fn delay(&self) -> Duration {
        if self.current_rate >= f64::MAX || self.current_rate <= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(1.0 / self.current_rate)
        }
    }

    /// Adjust rate based on success/failure ratio.
    fn maybe_adjust_rate(&mut self) {
        let DrainStrategy::Adaptive {
            initial_rate,
            max_rate,
        } = self.strategy
        else {
            return; // Only adjust for adaptive strategy
        };

        // Adjust every 100 operations or 1 second, whichever comes first
        let total = self.success_count + self.failure_count;
        let elapsed = self.last_adjustment.elapsed();

        if total < 100 && elapsed < Duration::from_secs(1) {
            return;
        }

        let success_ratio = if total > 0 {
            self.success_count as f64 / total as f64
        } else {
            1.0
        };

        // Adjust rate based on success ratio
        self.current_rate = if success_ratio > 0.95 {
            // Very successful, increase rate
            (self.current_rate * 1.5).min(max_rate as f64)
        } else if success_ratio > 0.8 {
            // Mostly successful, small increase
            (self.current_rate * 1.1).min(max_rate as f64)
        } else if success_ratio > 0.5 {
            // Mixed results, maintain rate
            self.current_rate
        } else {
            // Failing, reduce rate
            (self.current_rate * 0.5).max(initial_rate as f64 / 10.0)
        };

        // Reset counters
        self.success_count = 0;
        self.failure_count = 0;
        self.last_adjustment = Instant::now();
    }

    /// Get current rate for metrics.
    pub fn current_rate(&self) -> f64 {
        self.current_rate
    }
}

/// Result of a drain attempt.
enum DrainResult {
    /// Successfully sent and committed
    Success,
    /// Sink is full (backpressure)
    SinkFull,
    /// Sink is unavailable
    SinkUnavailable,
    /// Fatal error sending
    Fatal(String),
    /// Decompression error
    DecompressError(String),
    /// Queue is empty
    Empty,
    /// I/O error
    IoError,
}

/// Run the drain loop.
///
/// This function runs until the notify is triggered or the spool is empty
/// and the circuit is closed.
#[allow(clippy::too_many_arguments)]
pub async fn drain_loop<S: Sink>(
    sink: Arc<S>,
    spool_receiver: Arc<Mutex<Receiver>>,
    spool_count: Arc<AtomicU64>,
    spool_bytes: Arc<AtomicU64>,
    circuit: Arc<CircuitBreaker>,
    codec: crate::tiered_sink::CompressionCodec,
    strategy: DrainStrategy,
    interval: Duration,
    shutdown: Arc<Notify>,
) {
    let mut drainer = Drainer::new(strategy);

    loop {
        // Check for shutdown
        tokio::select! {
            () = shutdown.notified() => {
                #[cfg(feature = "logger")]
                tracing::info!("Drain task shutting down");
                return;
            }
            () = tokio::time::sleep(interval) => {}
        }

        // Don't drain if circuit is open
        if circuit.is_open().await {
            continue;
        }

        // Try to receive, decompress, send, and commit all within the lock
        // This is necessary because RecvGuard borrows the Receiver
        let result: DrainResult = {
            let mut receiver = spool_receiver.lock().await;

            let recv_result = receiver.try_recv();
            match recv_result {
                Ok(guard) => {
                    // Copy the compressed data
                    let compressed = guard.to_vec();
                    let compressed_len = compressed.len() as u64;

                    // Decompress
                    let decompress_result = codec.decompress(&compressed);
                    match decompress_result {
                        Ok(data) => {
                            // Try to send
                            let send_result = sink.try_send(&data).await;
                            match send_result {
                                Ok(()) => {
                                    // Success - commit to remove from queue
                                    if let Err(e) = guard.commit() {
                                        #[cfg(feature = "logger")]
                                        tracing::error!(error = %e, "Failed to commit after successful send");
                                    }
                                    spool_count.fetch_sub(1, AtomicOrdering::Relaxed);
                                    spool_bytes.fetch_sub(compressed_len, AtomicOrdering::Relaxed);
                                    DrainResult::Success
                                }
                                Err(SinkError::Full) => {
                                    // Don't commit - guard drops and rolls back
                                    drop(guard);
                                    DrainResult::SinkFull
                                }
                                Err(SinkError::Unavailable) => {
                                    // Don't commit - guard drops and rolls back
                                    drop(guard);
                                    DrainResult::SinkUnavailable
                                }
                                Err(SinkError::Fatal(e)) => {
                                    // Commit to remove unprocessable message
                                    if let Err(commit_err) = guard.commit() {
                                        #[cfg(feature = "logger")]
                                        tracing::error!(error = %commit_err, "Failed to commit after fatal error");
                                    }
                                    spool_count.fetch_sub(1, AtomicOrdering::Relaxed);
                                    spool_bytes.fetch_sub(compressed_len, AtomicOrdering::Relaxed);
                                    DrainResult::Fatal(e.to_string())
                                }
                            }
                        }
                        Err(e) => {
                            // Commit to remove corrupted message
                            if let Err(commit_err) = guard.commit() {
                                #[cfg(feature = "logger")]
                                tracing::error!(error = %commit_err, "Failed to commit after decompression error");
                            }
                            spool_count.fetch_sub(1, AtomicOrdering::Relaxed);
                            spool_bytes.fetch_sub(compressed_len, AtomicOrdering::Relaxed);
                            DrainResult::DecompressError(e.to_string())
                        }
                    }
                }
                Err(yaque::TryRecvError::QueueEmpty) => DrainResult::Empty,
                Err(yaque::TryRecvError::Io(e)) => {
                    #[cfg(feature = "logger")]
                    tracing::warn!(error = %e, "I/O error reading from spool");
                    DrainResult::IoError
                }
            }
        };

        // Handle the result outside the lock
        match result {
            DrainResult::Success => {
                drainer.record_success();
                circuit.record_success().await;
                #[cfg(feature = "logger")]
                tracing::debug!(rate = drainer.current_rate(), "Drained message to sink");
            }
            DrainResult::SinkFull => {
                drainer.record_failure();
                #[cfg(feature = "logger")]
                tracing::debug!("Sink full during drain, will retry");
            }
            DrainResult::SinkUnavailable => {
                drainer.record_failure();
                circuit.record_failure().await;
                #[cfg(feature = "logger")]
                tracing::debug!("Sink unavailable during drain, circuit may open");
            }
            DrainResult::Fatal(e) => {
                #[cfg(feature = "logger")]
                tracing::error!(error = %e, "Fatal error during drain, dropping message");
            }
            DrainResult::DecompressError(e) => {
                #[cfg(feature = "logger")]
                tracing::error!(error = %e, "Failed to decompress spooled message, dropping");
            }
            DrainResult::Empty | DrainResult::IoError => {
                // Nothing to do, just continue
            }
        }

        // Apply rate limiting
        let delay = drainer.delay();
        if delay > Duration::ZERO {
            tokio::time::sleep(delay).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drainer_greedy_no_delay() {
        let drainer = Drainer::new(DrainStrategy::Greedy);
        assert_eq!(drainer.delay(), Duration::ZERO);
    }

    #[test]
    fn test_drainer_rate_limited() {
        let drainer = Drainer::new(DrainStrategy::RateLimited { msgs_per_sec: 100 });
        assert_eq!(drainer.delay(), Duration::from_millis(10));
    }

    #[test]
    fn test_drainer_adaptive_initial() {
        let drainer = Drainer::new(DrainStrategy::adaptive(100, 1000));
        assert_eq!(drainer.delay(), Duration::from_millis(10));
    }

    #[test]
    fn test_drainer_rate_adjustment() {
        let mut drainer = Drainer::new(DrainStrategy::adaptive(100, 10000));

        // Simulate 100 successes
        for _ in 0..100 {
            drainer.record_success();
        }

        // Rate should have increased
        assert!(drainer.current_rate() > 100.0);
    }
}
