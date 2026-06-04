// Project:   hyperi-rustlib
// File:      src/transport/work_batch.rs
// Purpose:   Canonical data-plane contract: Record + WorkBatch
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # Canonical data-plane contract
//!
//! The single currency that flows through `get -> process -> send -> commit`:
//!
//! - [`Record`] -- one work record (payload + routing + lean metadata). It
//!   carries **no** commit token. Tokens live on the batch, not the record;
//!   that separation is what makes 1->N fan-out safe (a transform can grow or
//!   shrink the record count without disturbing the source acks).
//! - [`RecordMeta`] -- lean per-record metadata (timestamp + payload format).
//!   Deliberately leaner than the engine's `MessageMetadata`, which carries a
//!   type-erased commit token; here tokens move to the batch.
//! - [`WorkBatch`] -- the canonical zero-copy block of records, the source
//!   commit tokens for the whole block, and any inline-DLQ entries carried
//!   forward (the no-silent-drop contract preserved from the older
//!   `RecvBatch`).
//!
//! `commit_tokens.len()` is NOT tied to `records.len()`. After a fan-out
//! transform the record count may change while the commit tokens stay equal to
//! the input source acks -- they are fired only after the WHOLE block is sent
//! (at-least-once delivery).

use super::filter::FilteredDlqEntry;
use super::traits::CommitToken;
use super::types::PayloadFormat;
use bytes::Bytes;
use std::sync::Arc;

/// Lean per-record metadata.
///
/// Unlike the engine's `MessageMetadata`, this carries **no** commit token --
/// tokens live on the [`WorkBatch`], not the record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordMeta {
    /// Source timestamp (milliseconds since epoch), if the transport provided one.
    pub timestamp_ms: Option<i64>,

    /// Detected or declared payload format.
    pub format: PayloadFormat,
}

/// One work record: payload + routing + metadata, with **no** commit token.
///
/// The payload is held as [`bytes::Bytes`] so cloning a record (or fanning one
/// record out into many) is a refcount bump, not a deep copy.
#[derive(Debug, Clone)]
pub struct Record {
    /// Raw payload bytes -- zero-copy / refcounted.
    pub payload: Bytes,

    /// Routing key (Kafka topic, gRPC metadata key, Redis stream, ...).
    pub key: Option<Arc<str>>,

    /// Transport / application headers.
    pub headers: Vec<(String, Vec<u8>)>,

    /// Lean per-record metadata.
    pub metadata: RecordMeta,
}

/// The canonical zero-copy block of work records.
///
/// One `WorkBatch` is the single currency through `get -> process -> send ->
/// commit`. The `commit_tokens` are the INPUT source acks -- fired once after
/// the WHOLE block is sent (at-least-once). `T` generalises the source ack
/// (Kafka offset, HTTP responder, fetch cursor, ...).
///
/// `commit_tokens.len()` is intentionally decoupled from `records.len()`: a
/// fan-out transform may grow or shrink the record count while the commit
/// tokens stay equal to the input acks.
#[derive(Debug)]
pub struct WorkBatch<T: CommitToken> {
    /// The work records in this block.
    pub records: Vec<Record>,

    /// Source acks for the whole block. Fired after the block is fully sent.
    /// Length is NOT tied to `records.len()`.
    pub commit_tokens: Vec<T>,

    /// Inline-DLQ entries carried forward (no-silent-drop contract).
    pub dlq_entries: Vec<FilteredDlqEntry>,
}

impl<T: CommitToken> WorkBatch<T> {
    /// An empty batch (no records, no tokens, no DLQ entries).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            records: Vec::new(),
            commit_tokens: Vec::new(),
            dlq_entries: Vec::new(),
        }
    }

    /// A batch of records with no commit tokens and no DLQ entries.
    ///
    /// Useful for records that were generated downstream (e.g. a transform that
    /// emits new records) and do not themselves carry a source ack.
    #[must_use]
    pub fn from_records(records: Vec<Record>) -> Self {
        Self {
            records,
            commit_tokens: Vec::new(),
            dlq_entries: Vec::new(),
        }
    }

    /// A batch of records plus their source commit tokens (no DLQ entries).
    #[must_use]
    pub fn new(records: Vec<Record>, commit_tokens: Vec<T>) -> Self {
        Self {
            records,
            commit_tokens,
            dlq_entries: Vec::new(),
        }
    }

    /// Attach inline-DLQ entries to this batch, consuming and returning it.
    #[must_use]
    pub fn with_dlq_entries(mut self, dlq_entries: Vec<FilteredDlqEntry>) -> Self {
        self.dlq_entries = dlq_entries;
        self
    }

    /// Whether the batch has no records (DLQ entries may still be present).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Number of records.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Number of records (explicit alias for [`WorkBatch::len`]).
    #[must_use]
    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    /// Total payload bytes across all records.
    ///
    /// Needed by the byte-budget governor to size a block against a memory
    /// budget.
    #[must_use]
    pub fn total_payload_bytes(&self) -> usize {
        self.records.iter().map(|r| r.payload.len()).sum()
    }

    /// Transform the records while PRESERVING the commit tokens and DLQ entries.
    ///
    /// This is the safe shape for the future `process` handler: a transform may
    /// grow, shrink, or rewrite the records (fan-out / fan-in), but the source
    /// acks and inline-DLQ entries flow through untouched.
    #[must_use]
    pub fn map_records(mut self, f: impl FnOnce(Vec<Record>) -> Vec<Record>) -> Self {
        self.records = f(self.records);
        self
    }
}

