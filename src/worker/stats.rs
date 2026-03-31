// Project:   hyperi-rustlib
// File:      src/worker/stats.rs
// Purpose:   Atomic pipeline statistics for lock-free concurrent access
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Atomic pipeline statistics for DFE services.
//!
//! Every DFE pipeline tracks the same base counters: received, processed,
//! errors, DLQ. These use [`AtomicU64`] for lock-free updates from both
//! the parallel (rayon) and sequential phases.
//!
//! App-specific stats extend separately — these are the common fields
//! shared across all 6 DFE pipeline projects.

use std::sync::atomic::{AtomicU64, Ordering};

/// Common DFE pipeline statistics with atomic counters.
///
/// Lock-free, safe to read and write from any thread. Uses
/// `Ordering::Relaxed` — stats are informational, not safety-critical.
#[derive(Debug, Default)]
pub struct PipelineStats {
    /// Messages received from source (Kafka, HTTP, gRPC, etc.).
    pub received: AtomicU64,
    /// Messages successfully processed through the pipeline.
    pub processed: AtomicU64,
    /// Messages that failed processing and were routed to DLQ or dropped.
    pub errors: AtomicU64,
    /// Messages sent to the dead letter queue (subset of errors, only if DLQ enabled).
    pub dlq: AtomicU64,
    /// Total bytes received from source.
    pub bytes_received: AtomicU64,
    /// Total bytes written to sink.
    pub bytes_written: AtomicU64,
    /// Batches flushed to sink.
    pub batches_flushed: AtomicU64,
}

impl PipelineStats {
    /// Create new zeroed stats.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // --- Increment helpers (single message) ---

    pub fn incr_received(&self) {
        self.received.fetch_add(1, Ordering::Relaxed);
    }

    pub fn incr_processed(&self) {
        self.processed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn incr_errors(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn incr_dlq(&self) {
        self.dlq.fetch_add(1, Ordering::Relaxed);
    }

    pub fn incr_batches_flushed(&self) {
        self.batches_flushed.fetch_add(1, Ordering::Relaxed);
    }

    // --- Bulk add helpers (batch-level) ---

    pub fn add_received(&self, n: u64) {
        self.received.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_processed(&self, n: u64) {
        self.processed.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_bytes_received(&self, n: u64) {
        self.bytes_received.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_bytes_written(&self, n: u64) {
        self.bytes_written.fetch_add(n, Ordering::Relaxed);
    }

    /// Take an immutable snapshot for logging, metrics, or display.
    #[must_use]
    pub fn snapshot(&self) -> PipelineStatsSnapshot {
        PipelineStatsSnapshot {
            received: self.received.load(Ordering::Relaxed),
            processed: self.processed.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            dlq: self.dlq.load(Ordering::Relaxed),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
            batches_flushed: self.batches_flushed.load(Ordering::Relaxed),
        }
    }
}

/// Immutable snapshot of pipeline stats.
///
/// Safe to copy, pass between threads, and use in logging/display without
/// holding any reference to the atomic counters.
#[derive(Debug, Clone, Copy, Default)]
pub struct PipelineStatsSnapshot {
    pub received: u64,
    pub processed: u64,
    pub errors: u64,
    pub dlq: u64,
    pub bytes_received: u64,
    pub bytes_written: u64,
    pub batches_flushed: u64,
}

impl std::fmt::Display for PipelineStatsSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "received={} processed={} errors={} dlq={} batches={}",
            self.received, self.processed, self.errors, self.dlq, self.batches_flushed,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_zero() {
        let stats = PipelineStats::new();
        let snap = stats.snapshot();
        assert_eq!(snap.received, 0);
        assert_eq!(snap.processed, 0);
        assert_eq!(snap.errors, 0);
        assert_eq!(snap.dlq, 0);
    }

    #[test]
    fn test_increments() {
        let stats = PipelineStats::new();
        stats.incr_received();
        stats.incr_received();
        stats.incr_processed();
        stats.incr_errors();
        stats.incr_dlq();
        stats.add_bytes_received(1024);

        let snap = stats.snapshot();
        assert_eq!(snap.received, 2);
        assert_eq!(snap.processed, 1);
        assert_eq!(snap.errors, 1);
        assert_eq!(snap.dlq, 1);
        assert_eq!(snap.bytes_received, 1024);
    }

    #[test]
    fn test_bulk_add() {
        let stats = PipelineStats::new();
        stats.add_received(100);
        stats.add_processed(95);
        stats.add_bytes_written(4096);
        stats.incr_batches_flushed();

        let snap = stats.snapshot();
        assert_eq!(snap.received, 100);
        assert_eq!(snap.processed, 95);
        assert_eq!(snap.bytes_written, 4096);
        assert_eq!(snap.batches_flushed, 1);
    }

    #[test]
    fn test_snapshot_is_copy() {
        let stats = PipelineStats::new();
        stats.add_received(42);
        let snap = stats.snapshot();
        let copy = snap; // Copy
        assert_eq!(snap.received, copy.received);
    }

    #[test]
    fn test_display() {
        let stats = PipelineStats::new();
        stats.add_received(100);
        stats.add_processed(90);
        let snap = stats.snapshot();
        let display = format!("{snap}");
        assert!(display.contains("received=100"));
        assert!(display.contains("processed=90"));
    }
}
