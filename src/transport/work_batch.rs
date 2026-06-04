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
//!
//! The block flows `get -> process -> send -> commit` through the unified
//! engine driver (`crate::worker::engine::driver`); fields are parsed on
//! demand via the [`codec`](super::codec). For the wider picture see
//! `docs/BACKPRESSURE.md` (commit-token + brake contract),
//! `docs/SELF-REGULATION.md`, and `docs/MIGRATIONS.md` (the
//! `Message`/`RawMessage`/`RecvBatch` -> `WorkBatch` collapse).

use super::filter::FilteredDlqEntry;
use super::traits::CommitToken;
use super::types::PayloadFormat;
use bytes::Bytes;
use std::sync::Arc;
use thiserror::Error;

/// Failure modes for [`WorkBatch::from_json_array`] framing.
///
/// These cover only the **framing** contract -- slicing a top-level JSON array
/// into per-element byte views. Element values are NOT parsed, so a malformed
/// element body (e.g. `[1,nul]`) is not detected here; that is the downstream
/// parser's job. What IS detected: a missing opening `[`, an unterminated array
/// or string, structural imbalance, empty elements (leading/trailing/double
/// commas), and trailing garbage after the closing `]`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FramingError {
    /// The blob did not start (after optional whitespace) with `[`.
    #[error("json-array framing: expected opening '[', found {0}")]
    NotAnArray(String),

    /// The array (or a string within it) was not terminated before end-of-input.
    #[error("json-array framing: unexpected end of input (unterminated array or string)")]
    UnexpectedEof,

    /// An element position held no value (leading, trailing, or doubled comma).
    #[error("json-array framing: empty element at byte offset {0} (stray comma)")]
    EmptyElement(usize),

    /// Structure closed more containers than it opened.
    #[error("json-array framing: unbalanced closing bracket at byte offset {0}")]
    Unbalanced(usize),

    /// Non-whitespace content appeared after the array's closing `]`.
    #[error("json-array framing: trailing garbage after closing ']' at byte offset {0}")]
    TrailingGarbage(usize),
}

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
#[derive(Debug, Clone, PartialEq)]
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
    /// budget. -- Sums with saturating addition, so a pathological block can
    /// never overflow `usize`; it clamps at `usize::MAX` instead of wrapping.
    #[must_use]
    pub fn total_payload_bytes(&self) -> usize {
        self.records
            .iter()
            .map(|r| r.payload.len())
            .fold(0usize, usize::saturating_add)
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

    // ---- Zero-copy ingestion / framing helpers (Task 0.2) -----------------
    //
    // These slice ONE inbound `Bytes` blob into per-record views so the WHOLE
    // batch shares ONE allocation: `record.payload = blob.slice(start..end)` is
    // a refcounted view, never a payload copy. They do NOT parse / deserialise
    // and do NOT re-serialise -- framing only. Commit tokens and DLQ entries are
    // left EMPTY; the source attaches its acks afterwards.

    /// One record holding the WHOLE blob (no framing).
    ///
    /// The single record's payload is the blob itself (zero-copy move of the
    /// `Bytes` handle), `key` is `None`, `headers` empty, and the metadata
    /// format is auto-detected via [`PayloadFormat::detect`].
    #[must_use]
    pub fn single(blob: Bytes) -> Self {
        let format = PayloadFormat::detect(&blob);
        let record = Record {
            payload: blob,
            key: None,
            headers: Vec::new(),
            metadata: RecordMeta {
                timestamp_ms: None,
                format,
            },
        };
        Self {
            records: vec![record],
            commit_tokens: Vec::new(),
            dlq_entries: Vec::new(),
        }
    }

    /// Frame an NDJSON (newline-delimited JSON) blob into one record per line.
    ///
    /// Splits on `\n` using `memchr`. Each line is trimmed of a trailing `\r`
    /// (so Windows CRLF endings work) and of surrounding ASCII whitespace; blank
    /// or whitespace-only lines are skipped. Each surviving line becomes a
    /// [`Record`] whose payload is a zero-copy `blob.slice(start..end)` view.
    /// `format` is set to [`PayloadFormat::Json`].
    ///
    /// This is framing, not parsing -- it never fails. A line that is not valid
    /// JSON still becomes a record; the downstream parser surfaces that.
    #[must_use]
    pub fn from_ndjson(blob: Bytes) -> Self {
        let mut records = Vec::new();
        let mut line_start = 0usize;
        let bytes = blob.as_ref();

        // Walk newline boundaries; the tail after the last `\n` is its own line.
        for nl in memchr::memchr_iter(b'\n', bytes) {
            Self::push_ndjson_line(&mut records, &blob, line_start, nl);
            line_start = nl + 1;
        }
        if line_start < bytes.len() {
            Self::push_ndjson_line(&mut records, &blob, line_start, bytes.len());
        }

        Self {
            records,
            commit_tokens: Vec::new(),
            dlq_entries: Vec::new(),
        }
    }

    /// Trim one NDJSON line `[start, end)` (end is the `\n` index or EOF) and,
    /// if non-empty after trimming, push it as a zero-copy record.
    fn push_ndjson_line(records: &mut Vec<Record>, blob: &Bytes, start: usize, mut end: usize) {
        let bytes = blob.as_ref();
        // Strip a single trailing CR (CRLF) before generic whitespace trimming.
        if end > start && bytes[end - 1] == b'\r' {
            end -= 1;
        }
        // Trim surrounding ASCII whitespace so the slice is the bare record.
        while end > start && bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        let mut begin = start;
        while begin < end && bytes[begin].is_ascii_whitespace() {
            begin += 1;
        }
        if begin >= end {
            return; // blank / whitespace-only line
        }
        records.push(Record {
            payload: blob.slice(begin..end),
            key: None,
            headers: Vec::new(),
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Json,
            },
        });
    }

    /// Frame a top-level JSON array blob into one record per top-level element.
    ///
    /// Each element becomes a zero-copy `blob.slice(start..end)` view; element
    /// VALUES are not parsed. The byte-level scanner tracks string state (so
    /// `[`, `]`, `{`, `}` and `,` inside a string are literal), JSON escapes
    /// (`\"`, `\\`), and container depth, so nested objects/arrays frame
    /// correctly. An empty array `[]` yields zero records (`Ok`).
    ///
    /// # Errors
    ///
    /// Returns [`FramingError`] when the blob is not a well-framed top-level
    /// array: missing opening `[`, unterminated array/string, an empty element
    /// position (leading / trailing / doubled comma), an over-close, or trailing
    /// non-whitespace after the closing `]`.
    pub fn from_json_array(blob: Bytes) -> Result<Self, FramingError> {
        let records = scan_json_array(&blob)?;
        Ok(Self {
            records,
            commit_tokens: Vec::new(),
            dlq_entries: Vec::new(),
        })
    }
}