impl<T: CommitToken> From<crate::Message<T>> for WorkBatch<T> {
    /// Collapse a single [`crate::Message`] into a one-record `WorkBatch`.
    ///
    /// The message's `payload` (`Vec<u8>`) becomes the record payload as
    /// [`Bytes`] and its `token` becomes the single commit token.
    fn from(msg: crate::Message<T>) -> Self {
        let record = Record {
            payload: Bytes::from(msg.payload),
            key: msg.key,
            headers: Vec::new(),
            metadata: RecordMeta {
                timestamp_ms: msg.timestamp_ms,
                format: msg.format,
            },
        };
        Self {
            records: vec![record],
            commit_tokens: vec![msg.token],
            dlq_entries: Vec::new(),
        }
    }
}

impl<T: CommitToken> From<crate::transport::traits::RecvBatch<T>> for WorkBatch<T> {
    /// Collapse a [`RecvBatch`](crate::transport::traits::RecvBatch) into a
    /// `WorkBatch`.
    ///
    /// Each `Message<T>` becomes a [`Record`]; its `token` is collected into
    /// `commit_tokens` (preserving order); `dlq_entries` carry straight across.
    fn from(batch: crate::transport::traits::RecvBatch<T>) -> Self {
        let mut records = Vec::with_capacity(batch.messages.len());
        let mut commit_tokens = Vec::with_capacity(batch.messages.len());
        for msg in batch.messages {
            commit_tokens.push(msg.token);
            records.push(Record {
                payload: Bytes::from(msg.payload),
                key: msg.key,
                headers: Vec::new(),
                metadata: RecordMeta {
                    timestamp_ms: msg.timestamp_ms,
                    format: msg.format,
                },
            });
        }
        Self {
            records,
            commit_tokens,
            dlq_entries: batch.dlq_entries,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use crate::transport::traits::RecvBatch;

    /// Minimal commit token for tests.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestToken(u64);

    impl std::fmt::Display for TestToken {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "tok-{}", self.0)
        }
    }

    impl CommitToken for TestToken {}

    fn record(payload: &'static [u8]) -> Record {
        Record {
            payload: Bytes::from_static(payload),
            key: Some(Arc::from("events")),
            headers: vec![("h".to_string(), b"v".to_vec())],
            metadata: RecordMeta {
                timestamp_ms: Some(42),
                format: PayloadFormat::Json,
            },
        }
    }

    #[test]
    fn empty_has_no_records_tokens_or_dlq() {
        let b = WorkBatch::<TestToken>::empty();
        assert!(b.is_empty());
        assert_eq!(b.len(), 0);
        assert_eq!(b.record_count(), 0);
        assert!(b.commit_tokens.is_empty());
        assert!(b.dlq_entries.is_empty());
        assert_eq!(b.total_payload_bytes(), 0);
    }

    #[test]
    fn from_records_has_no_tokens() {
        let b = WorkBatch::<TestToken>::from_records(vec![record(b"{}"), record(b"[]")]);
        assert_eq!(b.len(), 2);
        assert!(!b.is_empty());
        assert!(b.commit_tokens.is_empty());
    }

    #[test]
    fn new_carries_records_and_tokens() {
        let b = WorkBatch::new(vec![record(b"{}")], vec![TestToken(1), TestToken(2)]);
        assert_eq!(b.record_count(), 1);
        assert_eq!(b.commit_tokens.len(), 2);
    }

