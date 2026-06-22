// Project:   hyperi-rustlib
// File:      src/worker/engine/driver_tests.rs
// Purpose:   Tests for the WorkBatch engine run-loop driver
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Driver run-loop tests, split out of driver.rs to keep that file focused
//! on the run paths. A `#[path]` submodule of `driver`, so `super` resolves
//! to the driver module's items.

use super::*;
use crate::transport::memory::{MemoryConfig, MemoryTransport};
use crate::transport::{CommitToken, PayloadFormat, RecordMeta};
use crate::worker::engine::BatchProcessingConfig;
use bytes::Bytes;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

fn default_engine() -> BatchEngine {
    BatchEngine::new(BatchProcessingConfig::default())
}

fn mem_transport(timeout_ms: u64) -> MemoryTransport {
    MemoryTransport::new(&MemoryConfig {
        recv_timeout_ms: timeout_ms,
        ..Default::default()
    })
    .expect("memory transport with valid config must construct")
}

/// Cancel `shutdown` after `ms` to stop the run loop cleanly.
fn cancel_after(shutdown: CancellationToken, ms: u64) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        shutdown.cancel();
    });
}

/// Clone one record into `factor` copies (1->N fan-out).
fn fan_out(records: Vec<Record>, factor: usize) -> Vec<Record> {
    let mut out = Vec::with_capacity(records.len() * factor);
    for r in records {
        for _ in 0..factor {
            out.push(r.clone());
        }
    }
    out
}