/// Byte-level top-level JSON-array element scanner (framing, NOT parsing).
///
/// Returns one zero-copy [`Record`] per top-level element. See
/// [`WorkBatch::from_json_array`] for the contract and error modes.
fn scan_json_array(blob: &Bytes) -> Result<Vec<Record>, FramingError> {
    let bytes = blob.as_ref();
    let len = bytes.len();

    // Skip leading whitespace; the first significant byte must be '['.
    let mut i = 0usize;
    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= len {
        return Err(FramingError::NotAnArray("end of input".to_string()));
    }
    if bytes[i] != b'[' {
        return Err(FramingError::NotAnArray(format!(
            "byte {:#04x} ('{}')",
            bytes[i], bytes[i] as char
        )));
    }
    i += 1; // consume '['

    let mut records: Vec<Record> = Vec::new();
    // `expect_value` distinguishes the slot just after '[' or ',' (where a value
    // or -- only after '[' -- a closing ']' may appear) from between elements.
    let mut first_element = true;

    loop {
        // Skip whitespace before an element (or the closing ']').
        while i < len && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= len {
            return Err(FramingError::UnexpectedEof);
        }

        if bytes[i] == b']' {
            // Closing the array. Only legal here if the array is empty (we are
            // still on the first element) -- otherwise we'd have consumed a ']'
            // inside the element-scan loop below after a value.
            if first_element {
                i += 1;
                return finish(blob, records, i);
            }
            // We reached here via the top of the loop after a ',', so a ']' now
            // means a trailing comma: `[1, ]`.
            return Err(FramingError::EmptyElement(i));
        }
        if bytes[i] == b',' {
            // A ',' where a value is expected: leading or doubled comma.
            return Err(FramingError::EmptyElement(i));
        }

        // Scan one element value, tracking string + escape + container depth.
        let elem_start = i;
        let mut depth: usize = 0;
        let mut in_string = false;
        let mut escaped = false;

        let elem_end;
        loop {
            if i >= len {
                return Err(FramingError::UnexpectedEof);
            }
            let c = bytes[i];

            if in_string {
                if escaped {
                    escaped = false;
                } else if c == b'\\' {
                    escaped = true;
                } else if c == b'"' {
                    in_string = false;
                }
                i += 1;
                continue;
            }

            match c {
                b'"' => {
                    in_string = true;
                    i += 1;
                }
                b'{' | b'[' => {
                    depth += 1;
                    i += 1;
                }
                b'}' => {
                    // A '}' at depth 0 has no matching opener inside this element.
                    depth = depth.checked_sub(1).ok_or(FramingError::Unbalanced(i))?;
                    i += 1;
                }
                b']' => {
                    if depth == 0 {
                        // This ']' closes the ARRAY -- the element ends here.
                        elem_end = i;
                        i += 1; // consume ']'
                        push_element(blob, &mut records, elem_start, elem_end);
                        return finish(blob, records, i);
                    }
                    depth -= 1;
                    i += 1;
                }
                b',' if depth == 0 => {
                    // Top-level separator -- the element ends just before it.
                    elem_end = i;
                    i += 1; // consume ','
                    break;
                }
                _ => {
                    i += 1;
                }
            }
        }

        push_element(blob, &mut records, elem_start, elem_end);
        first_element = false;
    }
}

