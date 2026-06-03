// Project:   hyperi-rustlib
// File:      src/transport/filter/staging.rs
// Purpose:   Bounded staging buffer for inbound-filter DLQ entries
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Bounded DLQ staging buffer.
//!
//! Inbound filtering classifies some messages as `action: dlq`; they are
//! removed from the `recv()` result and staged here until the caller drains
//! them. Each transport previously held these in an unbounded
//! `Mutex<Vec<FilteredDlqEntry>>` -- if a caller missed the drain step, or a
//! high-rate filter routed a flood to DLQ, memory grew without bound,
//! independent of any transport queue limit (finding 4).
//!
//! `DlqStaging` bounds the buffer by entry count AND total payload bytes.
//! When full it rejects the newest entry (reject-newest), counts the overflow,
//! warns once, and emits a metric -- bounded memory takes priority over
//! retaining every dead-letter. Draining resets the bound.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use parking_lot::Mutex;

use super::FilteredDlqEntry;

/// Default cap: at most this many staged entries.
const DEFAULT_MAX_ENTRIES: usize = 10_000;
/// Default cap: at most this many staged payload bytes (64 MiB).
const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Bounded staging buffer for DLQ entries produced by inbound filtering.
pub struct DlqStaging {
    entries: Mutex<VecDeque<FilteredDlqEntry>>,
    max_entries: usize,
    max_bytes: usize,
    bytes: AtomicUsize,
    overflowed: AtomicU64,
    warned: AtomicBool,
}

impl Default for DlqStaging {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_ENTRIES, DEFAULT_MAX_BYTES)
    }
}

impl DlqStaging {
    /// Create a staging buffer bounded to `max_entries` entries and
    /// `max_bytes` total payload bytes. A zero bound is treated as 1 to keep
    /// the buffer usable.
    #[must_use]
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: Mutex::new(VecDeque::new()),
            max_entries: max_entries.max(1),
            max_bytes: max_bytes.max(1),
            bytes: AtomicUsize::new(0),
            overflowed: AtomicU64::new(0),
            warned: AtomicBool::new(false),
        }
    }

    /// Stage one entry. Rejected (and counted) if it would exceed either bound.
    pub fn push(&self, entry: FilteredDlqEntry) {
        let sz = entry.payload.len();
        let mut q = self.entries.lock();
        let would_bytes = self.bytes.load(Ordering::Relaxed).saturating_add(sz);
        if q.len() >= self.max_entries || would_bytes > self.max_bytes {
            drop(q);
            self.record_overflow();
            return;
        }
        q.push_back(entry);
        self.bytes.fetch_add(sz, Ordering::Relaxed);
    }

    /// Stage many entries (each subject to the bound).
    pub fn push_all(&self, entries: impl IntoIterator<Item = FilteredDlqEntry>) {
        for e in entries {
            self.push(e);
        }
    }

    /// Drain all staged entries, resetting the byte counter.
    pub fn drain(&self) -> Vec<FilteredDlqEntry> {
        let mut q = self.entries.lock();
        self.bytes.store(0, Ordering::Relaxed);
        q.drain(..).collect()
    }

    /// Number of entries currently staged.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    /// Whether the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.lock().is_empty()
    }

    /// Total entries rejected due to the bound since construction.
    #[must_use]
    pub fn overflowed(&self) -> u64 {
        self.overflowed.load(Ordering::Relaxed)
    }

    fn record_overflow(&self) {
        self.overflowed.fetch_add(1, Ordering::Relaxed);
        #[cfg(feature = "metrics")]
        metrics::counter!("dfe_transport_dlq_staging_overflow_total").increment(1);
        if !self.warned.swap(true, Ordering::Relaxed) {
            tracing::warn!(
                max_entries = self.max_entries,
                max_bytes = self.max_bytes,
                "DLQ staging buffer full -- dropping filtered DLQ entries; \
                 drain take_filtered_dlq_entries() more often or raise the bound"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: usize) -> FilteredDlqEntry {
        FilteredDlqEntry {
            payload: vec![0u8; n],
            key: None,
            reason: "test".to_string(),
        }
    }

    #[test]
    fn push_and_drain_roundtrip() {
        let s = DlqStaging::default();
        s.push(entry(10));
        s.push(entry(20));
        assert_eq!(s.len(), 2);
        let drained = s.drain();
        assert_eq!(drained.len(), 2);
        assert!(s.is_empty());
        assert_eq!(s.overflowed(), 0);
    }

    #[test]
    fn entry_count_bound_rejects_newest() {
        let s = DlqStaging::new(2, 1_000_000);
        s.push(entry(1));
        s.push(entry(1));
        s.push(entry(1)); // over the 2-entry bound
        assert_eq!(s.len(), 2, "third entry rejected");
        assert_eq!(s.overflowed(), 1);
    }

    #[test]
    fn byte_bound_rejects_oversized() {
        let s = DlqStaging::new(1000, 100);
        s.push(entry(60));
        s.push(entry(60)); // 120 > 100 byte bound
        assert_eq!(s.len(), 1, "second entry over byte bound rejected");
        assert_eq!(s.overflowed(), 1);
        // After drain the byte counter resets, so a new entry fits.
        let _ = s.drain();
        s.push(entry(60));
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn drain_resets_byte_bound() {
        let s = DlqStaging::new(1000, 100);
        s.push(entry(90));
        assert_eq!(s.drain().len(), 1);
        // Byte counter reset -> another 90 fits.
        s.push(entry(90));
        assert_eq!(s.len(), 1);
        assert_eq!(s.overflowed(), 0);
    }
}