/// THE proving test: N source records, each with a distinct ack; a process
/// that fans 1->2; assert all 2N records hit the sink AND commit acked
/// EXACTLY N source tokens (committed_sequence advanced by the source acks,
/// not the doubled output count).
#[tokio::test]
async fn fan_out_commits_source_tokens_not_output_count() {
    let n = 5usize;
    let transport = mem_transport(50);
    for i in 0..n {
        transport
            .inject(None, format!(r#"{{"id":{i}}}"#).into_bytes())
            .await
            .unwrap();
    }

    let engine = default_engine();
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 200);

    let sink_records = Arc::new(AtomicUsize::new(0));
    let sink_tokens = Arc::new(AtomicUsize::new(0));
    let sr = Arc::clone(&sink_records);
    let st = Arc::clone(&sink_tokens);

    engine
        .run_workbatch(
            &transport,
            shutdown,
            |batch| Ok(batch.map_records(|recs| fan_out(recs, 2))),
            |out: &WorkBatch<_>| {
                let sr = Arc::clone(&sr);
                let st = Arc::clone(&st);
                let records = out.records.len();
                let tokens = out.commit_tokens.len();
                async move {
                    sr.fetch_add(records, Ordering::Relaxed);
                    st.fetch_add(tokens, Ordering::Relaxed);
                    Ok(())
                }
            },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await
        .unwrap();

    // (a) all 2N records reached the sink.
    assert_eq!(
        sink_records.load(Ordering::Relaxed),
        2 * n,
        "all 2N records sunk"
    );
    // (b) the out-batch carried exactly N source tokens (fan-out did not
    // multiply the acks).
    assert_eq!(
        sink_tokens.load(Ordering::Relaxed),
        n,
        "N source tokens carried"
    );
    // (b cont.) commit acked exactly the N source tokens: MemoryToken seq is
    // 0..N, so committed_sequence (a fetch_max) lands on N-1.
    assert_eq!(
        transport.committed_sequence(),
        (n - 1) as u64,
        "commit advanced to the highest of the N source acks, not the 2N output count"
    );
}

/// On a sink error the commit must NOT fire (the block is re-delivered) AND
/// the run loop stops -- the sink error is a TERMINAL ack-barrier error.
#[tokio::test]
async fn sink_error_does_not_commit() {
    let transport = mem_transport(50);
    transport
        .inject(None, br#"{"id":1}"#.to_vec())
        .await
        .unwrap();

    let engine = default_engine();
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 200);

    let result = engine
        .run_workbatch(
            &transport,
            shutdown,
            |batch| Ok(batch),
            |_out: &WorkBatch<_>| async { Err(EngineError::Sink("boom".into())) },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await;
    assert!(
        matches!(result, Err(EngineError::Sink(_))),
        "sink error is terminal: the run returns the sink error, got {result:?}"
    );

    // committed_sequence is a fetch_max seeded at 0 and the only injected
    // message had seq 0; a commit would still leave it at 0, so to PROVE the
    // commit did not fire we inject a higher-seq message that, if committed,
    // would advance the sequence past 0. Re-run with seq 1..=2.
    let transport = mem_transport(50);
    transport
        .inject(None, br#"{"a":1}"#.to_vec())
        .await
        .unwrap(); // seq 0
    transport
        .inject(None, br#"{"b":2}"#.to_vec())
        .await
        .unwrap(); // seq 1
    // drain seq 0 first so the failing block carries seq 1.
    let _ = transport.recv(1).await.unwrap();
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 200);
    let result = engine
        .run_workbatch(
            &transport,
            shutdown,
            |batch| Ok(batch),
            |_out: &WorkBatch<_>| async { Err(EngineError::Sink("boom".into())) },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await;
    assert!(result.is_err(), "sink error is terminal");
    assert_eq!(
        transport.committed_sequence(),
        0,
        "sink error must skip commit -- sequence stays at its initial 0"
    );
}

/// `CommitMode::Auto` commits after a successful sink.
#[tokio::test]
async fn auto_commits_after_sink_ok() {
    let transport = mem_transport(50);
    for i in 0..3u64 {
        transport
            .inject(None, format!(r#"{{"id":{i}}}"#).into_bytes())
            .await
            .unwrap();
    }

    let engine = default_engine();
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 200);

    engine
        .run_workbatch(
            &transport,
            shutdown,
            |batch| Ok(batch),
            |_out: &WorkBatch<_>| async { Ok(()) },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await
        .unwrap();

    // Three messages seq 0..=2 -> committed sequence is 2.
    assert_eq!(transport.committed_sequence(), 2);
}

/// `CommitMode::SinkManaged` leaves the commit to the sink -- the engine
/// does not commit.
#[tokio::test]
async fn sink_managed_does_not_commit_in_engine() {
    let transport = mem_transport(50);
    transport
        .inject(None, br#"{"a":1}"#.to_vec())
        .await
        .unwrap(); // seq 0
    transport
        .inject(None, br#"{"b":2}"#.to_vec())
        .await
        .unwrap(); // seq 1
    // Drain seq 0 so the block carries seq 1 -- a commit would push the
    // sequence past its initial 0.
    let _ = transport.recv(1).await.unwrap();

    let engine = default_engine();
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 200);

    engine
        .run_workbatch(
            &transport,
            shutdown,
            |batch| Ok(batch),
            // Sink does NOT commit here -- it could, but we prove the engine
            // does not commit on its behalf.
            |_out: &WorkBatch<_>| async { Ok(()) },
            CommitMode::SinkManaged,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await
        .unwrap();

    assert_eq!(
        transport.committed_sequence(),
        0,
        "SinkManaged: engine must not commit -- sequence stays at initial 0"
    );
}

/// The ticker fires on its interval; shutdown stops the loop cleanly.
#[tokio::test]
async fn ticker_fires_and_shutdown_stops_loop() {
    let transport = mem_transport(50);
    let engine = default_engine();
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 350);

    let ticks = Arc::new(AtomicU64::new(0));
    let tc = Arc::clone(&ticks);

    let result = engine
        .run_workbatch(
            &transport,
            shutdown,
            |batch| Ok(batch),
            |_out: &WorkBatch<_>| async { Ok(()) },
            CommitMode::Auto,
            Some((Duration::from_millis(100), move || {
                let tc = Arc::clone(&tc);
                async move {
                    tc.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                }
            })),
        )
        .await;

    assert!(result.is_ok(), "shutdown stops the loop cleanly");
    assert!(
        ticks.load(Ordering::Relaxed) >= 2,
        "ticker fired at least twice over 350ms at 100ms interval"
    );
}

/// On-demand path: a transform that calls codec::parse reads the right field
/// and can rewrite the payload, all without the driver pre-parsing.
#[tokio::test]
async fn on_demand_transform_reads_field_via_codec_parse() {
    let transport = mem_transport(50);
    transport
        .inject(None, br#"{"_table":"events","id":1}"#.to_vec())
        .await
        .unwrap();

    let engine = default_engine();
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 200);

    let seen_table = Arc::new(std::sync::Mutex::new(String::new()));
    let st = Arc::clone(&seen_table);

    engine
        .run_workbatch(
            &transport,
            shutdown,
            move |batch| {
                let st = Arc::clone(&st);
                Ok(batch.map_records(move |recs| {
                    recs.into_iter()
                        .inspect(|r| {
                            // Parse ON DEMAND inside the transform.
                            let parsed =
                                codec::parse(&r.payload, r.metadata.format).expect("valid json");
                            if let Some(t) = parsed.field_str("_table") {
                                *st.lock().unwrap() = t.to_string();
                            }
                        })
                        .collect()
                }))
            },
            |_out: &WorkBatch<_>| async { Ok(()) },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await
        .unwrap();

    assert_eq!(*seen_table.lock().unwrap(), "events");
}

/// Batch-parse path: the driver pre-parses; the process closure sees aligned
/// parsed payloads, the interner dedups field names, and the logical result
/// matches the on-demand path.
#[tokio::test]
async fn parsed_path_pre_parses_and_interner_dedups() {
    let transport = mem_transport(50);
    for i in 0..4 {
        transport
            .inject(
                None,
                format!(r#"{{"_table":"events","id":{i}}}"#).into_bytes(),
            )
            .await
            .unwrap();
    }

    let engine = default_engine();
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 200);

    let tables = Arc::new(AtomicUsize::new(0));
    let tc = Arc::clone(&tables);

    engine
        .run_workbatch_parsed(
            &transport,
            shutdown,
            move |pb: ParsedBatch<'_, _>| {
                // Records are aligned 1:1 with parsed payloads.
                assert_eq!(pb.records.len(), pb.parsed.len());
                // Intern the routing-field name once for the whole block.
                let field = pb.intern("_table");
                let mut hits = 0;
                for parsed in &pb.parsed {
                    if parsed.field_str(&field) == Some("events") {
                        hits += 1;
                    }
                }
                tc.fetch_add(hits, Ordering::Relaxed);
                // Re-assemble a WorkBatch preserving the source acks.
                Ok(WorkBatch::new(pb.records, pb.commit_tokens).with_dlq_entries(pb.dlq_entries))
            },
            |_out: &WorkBatch<_>| async { Ok(()) },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await
        .unwrap();

    assert_eq!(
        tables.load(Ordering::Relaxed),
        4,
        "all 4 records routed on _table"
    );
    assert_eq!(transport.committed_sequence(), 3, "all 4 acks committed");
}

/// Parsed path no-silent-drop (default `ParseErrorAction::Dlq`): an
/// unparseable record is routed to the out-batch DLQ entries, the process
/// closure sees them, AND they reach the DLQ route point (a `Route` policy
/// sink) before commit -- not dropped -- while source acks stay intact.
#[tokio::test]
async fn parsed_path_routes_parse_failures_to_dlq() {
    use crate::worker::engine::FilterDlqPolicy;
    let transport = mem_transport(50);
    transport
        .inject(None, br#"{"id":1}"#.to_vec())
        .await
        .unwrap(); // seq 0 ok
    transport
        .inject(None, b"not json {{{".to_vec())
        .await
        .unwrap(); // seq 1 bad
    transport
        .inject(None, br#"{"id":3}"#.to_vec())
        .await
        .unwrap(); // seq 2 ok

    // A Route policy captures the entries that reach the DLQ route point.
    let routed = Arc::new(AtomicUsize::new(0));
    let rc = Arc::clone(&routed);
    let engine = default_engine().with_filter_dlq_policy(FilterDlqPolicy::Route(Arc::new(
        move |entries: Vec<crate::transport::filter::FilteredDlqEntry>| {
            rc.fetch_add(entries.len(), Ordering::Relaxed);
            Ok(())
        },
    )));
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 200);

    let dlq_seen = Arc::new(AtomicUsize::new(0));
    let kept = Arc::new(AtomicUsize::new(0));
    let ds = Arc::clone(&dlq_seen);
    let kp = Arc::clone(&kept);

    engine
        .run_workbatch_parsed(
            &transport,
            shutdown,
            move |pb: ParsedBatch<'_, _>| {
                ds.fetch_add(pb.dlq_entries.len(), Ordering::Relaxed);
                kp.fetch_add(pb.records.len(), Ordering::Relaxed);
                Ok(WorkBatch::new(pb.records, pb.commit_tokens).with_dlq_entries(pb.dlq_entries))
            },
            |_out: &WorkBatch<_>| async { Ok(()) },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await
        .unwrap();

    assert_eq!(kept.load(Ordering::Relaxed), 2, "2 records parsed cleanly");
    assert_eq!(
        dlq_seen.load(Ordering::Relaxed),
        1,
        "1 parse failure carried to the process closure as a DLQ entry"
    );
    assert_eq!(
        routed.load(Ordering::Relaxed),
        1,
        "the parse-failure DLQ entry reached the DLQ route point before commit"
    );
    // All three source acks are still committed -- a parse failure does not
    // lose the source ack (at-least-once on the WHOLE block).
    assert_eq!(transport.committed_sequence(), 2);
}

/// Memory pressure / lease accounting on a WorkBatch.
#[cfg(feature = "memory")]
#[tokio::test]
async fn lease_ingress_batch_accounts_and_releases() {
    use crate::memory::{MemoryGuard, MemoryGuardConfig};

    let mut engine = default_engine();
    let guard = Arc::new(MemoryGuard::new(MemoryGuardConfig {
        limit_bytes: 1024 * 1024,
        ..Default::default()
    }));
    engine.set_memory_guard_for_test(Arc::clone(&guard));

    let payloads: Vec<Record> = (0..4)
        .map(|i| Record {
            payload: Bytes::from(format!(r#"{{"id":{i}}}"#)),
            key: None,
            headers: vec![],
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Json,
            },
        })
        .collect();
    let batch = WorkBatch::<MemTok>::from_records(payloads);
    let expected = batch.total_payload_bytes() as u64;

    assert_eq!(guard.current_bytes(), 0);
    {
        let _lease = engine.lease_ingress_batch(&batch).expect("guard present");
        assert_eq!(guard.current_bytes(), expected, "accounted while held");
    }
    assert_eq!(guard.current_bytes(), 0, "released on drop");
}

/// A minimal CommitToken for the memory-lease unit test (no transport recv).
#[cfg(feature = "memory")]
#[derive(Debug, Clone)]
struct MemTok;
#[cfg(feature = "memory")]
impl std::fmt::Display for MemTok {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("memtok")
    }
}
#[cfg(feature = "memory")]
impl CommitToken for MemTok {}

// ---- Remediation Phase 1: ordered-commit ack barrier -----------------
//
// Kafka (and MemoryTransport) commit is CUMULATIVE: `commit up to offset N`
// advances a watermark via fetch_max. So if a block carrying token 0 fails
// its sink/commit, a LATER block carrying token 1 must NEVER be committed --
// doing so advances the watermark past token 0's never-sent records, which
// silently skips them (data loss, at-least-once violated). These tests pin
// the ack barrier: the committed watermark never advances past the last
// successfully-sunk-and-committed block.

/// A real ORDERED receiver test double (real `Record`/`WorkBatch`/`MemoryToken`
/// types, no internal-code mock). It hands out ONE record per `recv` with
/// MONOTONIC tokens (seq 0, 1, 2, ...) and a CUMULATIVE commit -- the
/// committed watermark is `fetch_max` of the committed tokens, exactly like
/// a Kafka offset commit. This isolates the ordered-commit semantics from
/// MemoryTransport's channel batching (which would coalesce all pending
/// messages into a single block).
struct OrderedReceiver {
    /// Next seq to deliver; one record per recv until exhausted.
    next_seq: Arc<AtomicU64>,
    /// How many records to deliver before recv blocks (pending) forever.
    total: u64,
    /// Cumulative committed watermark (highest committed seq + 1, or 0 if
    /// nothing committed). `u64::MAX` sentinel means "no commit yet".
    committed_hwm: Arc<AtomicU64>,
    /// Count of commit calls (to prove a later block's commit did not fire).
    commit_calls: Arc<AtomicUsize>,
    /// If set, `commit` returns an error (broker commit failure) for any
    /// block whose highest token seq equals this value.
    fail_commit_on_seq: Option<u64>,
}

impl OrderedReceiver {
    fn new(total: u64) -> Self {
        Self {
            next_seq: Arc::new(AtomicU64::new(0)),
            total,
            committed_hwm: Arc::new(AtomicU64::new(u64::MAX)),
            commit_calls: Arc::new(AtomicUsize::new(0)),
            fail_commit_on_seq: None,
        }
    }
}

impl crate::transport::TransportBase for OrderedReceiver {
    fn close(
        &self,
    ) -> impl std::future::Future<Output = crate::transport::TransportResult<()>> + Send {
        std::future::ready(Ok(()))
    }
    fn is_healthy(&self) -> bool {
        true
    }
    fn name(&self) -> &'static str {
        "ordered-test"
    }
}

impl TransportReceiver for OrderedReceiver {
    type Token = crate::transport::memory::MemoryToken;

    fn recv(
        &self,
        _max: usize,
    ) -> impl std::future::Future<Output = crate::transport::TransportResult<WorkBatch<Self::Token>>>
    + Send {
        let next_seq = Arc::clone(&self.next_seq);
        let total = self.total;
        async move {
            let seq = next_seq.fetch_add(1, Ordering::Relaxed);
            if seq >= total {
                // Exhausted: block forever so the loop only exits on shutdown
                // (mirrors a quiet broker -- never an error/EOF).
                next_seq.fetch_sub(1, Ordering::Relaxed);
                std::future::pending::<()>().await;
            }
            let record = Record {
                payload: Bytes::from(format!(r#"{{"seq":{seq}}}"#)),
                key: None,
                headers: vec![],
                metadata: RecordMeta {
                    timestamp_ms: None,
                    format: PayloadFormat::Json,
                },
            };
            Ok(WorkBatch::new(
                vec![record],
                vec![crate::transport::memory::MemoryToken { seq }],
            ))
        }
    }

    async fn commit(&self, tokens: &[Self::Token]) -> crate::transport::TransportResult<()> {
        self.commit_calls.fetch_add(1, Ordering::Relaxed);
        let Some(max_seq) = tokens.iter().map(|t| t.seq).max() else {
            return Ok(());
        };
        if self.fail_commit_on_seq == Some(max_seq) {
            return Err(crate::transport::TransportError::Commit(format!(
                "broker commit failed for seq {max_seq}"
            )));
        }
        // Cumulative: watermark = max(current, this block's highest seq).
        self.committed_hwm.fetch_max(max_seq, Ordering::Relaxed);
        Ok(())
    }
}

/// THE ack-barrier bug test (sink failure). Token 0's block fails at the
/// sink; token 1's block would succeed. With an ORDERED/cumulative commit,
/// the engine must NEVER commit token 1 (which would advance the watermark
/// past the never-sent token 0). Assert: the committed watermark never
/// advances past the last successfully-sunk block -- i.e. NOTHING is
/// committed, and the run STOPS (terminal) rather than draining token 1.
#[tokio::test]
async fn sink_error_blocks_later_ordered_commits() {
    let receiver = OrderedReceiver::new(3);
    let committed = Arc::clone(&receiver.committed_hwm);
    let commit_calls = Arc::clone(&receiver.commit_calls);

    let engine = default_engine();
    let shutdown = CancellationToken::new();
    // Safety net: if the loop wrongly continued, shutdown stops it so the
    // test cannot hang. The assertions still catch the data-loss advance.
    cancel_after(shutdown.clone(), 500);

    let sink_calls = Arc::new(AtomicUsize::new(0));
    let sc = Arc::clone(&sink_calls);

    let result = engine
        .run_workbatch(
            &receiver,
            shutdown,
            |batch| Ok(batch),
            move |out: &WorkBatch<_>| {
                let sc = Arc::clone(&sc);
                // Fail the sink for the block carrying token 0.
                let carries_zero = out.commit_tokens.iter().any(|t| t.seq == 0);
                async move {
                    sc.fetch_add(1, Ordering::Relaxed);
                    if carries_zero {
                        Err(EngineError::Sink("boom on token 0".into()))
                    } else {
                        Ok(())
                    }
                }
            },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await;

    // The ack barrier: token 0 failed, so the watermark must NOT advance
    // past it. NOTHING may be committed (token 1 must never commit ahead).
    assert_eq!(
        committed.load(Ordering::Relaxed),
        u64::MAX,
        "sink error on token 0 must leave the committed watermark unmoved -- \
             a later token must NOT be committed past the failed offset"
    );
    assert_eq!(
        commit_calls.load(Ordering::Relaxed),
        0,
        "no commit may fire while token 0's block is unsent"
    );
    // The fix makes the sink error TERMINAL: the run returns Err and the
    // loop never advances to deliver token 1.
    assert!(
        result.is_err(),
        "sink failure under Auto must be a terminal engine error (ack barrier), \
             not a logged continue that drains later blocks"
    );
    assert_eq!(
        sink_calls.load(Ordering::Relaxed),
        1,
        "loop must stop at the failed block -- token 1 must not be fetched+sunk"
    );
}

/// Ack-barrier on COMMIT failure. The sink succeeds but the COMMIT for
/// token 0's block fails (broker commit error). The engine must treat this
/// as a terminal ack-barrier failure and NOT advance to fetch+commit
/// token 1 past the failed offset.
#[tokio::test]
async fn commit_error_blocks_later_ordered_commits() {
    let mut receiver = OrderedReceiver::new(3);
    receiver.fail_commit_on_seq = Some(0);
    let committed = Arc::clone(&receiver.committed_hwm);

    let engine = default_engine();
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 500);

    let sink_calls = Arc::new(AtomicUsize::new(0));
    let sc = Arc::clone(&sink_calls);

    let result = engine
        .run_workbatch(
            &receiver,
            shutdown,
            |batch| Ok(batch),
            move |_out: &WorkBatch<_>| {
                let sc = Arc::clone(&sc);
                async move {
                    sc.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                }
            },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await;

    // Commit of token 0 failed -> watermark unmoved, run terminates, token 1
    // is never fetched/committed past the failed offset.
    assert_eq!(
        committed.load(Ordering::Relaxed),
        u64::MAX,
        "failed commit must not leave a later commit to advance past it"
    );
    assert!(
        result.is_err(),
        "commit failure must be a terminal ack-barrier error"
    );
    assert_eq!(
        sink_calls.load(Ordering::Relaxed),
        1,
        "loop must stop at the failed commit -- token 1 must not be processed"
    );
}

/// Streaming variant of the ack barrier: a sink error on token 0's block
/// (streamed in sub-blocks) must block any later ordered commit. Mid-block
/// sink failure stops the block AND must not let a later block's commit
/// advance the watermark past it.
#[tokio::test]
async fn streaming_sink_error_blocks_later_ordered_commits() {
    let receiver = OrderedReceiver::new(3);
    let committed = Arc::clone(&receiver.committed_hwm);
    let commit_calls = Arc::clone(&receiver.commit_calls);

    let engine = default_engine();
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 500);

    let sink_calls = Arc::new(AtomicUsize::new(0));
    let sc = Arc::clone(&sink_calls);

    let result = engine
        .run_workbatch_streaming(
            &receiver,
            shutdown,
            |batch| Ok(batch),
            move |out: &WorkBatch<_>| {
                let sc = Arc::clone(&sc);
                // Streaming sub-block views carry EMPTY commit_tokens, so we
                // identify token 0's block by its payload bytes ({"seq":0}).
                let carries_zero = out
                    .records
                    .iter()
                    .any(|r| r.payload.as_ref() == br#"{"seq":0}"#);
                async move {
                    sc.fetch_add(1, Ordering::Relaxed);
                    if carries_zero {
                        Err(EngineError::Sink("boom on token 0 (streaming)".into()))
                    } else {
                        Ok(())
                    }
                }
            },
            CommitMode::Auto,
            64, // one record per sub-block (records are tiny)
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await;

    assert_eq!(
        committed.load(Ordering::Relaxed),
        u64::MAX,
        "streaming sink error on token 0 must not let a later token commit ahead"
    );
    assert_eq!(
        commit_calls.load(Ordering::Relaxed),
        0,
        "no commit may fire while token 0's block is unsent (streaming)"
    );
    assert!(
        result.is_err(),
        "streaming sink failure under Auto must be a terminal ack-barrier error"
    );
    assert_eq!(
        sink_calls.load(Ordering::Relaxed),
        1,
        "streaming loop must stop at the failed block"
    );
}

// ---- Task G4: per-unit streaming -------------------------------------

/// split_into_sub_blocks unit coverage: byte-budget splitting + floor-1.
#[test]
fn split_groups_by_byte_target() {
    // Five 10-byte records, target 25 -> sub-blocks of {2,2,1} records
    // (20 <= 25; adding the 3rd would be 30 > 25 -> close at 2).
    let records: Vec<Record> = (0..5)
        .map(|_| Record {
            payload: Bytes::from_static(b"0123456789"), // 10 bytes
            key: None,
            headers: vec![],
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Json,
            },
        })
        .collect();
    let sub = BatchEngine::split_into_sub_blocks(records, 25);
    let lens: Vec<usize> = sub.iter().map(Vec::len).collect();
    assert_eq!(lens, vec![2, 2, 1], "20<=25 per block, never overshoot 25");
}

#[test]
fn split_floor_one_oversized_record() {
    // A record larger than the target is still its own sub-block (no stall).
    let records = vec![
        Record {
            payload: Bytes::from_static(b"this-payload-is-way-over-the-target"),
            key: None,
            headers: vec![],
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Json,
            },
        },
        Record {
            payload: Bytes::from_static(b"small"),
            key: None,
            headers: vec![],
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Json,
            },
        },
    ];
    let sub = BatchEngine::split_into_sub_blocks(records, 4);
    let lens: Vec<usize> = sub.iter().map(Vec::len).collect();
    assert_eq!(lens, vec![1, 1], "oversized record floors to one-per-block");
}

#[test]
fn split_empty_yields_no_sub_blocks() {
    let sub = BatchEngine::split_into_sub_blocks(Vec::new(), 100);
    assert!(sub.is_empty());
}

#[test]
fn split_smaller_than_target_is_one_sub_block() {
    let records: Vec<Record> = (0..3)
        .map(|_| Record {
            payload: Bytes::from_static(b"abc"),
            key: None,
            headers: vec![],
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Json,
            },
        })
        .collect();
    let sub = BatchEngine::split_into_sub_blocks(records, 10_000);
    assert_eq!(sub.len(), 1, "whole batch under target -> single sub-block");
    assert_eq!(sub[0].len(), 3);
}

/// THE peak-memory proving test: a batch of N records totalling B bytes,
/// streamed with sub_block_bytes ~= B/4. A guard with a registered guard
/// (no heap source) reports current_bytes() = the outstanding lease. The sink
/// samples guard.current_bytes() on EACH call (the sub-block lease is held
/// during the sink); the high-water must stay at ~one sub-block, NOT the whole
/// batch B. The contrast: drive_block would peak at B.
#[cfg(feature = "memory")]
#[tokio::test]
async fn streaming_peak_lease_bounded_to_one_sub_block() {
    use crate::memory::{MemoryGuard, MemoryGuardConfig};

    // 16 records of 64 bytes each = 1024 bytes total.
    const RECORD_BYTES: usize = 64;
    const N: usize = 16;
    let total: u64 = (RECORD_BYTES * N) as u64; // 1024
    let payload = vec![b'x'; RECORD_BYTES];

    let transport = mem_transport(50);
    for _ in 0..N {
        transport.inject(None, payload.clone()).await.unwrap();
    }

    let mut engine = default_engine();
    let guard = Arc::new(MemoryGuard::new(MemoryGuardConfig {
        limit_bytes: 1024 * 1024,
        ..Default::default()
    }));
    engine.set_memory_guard_for_test(Arc::clone(&guard));

    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 200);

    // Sub-block target ~= B/4 -> ~256 bytes -> 4 records per sub-block.
    let sub_block_bytes = total / 4; // 256
    let one_sub_block_bytes = sub_block_bytes; // 4 records * 64 = 256

    // High-water of the guard's accounted bytes, sampled while the sub-block
    // lease is held (the sink runs inside the leased window).
    let high_water = Arc::new(AtomicU64::new(0));
    let guard_for_sink = Arc::clone(&guard);
    let hw = Arc::clone(&high_water);

    engine
        .run_workbatch_streaming(
            &transport,
            shutdown,
            |batch| Ok(batch),
            move |_out: &WorkBatch<_>| {
                let guard = Arc::clone(&guard_for_sink);
                let hw = Arc::clone(&hw);
                async move {
                    let now = guard.current_bytes();
                    hw.fetch_max(now, Ordering::Relaxed);
                    Ok(())
                }
            },
            CommitMode::Auto,
            sub_block_bytes,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await
        .unwrap();

    let peak = high_water.load(Ordering::Relaxed);
    // Peak in-flight lease is ONE sub-block, never the whole batch.
    assert!(
        peak <= one_sub_block_bytes,
        "peak lease {peak} exceeded one sub-block {one_sub_block_bytes} \
             (a whole-batch lease would be {total})"
    );
    assert!(
        peak > 0 && peak < total,
        "peak {peak} must be a partial sub-block, strictly less than the \
             whole batch {total}"
    );
    // Lease fully released after the run.
    assert_eq!(guard.current_bytes(), 0, "all leases released after run");
}

/// A counting receiver: delegates recv/lifecycle to an inner MemoryTransport,
/// but records EACH commit call (count + the tokens + how many sink calls had
/// happened by then) so the test can prove "commit fires exactly once, after
/// the final sub-block, with all N source tokens".
struct CountingReceiver {
    inner: MemoryTransport,
    commit_calls: Arc<AtomicUsize>,
    commit_token_count: Arc<AtomicUsize>,
    sink_calls: Arc<AtomicUsize>,
    sink_calls_at_commit: Arc<AtomicUsize>,
}

impl crate::transport::TransportBase for CountingReceiver {
    fn close(
        &self,
    ) -> impl std::future::Future<Output = crate::transport::TransportResult<()>> + Send {
        self.inner.close()
    }
    fn is_healthy(&self) -> bool {
        self.inner.is_healthy()
    }
    fn name(&self) -> &'static str {
        self.inner.name()
    }
}

impl TransportReceiver for CountingReceiver {
    type Token = <MemoryTransport as TransportReceiver>::Token;

    fn recv(
        &self,
        max: usize,
    ) -> impl std::future::Future<Output = crate::transport::TransportResult<WorkBatch<Self::Token>>>
    + Send {
        self.inner.recv(max)
    }

    async fn commit(&self, tokens: &[Self::Token]) -> crate::transport::TransportResult<()> {
        self.commit_calls.fetch_add(1, Ordering::Relaxed);
        self.commit_token_count
            .fetch_add(tokens.len(), Ordering::Relaxed);
        self.sink_calls_at_commit
            .store(self.sink_calls.load(Ordering::Relaxed), Ordering::Relaxed);
        self.inner.commit(tokens).await
    }
}

/// Commit-once-after-final: N source tokens streamed across multiple
/// sub-blocks. Commit must fire EXACTLY once, AFTER the last sub-block's sink,
/// carrying ALL N source tokens (at-least-once on the whole block).
#[tokio::test]
async fn streaming_commits_once_after_final_sub_block() {
    const N: usize = 12;
    const RECORD_BYTES: usize = 32;
    let payload = vec![b'y'; RECORD_BYTES];

    let inner = mem_transport(50);
    for _ in 0..N {
        inner.inject(None, payload.clone()).await.unwrap();
    }

    let commit_calls = Arc::new(AtomicUsize::new(0));
    let commit_token_count = Arc::new(AtomicUsize::new(0));
    let sink_calls = Arc::new(AtomicUsize::new(0));
    let sink_calls_at_commit = Arc::new(AtomicUsize::new(0));
    let receiver = CountingReceiver {
        inner,
        commit_calls: Arc::clone(&commit_calls),
        commit_token_count: Arc::clone(&commit_token_count),
        sink_calls: Arc::clone(&sink_calls),
        sink_calls_at_commit: Arc::clone(&sink_calls_at_commit),
    };

    let engine = default_engine();
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 200);

    let sc = Arc::clone(&sink_calls);
    // ~3 records per sub-block (96 bytes) -> 4 sub-blocks for 12 records.
    let sub_block_bytes = (RECORD_BYTES * 3) as u64;

    engine
        .run_workbatch_streaming(
            &receiver,
            shutdown,
            |batch| Ok(batch),
            move |_out: &WorkBatch<_>| {
                let sc = Arc::clone(&sc);
                async move {
                    sc.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                }
            },
            CommitMode::Auto,
            sub_block_bytes,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await
        .unwrap();

    let total_sinks = sink_calls.load(Ordering::Relaxed);
    assert!(
        total_sinks >= 4,
        "expected multiple sub-block sinks, got {total_sinks}"
    );
    // Commit fired exactly ONCE.
    assert_eq!(commit_calls.load(Ordering::Relaxed), 1, "commit fires once");
    // It carried ALL N source tokens.
    assert_eq!(
        commit_token_count.load(Ordering::Relaxed),
        N,
        "commit carried all N source tokens"
    );
    // It fired AFTER the final sub-block sink (all sinks done by commit time).
    assert_eq!(
        sink_calls_at_commit.load(Ordering::Relaxed),
        total_sinks,
        "commit fired after the last sub-block sink"
    );
}

/// A sink error on a MIDDLE sub-block stops the block and skips the commit
/// (the whole block is re-delivered -- at-least-once).
#[tokio::test]
async fn streaming_mid_sub_block_sink_error_skips_commit() {
    const N: usize = 9;
    const RECORD_BYTES: usize = 32;
    let payload = vec![b'z'; RECORD_BYTES];

    let inner = mem_transport(50);
    for _ in 0..N {
        inner.inject(None, payload.clone()).await.unwrap();
    }

    let commit_calls = Arc::new(AtomicUsize::new(0));
    let commit_token_count = Arc::new(AtomicUsize::new(0));
    let sink_calls = Arc::new(AtomicUsize::new(0));
    let sink_calls_at_commit = Arc::new(AtomicUsize::new(0));
    let receiver = CountingReceiver {
        inner,
        commit_calls: Arc::clone(&commit_calls),
        commit_token_count: Arc::clone(&commit_token_count),
        sink_calls: Arc::clone(&sink_calls),
        sink_calls_at_commit: Arc::clone(&sink_calls_at_commit),
    };

    let engine = default_engine();
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 200);

    let sc = Arc::clone(&sink_calls);
    // ~3 records per sub-block -> 3 sub-blocks; fail on the 2nd (middle).
    let sub_block_bytes = (RECORD_BYTES * 3) as u64;

    let result = engine
        .run_workbatch_streaming(
            &receiver,
            shutdown,
            |batch| Ok(batch),
            move |_out: &WorkBatch<_>| {
                let sc = Arc::clone(&sc);
                async move {
                    let nth = sc.fetch_add(1, Ordering::Relaxed) + 1;
                    if nth == 2 {
                        Err(EngineError::Sink("boom on middle sub-block".into()))
                    } else {
                        Ok(())
                    }
                }
            },
            CommitMode::Auto,
            sub_block_bytes,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await;

    // The sink error is TERMINAL (ack barrier): the run returns the error.
    assert!(
        matches!(result, Err(EngineError::Sink(_))),
        "mid sub-block sink error is terminal, got {result:?}"
    );
    // The block stopped at the failing sub-block: no commit, and the 3rd
    // sub-block was never sunk.
    assert_eq!(
        commit_calls.load(Ordering::Relaxed),
        0,
        "mid sub-block sink error must skip commit"
    );
    assert_eq!(
        sink_calls.load(Ordering::Relaxed),
        2,
        "stopped after the failing 2nd sub-block (3rd never sunk)"
    );
}

/// Floor case: a batch smaller than sub_block_bytes streams as ONE sub-block
/// and behaves like drive_block (all records sunk once, commit once).
#[tokio::test]
async fn streaming_small_batch_is_single_sub_block() {
    let transport = mem_transport(50);
    for i in 0..3u64 {
        transport
            .inject(None, format!(r#"{{"id":{i}}}"#).into_bytes())
            .await
            .unwrap();
    }

    let engine = default_engine();
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 200);

    let sink_calls = Arc::new(AtomicUsize::new(0));
    let sink_records = Arc::new(AtomicUsize::new(0));
    let scz = Arc::clone(&sink_calls);
    let srz = Arc::clone(&sink_records);

    engine
        .run_workbatch_streaming(
            &transport,
            shutdown,
            |batch| Ok(batch),
            move |out: &WorkBatch<_>| {
                let scz = Arc::clone(&scz);
                let srz = Arc::clone(&srz);
                let n = out.records.len();
                async move {
                    scz.fetch_add(1, Ordering::Relaxed);
                    srz.fetch_add(n, Ordering::Relaxed);
                    Ok(())
                }
            },
            CommitMode::Auto,
            10_000, // target far larger than the whole batch
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await
        .unwrap();

    assert_eq!(
        sink_calls.load(Ordering::Relaxed),
        1,
        "under-target batch sinks once (single sub-block)"
    );
    assert_eq!(
        sink_records.load(Ordering::Relaxed),
        3,
        "all 3 records sunk"
    );
    assert_eq!(
        transport.committed_sequence(),
        2,
        "all 3 acks committed once"
    );
}

/// The streaming path cannot honour `SinkManaged`: its sub-block views carry
/// EMPTY commit tokens, so the sink never sees the block's source acks and
/// physically cannot own the commit. The driver must fail fast at startup
/// rather than silently commit nothing and freeze the source offset.
#[tokio::test]
async fn streaming_rejects_sink_managed_commit() {
    let transport = mem_transport(50);
    let engine = default_engine();
    let shutdown = CancellationToken::new();

    let result = engine
        .run_workbatch_streaming(
            &transport,
            shutdown,
            |batch| Ok(batch),
            move |_out: &WorkBatch<_>| async move { Ok(()) },
            CommitMode::SinkManaged,
            10_000,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await;

    assert!(
        matches!(result, Err(EngineError::SinkManagedUnsupported)),
        "SinkManaged on the streaming path must fail fast, got {result:?}"
    );
}

// ---- Phase 3: governed run path (default-on self-regulation) ----------

/// Build a real governor over a MemoryGuard and wire its byte budget into
/// the engine, returning (engine, governor) so the test can inspect both.
#[cfg(feature = "governor")]
fn governed_engine() -> (BatchEngine, crate::governor::SelfRegulationGovernor) {
    use crate::memory::{MemoryGuard, MemoryGuardConfig};
    let guard = Arc::new(MemoryGuard::new(MemoryGuardConfig {
        limit_bytes: 1024 * 1024,
        ..Default::default()
    }));
    let gov = crate::governor::SelfRegulationConfig::default()
        .build(guard)
        .expect("enabled by default");
    let mut engine = default_engine();
    engine.set_byte_budget(gov.budget());
    (engine, gov)
}

/// Governor ON: the governed driver streams the input end-to-end through a
/// MemoryTransport, all records reach the sink, the source acks commit, and
/// the AIMD budget moves (observe is folded in per block).
#[cfg(feature = "governor")]
#[tokio::test]
async fn governed_on_streams_and_commits_via_memory_transport() {
    let transport = mem_transport(50);
    for i in 0..6u64 {
        transport
            .inject(None, format!(r#"{{"id":{i}}}"#).into_bytes())
            .await
            .unwrap();
    }

    let (engine, _gov) = governed_engine();
    assert!(engine.is_self_regulated(), "budget wired -> governed path");

    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 200);

    let sink_records = Arc::new(AtomicUsize::new(0));
    let sr = Arc::clone(&sink_records);

    engine
        .run_governed(
            &transport,
            shutdown,
            |batch| Ok(batch),
            move |out: &WorkBatch<_>| {
                let sr = Arc::clone(&sr);
                let n = out.records.len();
                async move {
                    sr.fetch_add(n, Ordering::Relaxed);
                    Ok(())
                }
            },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await
        .unwrap();

    assert_eq!(
        sink_records.load(Ordering::Relaxed),
        6,
        "all records streamed to the sink under the governor"
    );
    assert_eq!(transport.committed_sequence(), 5, "all 6 acks committed");
}

/// Governor OFF: with no byte budget wired, run_governed delegates to the
/// whole-batch run_workbatch -- behaviour is unchanged (one sink call for
/// the whole block, commit once).
#[cfg(feature = "governor")]
#[tokio::test]
async fn governed_off_is_whole_batch_passthrough() {
    let transport = mem_transport(50);
    for i in 0..4u64 {
        transport
            .inject(None, format!(r#"{{"id":{i}}}"#).into_bytes())
            .await
            .unwrap();
    }

    // No set_byte_budget -> byte_budget is None -> OFF path.
    let engine = default_engine();
    assert!(
        !engine.is_self_regulated(),
        "no budget wired -> whole-batch path"
    );

    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 200);

    let sink_calls = Arc::new(AtomicUsize::new(0));
    let sink_records = Arc::new(AtomicUsize::new(0));
    let sc = Arc::clone(&sink_calls);
    let sr = Arc::clone(&sink_records);

    engine
        .run_governed(
            &transport,
            shutdown,
            |batch| Ok(batch),
            move |out: &WorkBatch<_>| {
                let sc = Arc::clone(&sc);
                let sr = Arc::clone(&sr);
                let n = out.records.len();
                async move {
                    sc.fetch_add(1, Ordering::Relaxed);
                    sr.fetch_add(n, Ordering::Relaxed);
                    Ok(())
                }
            },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await
        .unwrap();

    assert_eq!(
        sink_calls.load(Ordering::Relaxed),
        1,
        "OFF path = whole-batch: the block sinks ONCE (not per sub-block)"
    );
    assert_eq!(sink_records.load(Ordering::Relaxed), 4, "all records sunk");
    assert_eq!(transport.committed_sequence(), 3, "all 4 acks committed");
}

/// The shared pressure feeds an InboundGate: under high memory the gate
/// holds (Admit::Hold) and the budget shrinks; low memory admits and the
/// budget sits at start-big. Proves the gate + budget share one pressure.
#[cfg(feature = "governor")]
#[test]
fn governed_gate_and_budget_share_pressure() {
    use crate::governor::{Admit, InboundGate, NoopActuator};
    use crate::memory::{MemoryGuard, MemoryGuardConfig};

    let guard = Arc::new(MemoryGuard::new(MemoryGuardConfig {
        limit_bytes: 1000,
        pressure_threshold: 0.80,
        ..Default::default()
    }));
    let gov = crate::governor::SelfRegulationConfig::default()
        .build(Arc::clone(&guard))
        .expect("enabled");

    let gate = InboundGate::new(gov.pressure(), Box::new(NoopActuator));
    let budget = gov.budget();
    let start = budget.byte_budget();

    // Low memory -> gate admits, budget unchanged on a slack observe.
    assert_eq!(gate.evaluate(), Admit::Yes, "low pressure admits");

    // Slam memory high -> the SAME pressure both holds the gate AND, via the
    // HARD override in observe(), shrinks the budget regardless of rho.
    guard.add_bytes(950); // 95% of limit
    assert_eq!(gate.evaluate(), Admit::Hold, "high pressure holds the gate");
    budget.observe(0, Duration::from_millis(1), Duration::from_millis(100));
    assert!(
        budget.byte_budget() < start,
        "high memory shrinks the shared budget (HARD override)"
    );
}

// ---- Phase 4: validation ---------------------------------------------

/// THE send-unaffected invariant: the OUTBOUND drain (sink) is NEVER gated
/// by pressure -- only the INBOUND recv side is. With a `UnifiedPressure`
/// pinned HARD-HIGH so `should_hold()` is true, the SAME transport's
/// `send` / `send_batch` still succeed. Gating the drain would deadlock the
/// pipeline (in-flight work could never leave), so the governor must never
/// touch it. MemoryTransport's send path consults no pressure governor by
/// construction; this test proves that holds even when a governor that the
/// inbound side WOULD obey is wired and saturated.
#[cfg(feature = "governor")]
#[tokio::test]
async fn send_unaffected_by_pressure_pinned_high() {
    use crate::governor::{Hysteresis, MemoryPressureSource, PressureSource, UnifiedPressure};
    use crate::memory::{MemoryGuard, MemoryGuardConfig};
    use crate::transport::TransportSender;

    // Pin a REAL HARD memory source high so the latch holds (>= pause_above).
    let guard = Arc::new(MemoryGuard::new(MemoryGuardConfig {
        limit_bytes: 1000,
        pressure_threshold: 0.80,
        ..Default::default()
    }));
    guard.add_bytes(950); // 95% -> HARD high
    let pressure = Arc::new(UnifiedPressure::new(
        vec![Arc::new(MemoryPressureSource::new(Arc::clone(&guard))) as Arc<dyn PressureSource>],
        Hysteresis::new(0.80, 0.65).expect("valid band"),
    ));
    assert!(
        pressure.should_hold(),
        "pinned-high governor must hold the INBOUND gate"
    );

    // The OUTBOUND sink: send / send_batch must still succeed under hold.
    let transport = mem_transport(50);

    let single = transport
        .send("k", Bytes::from_static(br#"{"id":1}"#))
        .await;
    assert!(
        single.is_ok(),
        "single send must succeed under pressure (sink never gated), got {single:?}"
    );

    let records: Vec<Record> = (0..5)
        .map(|i| Record {
            payload: Bytes::from(format!(r#"{{"id":{i}}}"#)),
            key: Some(Arc::from(format!("k{i}").as_str())),
            headers: vec![],
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Json,
            },
        })
        .collect();
    let batch_res = transport.send_batch(&records).await;
    assert!(
        batch_res.is_ok(),
        "send_batch must succeed under pressure (sink never gated), got {batch_res:?}"
    );

    // Pressure is STILL high after the sends -- nothing about the send path
    // cleared or consulted it.
    assert!(
        pressure.should_hold(),
        "send does not touch the pressure latch"
    );

    // And the sent data is intact on the wire (the drain really ran).
    let got = transport.recv(10).await.unwrap().records;
    assert_eq!(got.len(), 6, "1 single + 5 batched records all drained");
}

/// Build a governed engine over a guard with a LOW limit, sharing ONE guard
/// between the governor (pressure + budget) and the engine's ingress-lease
/// accounting. Returns `(engine, governor, guard)`.
#[cfg(all(feature = "governor", feature = "memory"))]
fn governed_engine_low_limit(
    limit_bytes: u64,
) -> (
    BatchEngine,
    crate::governor::SelfRegulationGovernor,
    Arc<crate::memory::MemoryGuard>,
) {
    use crate::memory::{MemoryGuard, MemoryGuardConfig};
    let guard = Arc::new(MemoryGuard::new(MemoryGuardConfig {
        limit_bytes,
        pressure_threshold: 0.80,
        ..Default::default()
    }));
    // The governor's pressure + AIMD budget run off THIS guard.
    let gov = crate::governor::SelfRegulationConfig::default()
        .build(Arc::clone(&guard))
        .expect("enabled by default");
    // A SMALL recv chunk so the load arrives over many blocks: the AIMD loop
    // (and the memory HARD override) shrink the budget block-to-block as
    // pressure builds, rather than pulling the whole load in one cold-budget
    // block. This is the realistic streaming shape -- a real broker/source
    // delivers in poll-sized chunks, not one giant block.
    let mut engine = BatchEngine::new(BatchProcessingConfig {
        max_chunk_size: 16,
        ..Default::default()
    });
    engine.set_byte_budget(gov.budget());
    // The engine's ingress leases must account against the SAME guard so the
    // streaming peak-lease feeds back into the pressure the budget reads.
    engine.set_memory_guard_for_test(Arc::clone(&guard));
    (engine, gov, guard)
}

/// THE operational never-OOM test (in-process logical form).
///
/// Drives sustained, large load through `run_governed` over a real
/// `MemoryTransport`, governor ON, with a `MemoryGuard` on a LOW limit. It
/// proves the four never-OOM invariants without a cgroup harness:
///
///   1. the inbound GATE engages -- with the governor's pressure pinned by
///      sustained ingress, an `InboundGate` over the SAME pressure returns
///      `Admit::Hold` (the brake the transport would apply);
///   2. the sink/drain KEEPS RUNNING -- every record reaches the sink and
///      the source acks commit (the drain is never gated);
///   3. `MemoryGuard::current_bytes()` stays BOUNDED -- the streaming
///      peak-lease holds at most ~one shrunk sub-block in flight, well under
///      the whole-batch footprint, sampled at its high-water inside the sink;
///   4. the pipeline does NOT panic and the budget never collapses below its
///      floor (>= 1, never 0).
///
/// A full OS-level cgroup OOM-kill test (a memory-limited container + a real
/// broker or transport under load) is FLAGGED for a CI harness (Phase 5.5);
/// see the report.
#[cfg(all(feature = "governor", feature = "memory"))]
#[tokio::test]
async fn operational_never_oom_governed_pipeline_bounds_memory() {
    use crate::governor::{Admit, InboundGate, NoopActuator};

    // LOW limit, sized so a SINGLE in-flight poll-chunk (16 x 1 KiB =
    // 16 KiB) sits above the 80% pressure threshold (16/18 ~= 0.89), so the
    // gate engages while a sub-block is leased -- yet the streaming
    // peak-lease keeps the in-flight footprint at one chunk, never the whole
    // load. This is the never-OOM shape: high pressure brakes inbound, but
    // memory stays bounded because only one sub-block is ever resident.
    const LIMIT: u64 = 18 * 1024; // 18 KiB
    // Records far larger than the floor; many of them -> sustained load.
    const RECORD_BYTES: usize = 1024; // 1 KiB each
    const N: usize = 256; // 256 KiB of payload total -- 14x the limit
    let payload = vec![b'q'; RECORD_BYTES];
    let total_payload: u64 = (RECORD_BYTES * N) as u64;

    let transport = mem_transport(50);
    for _ in 0..N {
        transport.inject(None, payload.clone()).await.unwrap();
    }

    let (engine, gov, guard) = governed_engine_low_limit(LIMIT);
    assert!(engine.is_self_regulated(), "budget wired -> governed path");

    // The gate the transport WOULD wire in, over the governor's shared
    // pressure. We evaluate it from inside the sink to observe the brake.
    let gate = Arc::new(InboundGate::new(gov.pressure(), Box::new(NoopActuator)));

    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 600);

    let sink_records = Arc::new(AtomicUsize::new(0));
    let high_water = Arc::new(AtomicU64::new(0));
    let gate_held_ever = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let sr = Arc::clone(&sink_records);
    let hw = Arc::clone(&high_water);
    let geh = Arc::clone(&gate_held_ever);
    let guard_for_sink = Arc::clone(&guard);
    let gate_for_sink = Arc::clone(&gate);

    engine
        .run_governed(
            &transport,
            shutdown,
            |batch| Ok(batch),
            move |out: &WorkBatch<_>| {
                let sr = Arc::clone(&sr);
                let hw = Arc::clone(&hw);
                let geh = Arc::clone(&geh);
                let guard = Arc::clone(&guard_for_sink);
                let gate = Arc::clone(&gate_for_sink);
                let n = out.records.len();
                async move {
                    // (3) sample current_bytes() while the sub-block lease is
                    // held -- this is the in-flight high-water.
                    hw.fetch_max(guard.current_bytes(), Ordering::Relaxed);
                    // (1) evaluate the gate over the SAME pressure: under
                    // sustained ingress it engages (Hold).
                    if gate.evaluate() == Admit::Hold {
                        geh.store(true, Ordering::Relaxed);
                    }
                    // (2) the drain keeps running -- count every record sunk.
                    sr.fetch_add(n, Ordering::Relaxed);
                    Ok(())
                }
            },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await
        .unwrap();

    // (2) The drain KEPT RUNNING: every record reached the sink and the
    // source acks committed -- the sink is never gated.
    assert_eq!(
        sink_records.load(Ordering::Relaxed),
        N,
        "all {N} records drained through the governed sink"
    );
    assert_eq!(
        transport.committed_sequence(),
        (N - 1) as u64,
        "all source acks committed (drain never stalled)"
    );

    // (1) The inbound gate ENGAGED at least once under the sustained load --
    // the brake the transport would apply did fire.
    assert!(
        gate_held_ever.load(Ordering::Relaxed),
        "inbound gate must engage (Admit::Hold) under sustained pressure"
    );

    // (3) Peak in-flight bytes stayed BOUNDED, NOT the whole payload. The
    // streaming peak-lease bounds it to ~one shrunk sub-block; allow generous
    // headroom but it must be a small fraction of the whole-batch footprint.
    let peak = high_water.load(Ordering::Relaxed);
    assert!(
        peak > 0,
        "some bytes must be accounted while a sub-block is in flight"
    );
    assert!(
        peak < total_payload / 2,
        "peak in-flight {peak} must stay well under half the whole payload \
             {total_payload} (streaming peak-lease bounds it, never OOM)"
    );

    // (4) Budget respected its floor (>= 1, never 0) and the run did not
    // panic (reaching here proves it). All leases released after the run.
    assert!(
        gov.budget().byte_budget() >= 1,
        "byte budget never collapses below its floor"
    );
    assert_eq!(
        guard.current_bytes(),
        0,
        "all ingress leases released after the run -- no leak"
    );
}

// ---- Remediation Phase 2: byte-aware recv bounds RECEIVE memory -------
//
// The gap: the governed driver bounds memory by the
// post-recv SUB-BLOCK lease, but `recv(max)` is RECORD-bounded only -- a
// single poll can build a WorkBatch whose total bytes >> byte_budget BEFORE
// any sub-block split, so the byte budget did NOT bound RECEIVE memory. The
// fix routes the governed recv through `recv_limited(RecvLimits)` so the poll
// is bounded by BOTH the record cap AND the byte budget.

/// A REAL test transport (not a mock of internal code -- a concrete
/// `TransportReceiver` over owned `Record`/`WorkBatch`/`MemoryToken`) that
/// makes the gap observable:
///
/// - `recv(max)` is RECORD-bounded: it hands out up to `max` records in ONE
///   block regardless of their bytes -- exactly the pre-fix behaviour that
///   let a single poll retain bytes >> budget.
/// - `recv_limited(limits)` is BYTE-bounded: it accumulates records until the
///   payload bytes reach `limits.max_bytes`, FLOOR one record.
///
/// Every handed-out block's total payload bytes are folded into a shared
/// high-water so the test can assert the bytes RETAINED at recv time.
struct ByteAwareSource {
    /// Remaining records to hand out (front = next).
    remaining: std::sync::Mutex<std::collections::VecDeque<Record>>,
    /// High-water of the bytes handed out in any single recv/recv_limited.
    recv_high_water: Arc<AtomicU64>,
    committed: Arc<AtomicU64>,
}

impl ByteAwareSource {
    fn new(records: Vec<Record>, recv_high_water: Arc<AtomicU64>) -> Self {
        Self {
            remaining: std::sync::Mutex::new(records.into_iter().collect()),
            recv_high_water,
            committed: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Pull a block (front records) bounded by an optional byte cap and a
    /// record cap, folding its total bytes into the high-water. Returns
    /// `None` when the source is exhausted (the caller then PENDS forever so
    /// the run loop parks until shutdown -- never a busy spin).
    fn pull(&self, max_records: usize, max_bytes: Option<u64>) -> Option<WorkBatch<MemTok2>> {
        let mut q = self.remaining.lock().unwrap();
        if q.is_empty() {
            return None;
        }
        let mut records = Vec::new();
        let mut bytes: u64 = 0;
        while records.len() < max_records {
            let Some(front) = q.front() else { break };
            let rb = front.payload.len() as u64;
            // Byte cap with floor-1: stop only once we already hold >= 1.
            if let Some(cap) = max_bytes
                && !records.is_empty()
                && bytes.saturating_add(rb) > cap
            {
                break;
            }
            bytes = bytes.saturating_add(rb);
            records.push(q.pop_front().expect("front exists"));
        }
        self.recv_high_water.fetch_max(bytes, Ordering::Relaxed);
        let n = records.len() as u64;
        let base = self.committed.load(Ordering::Relaxed);
        let tokens: Vec<MemTok2> = (0..n).map(|i| MemTok2 { seq: base + i }).collect();
        Some(WorkBatch::new(records, tokens))
    }
}

#[derive(Debug, Clone, Copy)]
struct MemTok2 {
    seq: u64,
}
impl std::fmt::Display for MemTok2 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "memtok2:{}", self.seq)
    }
}
impl crate::transport::CommitToken for MemTok2 {}

impl crate::transport::TransportBase for ByteAwareSource {
    fn close(
        &self,
    ) -> impl std::future::Future<Output = crate::transport::TransportResult<()>> + Send {
        std::future::ready(Ok(()))
    }
    fn is_healthy(&self) -> bool {
        true
    }
    fn name(&self) -> &'static str {
        "byte-aware-source"
    }
}

impl TransportReceiver for ByteAwareSource {
    type Token = MemTok2;

    fn recv(
        &self,
        max: usize,
    ) -> impl std::future::Future<Output = crate::transport::TransportResult<WorkBatch<Self::Token>>>
    + Send {
        // RECORD-bounded only -- ignores bytes. This is the pre-fix shape: a
        // single poll can retain bytes >> any budget.
        let pulled = self.pull(max, None);
        async move {
            match pulled {
                Some(batch) => Ok(batch),
                // Exhausted: park forever so the loop only exits on shutdown
                // (mirrors a quiet source -- never a busy spin).
                None => std::future::pending().await,
            }
        }
    }

    fn recv_limited(
        &self,
        limits: crate::transport::RecvLimits,
    ) -> impl std::future::Future<Output = crate::transport::TransportResult<WorkBatch<Self::Token>>>
    + Send {
        // BYTE-bounded (floor one record): the fix path.
        let pulled = self.pull(limits.max_records, Some(limits.max_bytes));
        async move {
            match pulled {
                Some(batch) => Ok(batch),
                None => std::future::pending().await,
            }
        }
    }

    async fn commit(&self, tokens: &[Self::Token]) -> crate::transport::TransportResult<()> {
        if let Some(max_seq) = tokens.iter().map(|t| t.seq).max() {
            self.committed.fetch_max(max_seq, Ordering::Relaxed);
        }
        Ok(())
    }
}

/// THE reproduce/fix test: drive the GOVERNED loop over a source that could
/// deliver a block whose total bytes are FAR larger than the byte budget.
///
/// PRE-FIX (governed recv == `recv(record_cap)`): the source's record-bounded
/// `recv` hands out the whole big block in one poll, so the bytes RETAINED at
/// recv time = the whole block >> budget. The high-water assertion below
/// FAILS (this is the reproduction).
///
/// POST-FIX (governed recv == `recv_limited(record_cap, byte_budget)`): the
/// source's byte-bounded `recv_limited` caps each poll at the budget (+ one
/// record), so the retained bytes stay ~<= budget + one record.
#[cfg(feature = "governor")]
#[tokio::test]
async fn governed_recv_is_byte_bounded_not_record_bounded() {
    use crate::memory::{MemoryGuard, MemoryGuardConfig};

    // 64 records of 4 KiB each = 256 KiB total available in the source.
    const RECORD_BYTES: usize = 4 * 1024;
    const N: usize = 64;
    // A SMALL byte budget: 16 KiB (4 records). The record cap is large (2000
    // default) so the count NEVER bounds the poll -- only the byte cap can.
    const BUDGET: u64 = 16 * 1024;

    let total: u64 = (RECORD_BYTES * N) as u64; // 256 KiB
    let payload = vec![b'b'; RECORD_BYTES];
    let records: Vec<Record> = (0..N)
        .map(|_| Record {
            payload: Bytes::from(payload.clone()),
            key: None,
            headers: vec![],
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Json,
            },
        })
        .collect();

    let guard = Arc::new(MemoryGuard::new(MemoryGuardConfig {
        limit_bytes: 1024 * 1024,
        ..Default::default()
    }));
    let cfg = crate::governor::ByteBudgetConfig {
        start_bytes: BUDGET,
        max_bytes: BUDGET, // pin it so the budget cannot grow past BUDGET
        floor_records: 1,
        nominal_record_bytes: RECORD_BYTES as u64,
        record_cap: 4096, // far above N -- count never bounds the poll
        ..Default::default()
    };
    let pressure = crate::governor::SelfRegulationConfig::default()
        .build(Arc::clone(&guard))
        .expect("enabled")
        .pressure();
    let budget = Arc::new(crate::governor::ByteBudgetController::new(
        cfg,
        Arc::clone(&pressure),
    ));

    let recv_high_water = Arc::new(AtomicU64::new(0));
    let source = ByteAwareSource::new(records, Arc::clone(&recv_high_water));

    let mut engine = BatchEngine::new(BatchProcessingConfig {
        // Big chunk so config never bounds the poll either -- the byte budget
        // is the ONLY thing that can.
        max_chunk_size: 4096,
        ..Default::default()
    });
    engine.set_byte_budget(budget);

    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 250);

    engine
        .run_governed(
            &source,
            shutdown,
            |batch| Ok(batch),
            |_out: &WorkBatch<_>| async { Ok(()) },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await
        .unwrap();

    let peak = recv_high_water.load(Ordering::Relaxed);
    // The fix: a single governed recv retains at most the byte budget plus
    // one oversized-record floor -- NOT the whole 256 KiB block.
    assert!(
        peak <= BUDGET + RECORD_BYTES as u64,
        "governed recv retained {peak} bytes at recv time -- must be bounded \
             by the byte budget {BUDGET} (+ one record {RECORD_BYTES}), not the \
             whole {total}-byte block (record-bounded recv would retain all of it)"
    );
    assert!(
        peak > 0,
        "the source did hand out records (sanity: the loop ran)"
    );
}

/// The sub-block drain is LAZY: it yields one sub-block at a time and does
/// NOT allocate every sub-block up front. We assert incremental yield -- the
/// first `next_sub_block()` returns one budget-sized sub-block while records
/// for later sub-blocks remain un-pulled in the drain.
#[test]
fn sub_block_drain_yields_incrementally() {
    // 6 records of 10 bytes; target 25 -> sub-blocks {2, 2, 2}.
    let records: Vec<Record> = (0..6)
        .map(|_| Record {
            payload: Bytes::from_static(b"0123456789"),
            key: None,
            headers: vec![],
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Json,
            },
        })
        .collect();
    let mut drain = SubBlockDrain::new(records, 25);

    // First pull yields ONE sub-block (2 records); the remaining 4 are still
    // inside the drain, NOT pre-materialised into sub-block vectors.
    let first = drain.next_sub_block().expect("first sub-block");
    assert_eq!(first.len(), 2, "first sub-block is one budget's worth");
    // The drain still has records to give (proves it did not eagerly split).
    let second = drain.next_sub_block().expect("second sub-block");
    assert_eq!(second.len(), 2);
    let third = drain.next_sub_block().expect("third sub-block");
    assert_eq!(third.len(), 2);
    // Now exhausted.
    assert!(drain.next_sub_block().is_none(), "drain exhausted");
}

// ---- Remediation Phase 3: DLQ + parse-error-action semantics ----------
//
// Two findings the parsed/process paths had:
//   1. parse_block hardcoded route-to-DLQ, ignoring ParseErrorAction.
//   2. out_batch.dlq_entries from process were never routed before commit
//      (silent-drop path) -- only inbound-filter entries were routed.
// These tests pin the fixed contract: one route point, one policy, fallible
// route, parse_error_action honoured on the parsed path.

use crate::worker::engine::FilterDlqPolicy;
use crate::worker::engine::config::ParseErrorAction;

/// An engine with a specific `ParseErrorAction` (default config otherwise).
fn engine_with_parse_action(action: ParseErrorAction) -> BatchEngine {
    BatchEngine::new(BatchProcessingConfig {
        parse_error_action: action,
        ..Default::default()
    })
}

/// Finding 1 -- `ParseErrorAction::Skip`: a parse failure on the parsed path
/// is DROPPED silently (NO DLQ entry routed) yet the survivors are kept and
/// ALL source acks commit (the block's tokens are decoupled from records).
#[tokio::test]
async fn parsed_parse_error_skip_drops_without_dlq_and_commits_survivors() {
    let transport = mem_transport(50);
    transport
        .inject(None, br#"{"id":1}"#.to_vec())
        .await
        .unwrap(); // seq 0 ok
    transport
        .inject(None, b"not json {{{".to_vec())
        .await
        .unwrap(); // seq 1 bad
    transport
        .inject(None, br#"{"id":3}"#.to_vec())
        .await
        .unwrap(); // seq 2 ok

    // Route policy so we can PROVE no entry is routed under Skip.
    let routed = Arc::new(AtomicUsize::new(0));
    let rc = Arc::clone(&routed);
    let engine = engine_with_parse_action(ParseErrorAction::Skip).with_filter_dlq_policy(
        FilterDlqPolicy::Route(Arc::new(
            move |entries: Vec<crate::transport::filter::FilteredDlqEntry>| {
                rc.fetch_add(entries.len(), Ordering::Relaxed);
                Ok(())
            },
        )),
    );
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 200);

    let dlq_seen = Arc::new(AtomicUsize::new(0));
    let kept = Arc::new(AtomicUsize::new(0));
    let ds = Arc::clone(&dlq_seen);
    let kp = Arc::clone(&kept);

    engine
        .run_workbatch_parsed(
            &transport,
            shutdown,
            move |pb: ParsedBatch<'_, _>| {
                ds.fetch_add(pb.dlq_entries.len(), Ordering::Relaxed);
                kp.fetch_add(pb.records.len(), Ordering::Relaxed);
                Ok(WorkBatch::new(pb.records, pb.commit_tokens).with_dlq_entries(pb.dlq_entries))
            },
            |_out: &WorkBatch<_>| async { Ok(()) },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await
        .unwrap();

    assert_eq!(kept.load(Ordering::Relaxed), 2, "2 survivors kept");
    assert_eq!(
        dlq_seen.load(Ordering::Relaxed),
        0,
        "Skip: parse failure produces NO DLQ entry (dropped, not dead-lettered)"
    );
    assert_eq!(
        routed.load(Ordering::Relaxed),
        0,
        "Skip: nothing reaches the DLQ route point"
    );
    // All three source acks committed -- survivors and the dropped record's
    // ack alike (at-least-once on the whole block; Skip is opt-in loss).
    assert_eq!(transport.committed_sequence(), 2);
}

/// Finding 1 -- `ParseErrorAction::FailBatch`: a parse failure fails the
/// WHOLE block terminally (no commit), consistent with the ack barrier. The
/// run returns the terminal error and the source watermark does not advance.
#[tokio::test]
async fn parsed_parse_error_fail_batch_skips_commit() {
    // OrderedReceiver hands one record per recv with monotonic tokens and a
    // cumulative watermark, so we can prove the commit never fired.
    let receiver = OrderedReceiverBad::new();
    let committed = Arc::clone(&receiver.committed_hwm);

    let engine = engine_with_parse_action(ParseErrorAction::FailBatch);
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 500);

    let sink_calls = Arc::new(AtomicUsize::new(0));
    let sc = Arc::clone(&sink_calls);

    let result = engine
        .run_workbatch_parsed(
            &receiver,
            shutdown,
            |pb: ParsedBatch<'_, _>| {
                Ok(WorkBatch::new(pb.records, pb.commit_tokens).with_dlq_entries(pb.dlq_entries))
            },
            move |_out: &WorkBatch<_>| {
                let sc = Arc::clone(&sc);
                async move {
                    sc.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                }
            },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await;

    assert!(
        matches!(result, Err(EngineError::ParseBatchFailed(_))),
        "FailBatch: a parse failure is a terminal engine error, got {result:?}"
    );
    assert_eq!(
        committed.load(Ordering::Relaxed),
        u64::MAX,
        "FailBatch: the whole block fails its commit -- watermark unmoved"
    );
    assert_eq!(
        sink_calls.load(Ordering::Relaxed),
        0,
        "FailBatch: the block never reaches the sink (parse fails first)"
    );
}

/// Finding 1 -- `ParseErrorAction::Dlq`: a parse failure routes to the DLQ
/// route point BEFORE commit, survivors are sunk, all source acks commit.
#[tokio::test]
async fn parsed_parse_error_dlq_routes_before_commit() {
    let transport = Arc::new(mem_transport(50));
    transport
        .inject(None, br#"{"id":1}"#.to_vec())
        .await
        .unwrap(); // seq 0 ok
    transport
        .inject(None, b"not json {{{".to_vec())
        .await
        .unwrap(); // seq 1 bad
    transport
        .inject(None, br#"{"id":3}"#.to_vec())
        .await
        .unwrap(); // seq 2 ok

    // Sample committed_sequence at DLQ-route time to prove route precedes
    // commit: when the route sink fires, the commit must NOT yet have run.
    let routed = Arc::new(AtomicUsize::new(0));
    let committed_at_route = Arc::new(AtomicU64::new(u64::MAX));
    let rc = Arc::clone(&routed);
    let car = Arc::clone(&committed_at_route);
    let transport_for_route = Arc::clone(&transport);
    let engine = engine_with_parse_action(ParseErrorAction::Dlq).with_filter_dlq_policy(
        FilterDlqPolicy::Route(Arc::new(
            move |entries: Vec<crate::transport::filter::FilteredDlqEntry>| {
                car.store(transport_for_route.committed_sequence(), Ordering::Relaxed);
                rc.fetch_add(entries.len(), Ordering::Relaxed);
                Ok(())
            },
        )),
    );
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 200);

    engine
        .run_workbatch_parsed(
            &*transport,
            shutdown,
            |pb: ParsedBatch<'_, _>| {
                Ok(WorkBatch::new(pb.records, pb.commit_tokens).with_dlq_entries(pb.dlq_entries))
            },
            |_out: &WorkBatch<_>| async { Ok(()) },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await
        .unwrap();

    assert_eq!(
        routed.load(Ordering::Relaxed),
        1,
        "Dlq: the parse failure reached the DLQ route point"
    );
    // The route fired BEFORE the commit: MemoryTransport's committed_sequence
    // starts at 0; the block's highest seq is 2, so a commit would set it to
    // 2. At route time it must still be its pre-commit value (0).
    assert_eq!(
        committed_at_route.load(Ordering::Relaxed),
        0,
        "DLQ route ran BEFORE the source commit advanced the watermark"
    );
    assert_eq!(
        transport.committed_sequence(),
        2,
        "all 3 acks committed after"
    );
}

/// Finding 2 -- the STANDARD (on-demand) `run_workbatch` path must NOT
/// silently drop DLQ entries that `process` emits on the out-batch. A
/// process closure that attaches a dlq_entry has it ROUTED (reaches the DLQ
/// route point) before the sink-success leads to a source commit -- it does
/// not depend on the sink closure remembering to carry it.
#[tokio::test]
async fn standard_send_batch_sink_does_not_silently_drop_dlq_entries() {
    let transport = mem_transport(50);
    transport
        .inject(None, br#"{"id":1}"#.to_vec())
        .await
        .unwrap();
    transport
        .inject(None, br#"{"id":2}"#.to_vec())
        .await
        .unwrap();

    let routed = Arc::new(AtomicUsize::new(0));
    let rc = Arc::clone(&routed);
    let engine = default_engine().with_filter_dlq_policy(FilterDlqPolicy::Route(Arc::new(
        move |entries: Vec<crate::transport::filter::FilteredDlqEntry>| {
            rc.fetch_add(entries.len(), Ordering::Relaxed);
            Ok(())
        },
    )));
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 200);

    // The SINK ignores dlq_entries entirely (the realistic app shape). The
    // PROCESS closure emits a dlq_entry on the out-batch. Pre-fix this entry
    // would vanish; post-fix the driver routes it before commit.
    engine
        .run_workbatch(
            &transport,
            shutdown,
            |batch| {
                let dlq = vec![crate::transport::filter::FilteredDlqEntry {
                    payload: b"process-emitted dead-letter".to_vec(),
                    key: None,
                    reason: "process decided this record is bad".to_string(),
                }];
                let tokens = batch.commit_tokens;
                let records = batch.records;
                Ok(WorkBatch::new(records, tokens).with_dlq_entries(dlq))
            },
            |_out: &WorkBatch<_>| async { Ok(()) },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await
        .unwrap();

    assert!(
        routed.load(Ordering::Relaxed) >= 1,
        "process-emitted DLQ entry must reach the DLQ route point, not be \
             silently dropped on the path to commit"
    );
    // Source acks still commit -- the dead-letter routing is independent of
    // the source ack (at-least-once on the whole block).
    assert_eq!(transport.committed_sequence(), 1);
}

/// Finding 3 -- a DLQ-route FAILURE under `Route` is a terminal ack-barrier
/// error: the source commit is skipped (no later ordered commit advances
/// past the undelivered dead-letters). Silent discard is opt-in only.
#[tokio::test]
async fn dlq_route_failure_is_terminal_and_blocks_commit() {
    let receiver = OrderedReceiverBad::without_parse_fail();
    let committed = Arc::clone(&receiver.committed_hwm);

    // A Route sink that FAILS, simulating a DLQ transport outage.
    let engine = default_engine().with_filter_dlq_policy(FilterDlqPolicy::Route(Arc::new(
        |_e: Vec<crate::transport::filter::FilteredDlqEntry>| {
            Err(EngineError::Sink("dlq transport down".into()))
        },
    )));
    let shutdown = CancellationToken::new();
    cancel_after(shutdown.clone(), 500);

    let result = engine
        .run_workbatch(
            &receiver,
            shutdown,
            |batch| {
                // process emits a dlq entry; routing it will fail.
                let dlq = vec![crate::transport::filter::FilteredDlqEntry {
                    payload: b"bad".to_vec(),
                    key: None,
                    reason: "process dlq".to_string(),
                }];
                Ok(WorkBatch::new(batch.records, batch.commit_tokens).with_dlq_entries(dlq))
            },
            |_out: &WorkBatch<_>| async { Ok(()) },
            CommitMode::Auto,
            None::<(
                Duration,
                fn() -> std::future::Ready<Result<(), EngineError>>,
            )>,
        )
        .await;

    assert!(
        result.is_err(),
        "DLQ route failure must be a terminal ack-barrier error, got {result:?}"
    );
    assert_eq!(
        committed.load(Ordering::Relaxed),
        u64::MAX,
        "DLQ route failure must skip the commit -- watermark unmoved"
    );
}

/// An ordered receiver that delivers ONE bad (unparseable) record then parks.
/// Cumulative watermark via fetch_max, so a commit is observable. Used to
/// prove FailBatch / DLQ-route-failure leave the watermark unmoved.
struct OrderedReceiverBad {
    next: Arc<AtomicU64>,
    committed_hwm: Arc<AtomicU64>,
    good_payload: bool,
}

impl OrderedReceiverBad {
    fn new() -> Self {
        Self {
            next: Arc::new(AtomicU64::new(0)),
            committed_hwm: Arc::new(AtomicU64::new(u64::MAX)),
            good_payload: false,
        }
    }
    /// Delivers a PARSEABLE record (for the DLQ-route-failure test, where the
    /// dead-letter comes from the process closure, not a parse failure).
    fn without_parse_fail() -> Self {
        Self {
            next: Arc::new(AtomicU64::new(0)),
            committed_hwm: Arc::new(AtomicU64::new(u64::MAX)),
            good_payload: true,
        }
    }
}

impl crate::transport::TransportBase for OrderedReceiverBad {
    fn close(
        &self,
    ) -> impl std::future::Future<Output = crate::transport::TransportResult<()>> + Send {
        std::future::ready(Ok(()))
    }
    fn is_healthy(&self) -> bool {
        true
    }
    fn name(&self) -> &'static str {
        "ordered-bad-test"
    }
}

impl TransportReceiver for OrderedReceiverBad {
    type Token = crate::transport::memory::MemoryToken;

    fn recv(
        &self,
        _max: usize,
    ) -> impl std::future::Future<Output = crate::transport::TransportResult<WorkBatch<Self::Token>>>
    + Send {
        let next = Arc::clone(&self.next);
        let good = self.good_payload;
        async move {
            let seq = next.fetch_add(1, Ordering::Relaxed);
            if seq >= 1 {
                next.fetch_sub(1, Ordering::Relaxed);
                std::future::pending::<()>().await;
            }
            let payload = if good {
                Bytes::from_static(br#"{"ok":1}"#)
            } else {
                Bytes::from_static(b"not json {{{")
            };
            let record = Record {
                payload,
                key: None,
                headers: vec![],
                metadata: RecordMeta {
                    timestamp_ms: None,
                    format: PayloadFormat::Json,
                },
            };
            Ok(WorkBatch::new(
                vec![record],
                vec![crate::transport::memory::MemoryToken { seq }],
            ))
        }
    }

    async fn commit(&self, tokens: &[Self::Token]) -> crate::transport::TransportResult<()> {
        if let Some(max_seq) = tokens.iter().map(|t| t.seq).max() {
            self.committed_hwm.fetch_max(max_seq, Ordering::Relaxed);
        }
        Ok(())
    }
}