    #[test]
    fn with_dlq_entries_attaches_entries() {
        let entry = FilteredDlqEntry {
            payload: b"bad".to_vec(),
            key: None,
            reason: "filter".to_string(),
        };
        let b = WorkBatch::<TestToken>::from_records(vec![record(b"{}")])
            .with_dlq_entries(vec![entry]);
        assert_eq!(b.dlq_entries.len(), 1);
        assert_eq!(b.dlq_entries[0].reason, "filter");
    }

    #[test]
    fn total_payload_bytes_sums_payloads() {
        let b = WorkBatch::<TestToken>::from_records(vec![
            record(b"abc"),    // 3
            record(b"de"),     // 2
            record(b"f"),      // 1
        ]);
        assert_eq!(b.total_payload_bytes(), 6);
    }

    #[test]
    fn map_records_preserves_tokens_and_dlq() {
        let entry = FilteredDlqEntry {
            payload: b"bad".to_vec(),
            key: None,
            reason: "filter".to_string(),
        };
        let b = WorkBatch::new(vec![record(b"{}")], vec![TestToken(7)])
            .with_dlq_entries(vec![entry]);

        let b = b.map_records(|recs| {
            // identity-ish transform that mutates payload but keeps count
            recs.into_iter()
                .map(|mut r| {
                    r.payload = Bytes::from_static(b"changed");
                    r
                })
                .collect()
        });

        assert_eq!(b.record_count(), 1);
        assert_eq!(b.records[0].payload.as_ref(), b"changed");
        // tokens + dlq preserved across the transform
        assert_eq!(b.commit_tokens, vec![TestToken(7)]);
        assert_eq!(b.dlq_entries.len(), 1);
    }

    #[test]
    fn map_records_fan_out_keeps_tokens_intact() {
        // One input record + one source ack...
        let b = WorkBatch::new(vec![record(b"{}")], vec![TestToken(99)]);
        assert_eq!(b.record_count(), 1);
        assert_eq!(b.commit_tokens.len(), 1);

        // ...fans out to three records, but the source ack must stay singular.
        let b = b.map_records(|recs| {
            let mut out = Vec::new();
            for r in recs {
                out.push(r.clone());
                out.push(r.clone());
                out.push(r);
            }
            out
        });

        assert_eq!(b.record_count(), 3);
        assert_eq!(b.commit_tokens, vec![TestToken(99)]);
    }

    #[test]
    fn from_message_yields_single_record_batch() {
        let msg = Message::new(Some(Arc::from("topic")), b"{\"a\":1}".to_vec(), TestToken(5), Some(11));
        let b: WorkBatch<TestToken> = msg.into();

        assert_eq!(b.record_count(), 1);
        assert_eq!(b.commit_tokens, vec![TestToken(5)]);
        assert_eq!(b.records[0].payload.as_ref(), b"{\"a\":1}");
        assert_eq!(b.records[0].key.as_deref(), Some("topic"));
        assert_eq!(b.records[0].metadata.timestamp_ms, Some(11));
        assert_eq!(b.records[0].metadata.format, PayloadFormat::Json);
        assert!(b.dlq_entries.is_empty());
    }

    #[test]
    fn from_recv_batch_collapses_and_preserves_order() {
        let entry = FilteredDlqEntry {
            payload: b"bad".to_vec(),
            key: None,
            reason: "drop-it".to_string(),
        };
        let recv = RecvBatch {
            messages: vec![
                Message::new(Some(Arc::from("a")), b"{}".to_vec(), TestToken(1), None),
                Message::new(Some(Arc::from("b")), b"[]".to_vec(), TestToken(2), None),
                Message::new(None, b"{}".to_vec(), TestToken(3), None),
            ],
            dlq_entries: vec![entry],
        };

        let b: WorkBatch<TestToken> = recv.into();

        assert_eq!(b.record_count(), 3);
        // commit tokens preserved in order
        assert_eq!(b.commit_tokens, vec![TestToken(1), TestToken(2), TestToken(3)]);
        // dlq entries carried straight across
        assert_eq!(b.dlq_entries.len(), 1);
        assert_eq!(b.dlq_entries[0].reason, "drop-it");
        // record payloads + keys preserved
        assert_eq!(b.records[0].key.as_deref(), Some("a"));
        assert_eq!(b.records[2].key, None);
    }

    #[test]
    fn payload_is_bytes_and_clone_is_zero_copy() {
        // Bytes::clone shares the same underlying allocation (refcount bump).
        let r = record(b"shared-buffer");
        let p1 = r.payload.clone();
        let r2 = r.clone();
        // same backing buffer pointer => zero-copy clone
        assert_eq!(p1.as_ptr(), r2.payload.as_ptr());
        assert_eq!(r2.payload.as_ref(), b"shared-buffer");
    }
}