/// Trim trailing whitespace from `[start, end)` and push the zero-copy slice.
///
/// Leading whitespace was already skipped by the caller, so only the trailing
/// edge (between the value and the `,`/`]`) needs trimming.
fn push_element(blob: &Bytes, records: &mut Vec<Record>, start: usize, end: usize) {
    let bytes = blob.as_ref();
    let mut e = end;
    while e > start && bytes[e - 1].is_ascii_whitespace() {
        e -= 1;
    }
    records.push(Record {
        payload: blob.slice(start..e),
        key: None,
        headers: Vec::new(),
        metadata: RecordMeta {
            timestamp_ms: None,
            format: PayloadFormat::Json,
        },
    });
}

/// After the closing `]`, only trailing whitespace is permitted. Build the
/// records vector into a `Vec<Record>` result, or error on trailing garbage.
fn finish(blob: &Bytes, records: Vec<Record>, mut i: usize) -> Result<Vec<Record>, FramingError> {
    let bytes = blob.as_ref();
    let len = bytes.len();
    while i < len && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i < len {
        return Err(FramingError::TrailingGarbage(i));
    }
    Ok(records)
}

impl<T: CommitToken> From<crate::Message<T>> for WorkBatch<T> {
    /// Collapse a single [`crate::Message`] into a one-record `WorkBatch`.
    ///
    /// The message payload is already [`Bytes`] -- this is a move, not a copy.
    fn from(msg: crate::Message<T>) -> Self {
        let record = Record {
            payload: msg.payload,
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
    /// Payloads are already [`Bytes`] -- each is a move, not a copy.
    fn from(batch: crate::transport::traits::RecvBatch<T>) -> Self {
        let mut records = Vec::with_capacity(batch.messages.len());
        let mut commit_tokens = Vec::with_capacity(batch.messages.len());
        for msg in batch.messages {
            commit_tokens.push(msg.token);
            records.push(Record {
                payload: msg.payload,
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
        let b =
            WorkBatch::<TestToken>::from_records(vec![record(b"{}")]).with_dlq_entries(vec![entry]);
        assert_eq!(b.dlq_entries.len(), 1);
        assert_eq!(b.dlq_entries[0].reason, "filter");
    }

    #[test]
    fn total_payload_bytes_sums_payloads() {
        let b = WorkBatch::<TestToken>::from_records(vec![
            record(b"abc"), // 3
            record(b"de"),  // 2
            record(b"f"),   // 1
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
        let b =
            WorkBatch::new(vec![record(b"{}")], vec![TestToken(7)]).with_dlq_entries(vec![entry]);

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
        let msg = Message::new(
            Some(Arc::from("topic")),
            b"{\"a\":1}".to_vec(),
            TestToken(5),
            Some(11),
        );
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
        assert_eq!(
            b.commit_tokens,
            vec![TestToken(1), TestToken(2), TestToken(3)]
        );
        // dlq entries carried straight across
        assert_eq!(b.dlq_entries.len(), 1);
        assert_eq!(b.dlq_entries[0].reason, "drop-it");
        // record payloads + keys preserved
        assert_eq!(b.records[0].key.as_deref(), Some("a"));
        assert_eq!(b.records[2].key, None);
    }

    #[test]
    fn from_message_moves_payload_without_copying() {
        // Build a heap-backed payload and remember its allocation pointer.
        // The vec is moved via `impl Into<Bytes>` -- no copy at Message::new.
        let payload = b"zero-copy-please".to_vec();
        let payload_ptr = payload.as_ptr();

        let msg = Message::new(Some(Arc::from("topic")), payload, TestToken(1), None);
        // Message.payload is Bytes built from the vec; same allocation.
        assert_eq!(msg.payload.as_ptr(), payload_ptr);

        let wb: WorkBatch<TestToken> = msg.into();

        // From<Message> moves the Bytes handle -- the record payload must point
        // at the SAME allocation. A regression to copy_from_slice would fail here.
        assert_eq!(wb.records[0].payload.as_ptr(), payload_ptr);
    }

    /// Task 0.4.1 capability test: `Message::new` with an already-allocated
    /// `Bytes` payload travels through `WorkBatch` with ZERO copies.
    ///
    /// This proves the headline win of the migration: an upstream `Bytes` (e.g.
    /// the axum body) reaches the `Record` without ever calling
    /// `copy_from_slice`. The allocation pointer must be identical at every hop.
    #[test]
    fn bytes_payload_travels_zero_copy_through_workbatch() {
        // Simulate a caller that already holds a `Bytes` (e.g. the axum ingest
        // handler after the body.to_vec() removal).
        let raw = b"bytes-zero-copy-payload-test".to_vec();
        let src: Bytes = raw.into();
        let src_ptr = src.as_ptr();

        // Pass the Bytes directly into Message::new (accepted via impl Into<Bytes>).
        let msg = Message::new(Some(Arc::from("k")), src, TestToken(42), Some(99));
        // The Bytes handle is MOVED -- same backing allocation.
        assert_eq!(msg.payload.as_ptr(), src_ptr, "copy at Message::new");

        // Convert to WorkBatch: From<Message> moves msg.payload (already Bytes).
        let wb: WorkBatch<TestToken> = msg.into();
        assert_eq!(
            wb.records[0].payload.as_ptr(),
            src_ptr,
            "copy at From<Message> for WorkBatch"
        );

        // Clone the record payload (fan-out): Bytes::clone is a refcount bump.
        let cloned = wb.records[0].payload.clone();
        assert_eq!(
            cloned.as_ptr(),
            src_ptr,
            "clone allocated a new buffer instead of bumping refcount"
        );
    }

    #[test]
    fn from_recv_batch_moves_payloads_without_copying() {
        let p0 = b"first-buffer".to_vec();
        let p1 = b"second-buffer".to_vec();
        let p0_ptr = p0.as_ptr();
        let p1_ptr = p1.as_ptr();

        let recv = RecvBatch {
            messages: vec![
                Message::new(Some(Arc::from("a")), p0, TestToken(1), None),
                Message::new(Some(Arc::from("b")), p1, TestToken(2), None),
            ],
            dlq_entries: Vec::new(),
        };

        let wb: WorkBatch<TestToken> = recv.into();

        // Each payload Vec is MOVED into Bytes -- same allocations, no copy.
        assert_eq!(wb.records[0].payload.as_ptr(), p0_ptr);
        assert_eq!(wb.records[1].payload.as_ptr(), p1_ptr);
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

    // ---- Task 0.2: zero-copy framing helpers -------------------------------

    /// Assert that `slice` is a zero-copy view INTO `blob` (a refcounted slice,
    /// not a fresh allocation): its byte range must fall within `blob`'s range.
    fn assert_within(slice: &Bytes, blob: &Bytes) {
        let blob_start = blob.as_ptr() as usize;
        let blob_end = blob_start + blob.len();
        let slice_start = slice.as_ptr() as usize;
        let slice_end = slice_start + slice.len();
        assert!(
            slice_start >= blob_start && slice_end <= blob_end,
            "slice [{slice_start:#x}, {slice_end:#x}) is not within blob \
             [{blob_start:#x}, {blob_end:#x}) -- it is a copy, not a view"
        );
    }

    // --- single() -----------------------------------------------------------

    #[test]
    fn single_holds_whole_blob_as_one_record() {
        let blob = Bytes::from_static(b"{\"a\":1}");
        let b = WorkBatch::<TestToken>::single(blob.clone());
        assert_eq!(b.record_count(), 1);
        assert!(b.commit_tokens.is_empty());
        assert!(b.dlq_entries.is_empty());
        assert_eq!(b.records[0].payload, blob);
        assert_eq!(b.records[0].key, None);
        assert!(b.records[0].headers.is_empty());
        // whole blob is the same allocation (zero-copy)
        assert_eq!(b.records[0].payload.as_ptr(), blob.as_ptr());
    }

    #[test]
    fn single_detects_format_json_object() {
        let b = WorkBatch::<TestToken>::single(Bytes::from_static(b"{\"a\":1}"));
        assert_eq!(b.records[0].metadata.format, PayloadFormat::Json);
    }

    #[test]
    fn single_detects_format_msgpack() {
        // fixmap with one entry (0x81) -> MsgPack
        let b = WorkBatch::<TestToken>::single(Bytes::from_static(&[0x81, 0xa1, 0x61]));
        assert_eq!(b.records[0].metadata.format, PayloadFormat::MsgPack);
    }

    // --- from_ndjson() ------------------------------------------------------

    #[test]
    fn ndjson_splits_lines_into_records() {
        let blob = Bytes::from_static(b"{\"a\":1}\n{\"b\":2}\n{\"c\":3}");
        let b = WorkBatch::<TestToken>::from_ndjson(blob.clone());
        assert_eq!(b.record_count(), 3);
        assert_eq!(b.records[0].payload.as_ref(), b"{\"a\":1}");
        assert_eq!(b.records[1].payload.as_ref(), b"{\"b\":2}");
        assert_eq!(b.records[2].payload.as_ref(), b"{\"c\":3}");
        for r in &b.records {
            assert_eq!(r.metadata.format, PayloadFormat::Json);
            assert_within(&r.payload, &blob);
        }
        assert!(b.commit_tokens.is_empty());
    }

    #[test]
    fn ndjson_trims_trailing_carriage_return() {
        // Windows CRLF line endings.
        let blob = Bytes::from_static(b"{\"a\":1}\r\n{\"b\":2}\r\n");
        let b = WorkBatch::<TestToken>::from_ndjson(blob);
        assert_eq!(b.record_count(), 2);
        assert_eq!(b.records[0].payload.as_ref(), b"{\"a\":1}");
        assert_eq!(b.records[1].payload.as_ref(), b"{\"b\":2}");
    }

    #[test]
    fn ndjson_skips_blank_and_whitespace_only_lines() {
        let blob = Bytes::from_static(b"{\"a\":1}\n\n   \n{\"b\":2}\n\t\r\n");
        let b = WorkBatch::<TestToken>::from_ndjson(blob);
        assert_eq!(b.record_count(), 2);
        assert_eq!(b.records[0].payload.as_ref(), b"{\"a\":1}");
        assert_eq!(b.records[1].payload.as_ref(), b"{\"b\":2}");
    }

    #[test]
    fn ndjson_empty_blob_yields_no_records() {
        let b = WorkBatch::<TestToken>::from_ndjson(Bytes::new());
        assert_eq!(b.record_count(), 0);
    }

    #[test]
    fn ndjson_single_line_no_newline() {
        let blob = Bytes::from_static(b"{\"only\":true}");
        let b = WorkBatch::<TestToken>::from_ndjson(blob.clone());
        assert_eq!(b.record_count(), 1);
        assert_eq!(b.records[0].payload.as_ref(), b"{\"only\":true}");
        assert_within(&b.records[0].payload, &blob);
    }

    #[test]
    fn ndjson_preserves_inner_whitespace_but_trims_edges() {
        // Leading/trailing spaces on a line are trimmed for framing.
        let blob = Bytes::from_static(b"  {\"a\": 1}  \n");
        let b = WorkBatch::<TestToken>::from_ndjson(blob);
        assert_eq!(b.record_count(), 1);
        assert_eq!(b.records[0].payload.as_ref(), b"{\"a\": 1}");
    }

    // --- from_json_array() --------------------------------------------------

    #[test]
    fn json_array_empty_yields_no_records() {
        let b = WorkBatch::<TestToken>::from_json_array(Bytes::from_static(b"[]")).unwrap();
        assert_eq!(b.record_count(), 0);
        assert!(b.commit_tokens.is_empty());
        assert!(b.dlq_entries.is_empty());
    }

    #[test]
    fn json_array_empty_with_whitespace() {
        let b = WorkBatch::<TestToken>::from_json_array(Bytes::from_static(b"  [  ]  ")).unwrap();
        assert_eq!(b.record_count(), 0);
    }

    #[test]
    fn json_array_single_element() {
        let blob = Bytes::from_static(b"[{\"a\":1}]");
        let b = WorkBatch::<TestToken>::from_json_array(blob.clone()).unwrap();
        assert_eq!(b.record_count(), 1);
        assert_eq!(b.records[0].payload.as_ref(), b"{\"a\":1}");
        assert_eq!(b.records[0].metadata.format, PayloadFormat::Json);
        assert_within(&b.records[0].payload, &blob);
    }

    #[test]
    fn json_array_multiple_scalar_elements() {
        let blob = Bytes::from_static(b"[1, 2, 3]");
        let b = WorkBatch::<TestToken>::from_json_array(blob).unwrap();
        assert_eq!(b.record_count(), 3);
        assert_eq!(b.records[0].payload.as_ref(), b"1");
        assert_eq!(b.records[1].payload.as_ref(), b"2");
        assert_eq!(b.records[2].payload.as_ref(), b"3");
    }

    #[test]
    fn json_array_trims_whitespace_around_elements() {
        let blob = Bytes::from_static(b"[  {\"a\":1}  ,  {\"b\":2}  ]");
        let b = WorkBatch::<TestToken>::from_json_array(blob).unwrap();
        assert_eq!(b.record_count(), 2);
        assert_eq!(b.records[0].payload.as_ref(), b"{\"a\":1}");
        assert_eq!(b.records[1].payload.as_ref(), b"{\"b\":2}");
    }

    #[test]
    fn json_array_leading_trailing_whitespace_and_newlines() {
        let blob = Bytes::from_static(b"\n\t [\n  1,\n  2\n] \n");
        let b = WorkBatch::<TestToken>::from_json_array(blob).unwrap();
        assert_eq!(b.record_count(), 2);
        assert_eq!(b.records[0].payload.as_ref(), b"1");
        assert_eq!(b.records[1].payload.as_ref(), b"2");
    }

    #[test]
    fn json_array_string_with_brackets_and_commas() {
        // The commas/brackets INSIDE the string must not split or change depth.
        let blob = Bytes::from_static(b"[\"a,b[c]{d}\", \"plain\"]");
        let b = WorkBatch::<TestToken>::from_json_array(blob).unwrap();
        assert_eq!(b.record_count(), 2);
        assert_eq!(b.records[0].payload.as_ref(), b"\"a,b[c]{d}\"");
        assert_eq!(b.records[1].payload.as_ref(), b"\"plain\"");
    }

    #[test]
    fn json_array_string_with_escaped_quote() {
        // `\"` does not terminate the string.
        let blob = Bytes::from_static(b"[\"he said \\\"hi\\\", then left\", 7]");
        let b = WorkBatch::<TestToken>::from_json_array(blob).unwrap();
        assert_eq!(b.record_count(), 2);
        assert_eq!(
            b.records[0].payload.as_ref(),
            b"\"he said \\\"hi\\\", then left\""
        );
        assert_eq!(b.records[1].payload.as_ref(), b"7");
    }

    #[test]
    fn json_array_string_with_escaped_backslash_then_closing_quote() {
        // `\\` is an escaped backslash; the following `"` DOES close the string.
        // Element 0 is the string "path\\" and element 1 is the number 1.
        let blob = Bytes::from_static(b"[\"path\\\\\", 1]");
        let b = WorkBatch::<TestToken>::from_json_array(blob).unwrap();
        assert_eq!(b.record_count(), 2);
        assert_eq!(b.records[0].payload.as_ref(), b"\"path\\\\\"");
        assert_eq!(b.records[1].payload.as_ref(), b"1");
    }

    #[test]
    fn json_array_nested_arrays_and_objects() {
        let blob = Bytes::from_static(b"[[1,2],[3],{\"k\":[4,5]}]");
        let b = WorkBatch::<TestToken>::from_json_array(blob).unwrap();
        assert_eq!(b.record_count(), 3);
        assert_eq!(b.records[0].payload.as_ref(), b"[1,2]");
        assert_eq!(b.records[1].payload.as_ref(), b"[3]");
        assert_eq!(b.records[2].payload.as_ref(), b"{\"k\":[4,5]}");
    }

    #[test]
    fn json_array_deeply_nested_object_one_element() {
        let blob = Bytes::from_static(b"[{\"a\":{\"b\":{\"c\":[1,{\"d\":2}]}}}]");
        let b = WorkBatch::<TestToken>::from_json_array(blob).unwrap();
        assert_eq!(b.record_count(), 1);
        assert_eq!(
            b.records[0].payload.as_ref(),
            b"{\"a\":{\"b\":{\"c\":[1,{\"d\":2}]}}}"
        );
    }

    #[test]
    fn json_array_unicode_in_strings() {
        // Multi-byte UTF-8 inside an element string must pass through verbatim.
        let blob = Bytes::from(r#"["café", "naïve"]"#.as_bytes().to_vec());
        let b = WorkBatch::<TestToken>::from_json_array(blob.clone()).unwrap();
        assert_eq!(b.record_count(), 2);
        assert_eq!(b.records[0].payload.as_ref(), "\"café\"".as_bytes());
        assert_eq!(b.records[1].payload.as_ref(), "\"naïve\"".as_bytes());
        assert_within(&b.records[1].payload, &blob);
    }

    #[test]
    fn json_array_zero_copy_views_into_blob() {
        let blob = Bytes::from_static(b"[{\"a\":1}, {\"b\":2}, {\"c\":3}]");
        let b = WorkBatch::<TestToken>::from_json_array(blob.clone()).unwrap();
        assert_eq!(b.record_count(), 3);
        for r in &b.records {
            assert_within(&r.payload, &blob);
        }
    }

    // malformed inputs -> Err

    #[test]
    fn json_array_no_opening_bracket_errors() {
        assert!(WorkBatch::<TestToken>::from_json_array(Bytes::from_static(b"{\"a\":1}")).is_err());
    }

    #[test]
    fn json_array_empty_blob_errors() {
        assert!(WorkBatch::<TestToken>::from_json_array(Bytes::new()).is_err());
    }

    #[test]
    fn json_array_whitespace_only_errors() {
        assert!(WorkBatch::<TestToken>::from_json_array(Bytes::from_static(b"   \n\t ")).is_err());
    }

    #[test]
    fn json_array_unterminated_errors() {
        assert!(WorkBatch::<TestToken>::from_json_array(Bytes::from_static(b"[1, 2")).is_err());
    }

    #[test]
    fn json_array_unterminated_string_errors() {
        assert!(
            WorkBatch::<TestToken>::from_json_array(Bytes::from_static(b"[\"unclosed]")).is_err()
        );
    }

    #[test]
    fn json_array_trailing_garbage_errors() {
        assert!(
            WorkBatch::<TestToken>::from_json_array(Bytes::from_static(b"[1, 2] junk")).is_err()
        );
    }

    #[test]
    fn json_array_trailing_comma_errors() {
        assert!(WorkBatch::<TestToken>::from_json_array(Bytes::from_static(b"[1, 2, ]")).is_err());
    }

    #[test]
    fn json_array_leading_comma_errors() {
        assert!(WorkBatch::<TestToken>::from_json_array(Bytes::from_static(b"[, 1]")).is_err());
    }

    #[test]
    fn json_array_double_comma_errors() {
        assert!(WorkBatch::<TestToken>::from_json_array(Bytes::from_static(b"[1,, 2]")).is_err());
    }

    #[test]
    fn json_array_only_open_bracket_errors() {
        assert!(WorkBatch::<TestToken>::from_json_array(Bytes::from_static(b"[")).is_err());
    }

    #[test]
    fn json_array_unbalanced_extra_close_errors() {
        // A nested structure that closes too many times.
        assert!(WorkBatch::<TestToken>::from_json_array(Bytes::from_static(b"[1]]")).is_err());
    }

    #[test]
    fn framing_error_is_displayable() {
        let err = WorkBatch::<TestToken>::from_json_array(Bytes::from_static(b"nope")).unwrap_err();
        // thiserror Display should produce a non-empty, informative message.
        assert!(!err.to_string().is_empty());
    }
}
