// Project:   hyperi-rustlib
// File:      src/worker/engine/driver.rs
// Purpose:   Unified WorkBatch engine driver (get -> process -> send -> commit)
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # Unified `WorkBatch` engine driver
//!
//! The single run loop that the four legacy loops (`run` / `run_raw` /
//! `run_async` / `run_raw_async`) collapsed into when the spine flipped to
//! `WorkBatch` (Task 0.7b). It drives the canonical currency -- [`WorkBatch`] --
//! through one block at a time:
//!
//! ```text
//!   recv(max) -> WorkBatch        (recv now yields a WorkBatch natively)
//!     -> apply_workbatch_dlq_policy  (route/discard/reject inline-DLQ entries)
//!     -> lease_ingress_batch      (memory accounting on the block's bytes)
//!     -> process(WorkBatch)       (transforms parse ON DEMAND via codec::parse)
//!     -> sink(&out_batch).await   (async send of the whole block)
//!     -> commit per CommitMode    (at-least-once, AFTER the block is sent)
//! ```
//!
//! ## Why tokens live on the batch, not the record
//!
//! [`WorkBatch::commit_tokens`] are the INPUT source acks. They are decoupled
//! from `records.len()`, so a `process` that fans `N` records out to `2N` (or
//! collapses them) does NOT disturb the source acks. The driver commits EXACTLY
//! the input tokens after the whole out-batch is sent -- never `2N`, never per
//! output record. That invariant is the data-plane core; the fan-out
//! commit-correctness test proves it.
//!
//! ## Two parse modes (the hybrid)
//!
//! - [`run_workbatch`](BatchEngine::run_workbatch) -- the DEFAULT. The driver
//!   does NOT pre-parse. A transform that needs a field calls
//!   [`codec::parse`] on demand. Pass-through apps (receiver, raw forwarders)
//!   never pay a parse.
//!
//! - [`run_workbatch_parsed`](BatchEngine::run_workbatch_parsed) -- opt-in for
//!   hot pipelines. The driver pre-parses the whole block via `codec::parse`
//!   (SIMD JSON / native MsgPack) on the worker pool and hands the process
//!   closure a [`ParsedBatch`] -- records + their aligned `ParsedPayload`s + a
//!   shared [`FieldInterner`](super::FieldInterner) for hot routing-field
//!   dedup. This keeps the batch-parse + interner throughput win for apps that
//!   opt in.
//!
//! `process_mid_tier`, `process_raw` and `ParsedMessage` remain for the
//! in-process (non-run-loop) callers; only the four legacy run loops were
//! removed by the 0.7b flip.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::{BatchEngine, EngineError};
use crate::transport::codec::{self, ParsedPayload};
use crate::transport::{Record, TransportReceiver, WorkBatch};

/// When the driver commits the input source acks.
///
/// The `commit_tokens` carried on the [`WorkBatch`] ARE the input source acks
/// (Kafka offsets, fetch cursors, ...). This enum decides who fires them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitMode {
    /// At-least-once: after the sink returns `Ok` for the WHOLE out-batch, the
    /// engine calls `receiver.commit(&out_batch.commit_tokens)`. A sink error
    /// skips the commit so the block is re-delivered. This is the engine-commits
    /// behaviour of the former mid-tier / raw run loops, lifted onto the block.
    Auto,
    /// The sink owns the commit -- the engine does NOT commit. The sink is
    /// handed the block (which carries `commit_tokens`) and decides when to
    /// acknowledge (e.g. after a downstream flush). The deferred-commit shape of
    /// the former async run loop, lifted onto the block.
    SinkManaged,
}

/// A pre-parsed block for the opt-in
/// [`run_workbatch_parsed`](BatchEngine::run_workbatch_parsed) hot path.
///
/// Bundles the surviving [`Record`]s with their aligned [`ParsedPayload`]s
/// (`records[i]` parsed to `parsed[i]`), the input `commit_tokens`, any inline
/// DLQ entries carried forward, and a shared [`FieldInterner`](super::FieldInterner)
/// for hot routing-field dedup.
///
/// ## Parse-failure contract
///
/// `records` and `parsed` are aligned 1:1 and contain ONLY records that parsed
/// successfully. A record whose payload fails [`codec::parse`] is handled per
/// the engine's configured [`ParseErrorAction`](super::ParseErrorAction) -- the
/// same contract the legacy `process_mid_tier` honoured:
///
/// - [`Dlq`](super::ParseErrorAction::Dlq) (default): its bytes are appended to
///   [`dlq_entries`](Self::dlq_entries) with a `parse error: ...` reason
///   (no silent drop); the resulting [`WorkBatch`] inherits those entries and
///   the driver routes them through the DLQ policy before commit.
/// - [`Skip`](super::ParseErrorAction::Skip): the record is dropped (counted in
///   errors) -- a deliberate, configured drop, not a silent vanish.
/// - [`FailBatch`](super::ParseErrorAction::FailBatch): the whole block fails
///   terminally (no commit), consistent with the ack barrier.
///
/// The process closure therefore always sees a clean, fully-parsed view.
///
/// `commit_tokens` are the INPUT source acks and are carried through unchanged
/// regardless of how many records survived parsing -- the same fan-out-safe
/// token decoupling as [`WorkBatch`].
pub struct ParsedBatch<'a, T: crate::transport::CommitToken> {
    /// Records that parsed successfully (aligned 1:1 with [`parsed`](Self::parsed)).
    pub records: Vec<Record>,

    /// The parsed payloads, `parsed[i]` being `records[i]` decoded.
    pub parsed: Vec<ParsedPayload>,

    /// Input source acks for the whole block (decoupled from record count).
    pub commit_tokens: Vec<T>,

    /// Inline-DLQ entries: those carried in on the source batch PLUS any record
    /// that failed to parse (no-silent-drop).
    pub dlq_entries: Vec<crate::transport::filter::FilteredDlqEntry>,

    /// Shared interner for hot routing-field-name dedup. The first time a field
    /// name is seen it allocates an `Arc<str>`; later lookups are a refcount
    /// bump. Reused from the engine so dedup persists across blocks.
    pub interner: &'a super::FieldInterner,
}

impl<T: crate::transport::CommitToken> ParsedBatch<'_, T> {
    /// Number of successfully-parsed records.
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether there are no successfully-parsed records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Intern a routing-field name through the shared interner.
    ///
    /// Use this to dedup the routing-key field name once per block rather than
    /// re-allocating it per record.
    #[must_use]
    pub fn intern(&self, name: &str) -> std::sync::Arc<str> {
        self.interner.intern(name)
    }
}

/// Optional periodic ticker shared by the run-loops (flush timers, periodic
/// maintenance). Folds away the identical interval-setup + select-arm the four
/// loops would otherwise each carry. The ticker is COLD (fires on the order of
/// the flush interval, not per block), so this extraction does not touch the
/// hot recv path.
#[cfg(feature = "transport")]
struct LoopTicker<F> {
    interval: Option<tokio::time::Interval>,
    callback: Option<F>,
}

#[cfg(feature = "transport")]
impl<F, Fut> LoopTicker<F>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), EngineError>>,
{
    fn new(ticker: Option<(Duration, F)>) -> Self {
        // Start the first tick one period out (not immediately) so the loop
        // does not fire a tick before it has polled the source once.
        let interval = ticker
            .as_ref()
            .map(|(d, _)| tokio::time::interval_at(tokio::time::Instant::now() + *d, *d));
        Self {
            interval,
            callback: ticker.map(|(_, f)| f),
        }
    }

    /// Yield when the next tick is due, or never if no ticker is configured.
    /// Cancel-safe: `Interval::tick` is cancel-safe and the no-ticker arm pends,
    /// so this sits directly in `tokio::select!`.
    async fn wait(&mut self) {
        match self.interval.as_mut() {
            Some(i) => {
                i.tick().await;
            }
            None => std::future::pending::<()>().await,
        }
    }

    /// Run the ticker callback; a callback error is logged, not fatal.
    async fn fire(&mut self, label: &str) {
        if let Some(f) = self.callback.as_mut()
            && let Err(e) = f().await
        {
            tracing::error!(error = %e, ticker = label, "Ticker failed");
        }
    }
}

impl BatchEngine {
    /// Unified on-demand `WorkBatch` driver -- the default data-plane loop.
    ///
    /// Drives one [`WorkBatch`] at a time through `recv -> filter-DLQ policy ->
    /// ingress lease -> process -> sink -> commit`. The driver does NOT pre-parse:
    /// `process` reads fields on demand via [`codec::parse`]. Pass-through apps
    /// pay zero parse cost.
    ///
    /// - `process` runs on the loop task (cancellation-aware between awaits) and
    ///   may fan records out or in; it MUST preserve `commit_tokens` (use
    ///   [`WorkBatch::map_records`], which does so automatically).
    /// - `sink` is async and receives the WHOLE out-batch by reference.
    /// - `commit` selects [`CommitMode::Auto`] (engine commits after sink `Ok`)
    ///   or [`CommitMode::SinkManaged`] (sink owns commit).
    /// - `ticker` is an optional `(interval, fn)` that fires on the interval
    ///   inside the select loop (flush timers, periodic maintenance).
    ///
    /// Stops cleanly when `shutdown` is cancelled.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Transport`] if `recv` fails fatally,
    /// [`EngineError::FilterDlqUnrouted`] if inline-DLQ entries appear under the
    /// default [`FilterDlqPolicy::Reject`](super::FilterDlqPolicy::Reject), or
    /// the error returned by `process`.
    ///
    /// A sink error (and, under [`CommitMode::Auto`], a commit error) is
    /// TERMINAL: it stops the run loop and propagates. This is the ack barrier
    /// for the ORDERED/cumulative source commit (Kafka "commit up to offset N"):
    /// the failed block's tokens are NOT committed, and -- crucially -- no LATER
    /// block is fetched and committed past them, which would silently skip the
    /// never-sent records (data loss). On restart the source re-delivers from
    /// the last committed watermark, preserving at-least-once. The app owns
    /// restart/retry policy.
    #[cfg(feature = "transport")]
    #[allow(clippy::too_many_arguments)]
    pub async fn run_workbatch<R, P, Sink, SinkFut, Ticker, TickerFut>(
        &self,
        receiver: &R,
        shutdown: CancellationToken,
        process: P,
        mut sink: Sink,
        commit: CommitMode,
        ticker: Option<(Duration, Ticker)>,
    ) -> Result<(), EngineError>
    where
        R: TransportReceiver,
        P: Fn(WorkBatch<R::Token>) -> Result<WorkBatch<R::Token>, EngineError>,
        Sink: FnMut(&WorkBatch<R::Token>) -> SinkFut,
        SinkFut: std::future::Future<Output = Result<(), EngineError>>,
        Ticker: FnMut() -> TickerFut,
        TickerFut: std::future::Future<Output = Result<(), EngineError>>,
    {
        tracing::info!(
            chunk_size = self.config.max_chunk_size,
            commit = ?commit,
            ticker = ticker.is_some(),
            "BatchEngine (workbatch) starting"
        );

        let mut ticker = LoopTicker::new(ticker);

        loop {
            tokio::select! {
                biased;

                () = shutdown.cancelled() => {
                    tracing::info!("BatchEngine (workbatch) shutting down");
                    return Ok(());
                }

                () = ticker.wait() => ticker.fire("workbatch").await,

                recv_result = receiver.recv(self.config.max_chunk_size) => {
                    let work_batch = recv_result.map_err(EngineError::Transport)?;
                    let Some(batch) = self.ingest_workbatch(work_batch)? else {
                        continue;
                    };
                    self.drive_block(receiver, batch, &process, &mut sink, commit).await?;
                }
            }
        }
    }

    /// Streaming `WorkBatch` driver -- the opt-in peak-memory-bounded path.
    ///
    /// Identical loop shape to [`run_workbatch`](Self::run_workbatch), but each
    /// received block is processed in consecutive byte-budget-sized SUB-BLOCKS
    /// rather than all at once. Peak in-flight ingress memory is bounded to ONE
    /// sub-block (`~sub_block_bytes`) instead of the whole block: the per-sub-block
    /// ingress lease is dropped (releasing those bytes) BEFORE the next sub-block
    /// is leased and processed.
    ///
    /// The source acks for the WHOLE block are committed EXACTLY ONCE, after the
    /// FINAL sub-block's sink returns `Ok` (under [`CommitMode::Auto`]) -- so
    /// at-least-once is preserved: a sink error on any sub-block stops the block
    /// and skips the commit, so the WHOLE block is re-delivered. The sub-block
    /// views carry EMPTY `commit_tokens`; the batch's tokens are committed once at
    /// the end.
    ///
    /// `sub_block_bytes` is the target sum of `payload.len()` per sub-block (floor
    /// one record, so a record larger than the target is still its own sub-block
    /// and the loop never stalls). Taken as an explicit parameter so the path is
    /// testable in isolation; [`run_governed`](Self::run_governed) supplies it
    /// from the governor's byte budget.
    ///
    /// Fan-out WITHIN a sub-block's `process` is fine (records grow); the source
    /// acks are still the batch's input tokens, committed once at the end.
    ///
    /// # Errors
    ///
    /// Same as [`run_workbatch`](Self::run_workbatch).
    #[cfg(feature = "transport")]
    #[allow(clippy::too_many_arguments)]
    pub async fn run_workbatch_streaming<R, P, Sink, SinkFut, Ticker, TickerFut>(
        &self,
        receiver: &R,
        shutdown: CancellationToken,
        process: P,
        mut sink: Sink,
        commit: CommitMode,
        sub_block_bytes: u64,
        ticker: Option<(Duration, Ticker)>,
    ) -> Result<(), EngineError>
    where
        R: TransportReceiver,
        P: Fn(WorkBatch<R::Token>) -> Result<WorkBatch<R::Token>, EngineError>,
        Sink: FnMut(&WorkBatch<R::Token>) -> SinkFut,
        SinkFut: std::future::Future<Output = Result<(), EngineError>>,
        Ticker: FnMut() -> TickerFut,
        TickerFut: std::future::Future<Output = Result<(), EngineError>>,
    {
        // SinkManaged is unrepresentable on the streaming path: sub-block views
        // carry EMPTY tokens, so the sink never sees the block's source acks and
        // cannot own the commit. Fail fast at startup instead of freezing the
        // partition at runtime.
        if matches!(commit, CommitMode::SinkManaged) {
            return Err(EngineError::SinkManagedUnsupported);
        }

        tracing::info!(
            chunk_size = self.config.max_chunk_size,
            commit = ?commit,
            sub_block_bytes,
            ticker = ticker.is_some(),
            "BatchEngine (workbatch streaming) starting"
        );

        let mut ticker = LoopTicker::new(ticker);

        loop {
            tokio::select! {
                biased;

                () = shutdown.cancelled() => {
                    tracing::info!("BatchEngine (workbatch streaming) shutting down");
                    return Ok(());
                }

                () = ticker.wait() => ticker.fire("workbatch streaming").await,

                recv_result = receiver.recv(self.config.max_chunk_size) => {
                    let work_batch = recv_result.map_err(EngineError::Transport)?;
                    let Some(batch) = self.ingest_workbatch(work_batch)? else {
                        continue;
                    };
                    self.drive_block_streaming(
                        receiver, batch, &process, &mut sink, commit, sub_block_bytes,
                    )
                    .await?;
                }
            }
        }
    }

    /// Governed `WorkBatch` driver -- the default-ON self-regulation run path.
    ///
    /// This is what a self-regulating app calls instead of choosing between
    /// [`run_workbatch`](Self::run_workbatch) and
    /// [`run_workbatch_streaming`](Self::run_workbatch_streaming) by hand. It
    /// dispatches on whether the byte-budget lever is wired
    /// ([`set_byte_budget`](BatchEngine::set_byte_budget), done by
    /// `ServiceRuntime` when `self_regulation.enabled = true`):
    ///
    /// - **Governor ON** (budget wired): streams each received block in
    ///   sub-blocks sized to the CURRENT byte budget (re-read per block), bounds
    ///   peak in-flight memory to one sub-block, and folds each block's
    ///   `(bytes, process_time, ingest_interval)` into the AIMD loop via
    ///   [`observe`](crate::governor::ByteBudgetController::observe). The recv
    ///   `max` is capped to the budget's poll-safety
    ///   [`record_cap`](crate::governor::ByteBudgetController::record_cap).
    ///   While pressure is LOW the budget sits at its big start value, so the
    ///   block becomes a SINGLE sub-block -- no per-record overhead, behaviour
    ///   matches the whole-batch loop.
    /// - **Governor OFF** (no budget): delegates verbatim to
    ///   [`run_workbatch`](Self::run_workbatch) -- byte-identical to
    ///   pre-governor behaviour.
    ///
    /// The inbound GATE (Kafka pause-partitions / HTTP-gRPC 503) is wired
    /// SEPARATELY into the receive transport, not here -- this method is the
    /// driver-side lever (sub-block sizing + AIMD), the gate is the
    /// transport-side brake. The two share the same `UnifiedPressure`.
    ///
    /// # Errors
    ///
    /// Same as [`run_workbatch`](Self::run_workbatch).
    #[cfg(all(feature = "transport", feature = "governor"))]
    #[allow(clippy::too_many_arguments)]
    pub async fn run_governed<R, P, Sink, SinkFut, Ticker, TickerFut>(
        &self,
        receiver: &R,
        shutdown: CancellationToken,
        process: P,
        mut sink: Sink,
        commit: CommitMode,
        ticker: Option<(Duration, Ticker)>,
    ) -> Result<(), EngineError>
    where
        R: TransportReceiver,
        P: Fn(WorkBatch<R::Token>) -> Result<WorkBatch<R::Token>, EngineError>,
        Sink: FnMut(&WorkBatch<R::Token>) -> SinkFut,
        SinkFut: std::future::Future<Output = Result<(), EngineError>>,
        Ticker: FnMut() -> TickerFut,
        TickerFut: std::future::Future<Output = Result<(), EngineError>>,
    {
        // Governor OFF -> the original whole-batch loop, byte-for-byte. The
        // whole-batch path DOES support SinkManaged (the sink receives the full
        // block with its tokens), so the guard below must sit AFTER this
        // delegate, not before it.
        let Some(budget) = self.byte_budget.clone() else {
            return self
                .run_workbatch(receiver, shutdown, process, sink, commit, ticker)
                .await;
        };

        // Governor ON streams in sub-blocks whose views carry EMPTY tokens, so
        // the sink can never own a SinkManaged commit -- reject it at startup
        // rather than silently freeze the source offset. (Same guard as
        // run_workbatch_streaming, which this path bypasses.)
        if matches!(commit, CommitMode::SinkManaged) {
            return Err(EngineError::SinkManagedUnsupported);
        }

        tracing::info!(
            chunk_size = self.config.max_chunk_size,
            commit = ?commit,
            ticker = ticker.is_some(),
            start_byte_budget = budget.byte_budget(),
            "BatchEngine (governed) starting -- self-regulation ON"
        );

        let mut ticker = LoopTicker::new(ticker);

        // Track the previous block's arrival instant so we can feed the AIMD
        // loop a real ingest inter-arrival interval.
        let mut last_recv: Option<std::time::Instant> = None;

        loop {
            // The recv limits bound a single poll by BOTH:
            //   - the SMALLER of the config chunk size and the budget's
            //     poll-safety record cap (a tiny-record flood cannot blow the
            //     count even within the byte budget), AND
            //   - the CURRENT byte budget (re-read per block), so a single poll
            //     never RETAINS more than ~one budget's worth of inbound payload
            //     BEFORE the sub-block split. This is the fix for the
            //     "byte budget does not bound RECEIVE memory" gap: without the
            //     byte cap, `recv(max)` could build a WorkBatch (and, for the
            //     Kafka recv-arena, allocate one arena) far larger than the
            //     budget before any sub-block lease ran.
            let recv_limits = crate::transport::RecvLimits {
                max_records: self.config.max_chunk_size.min(budget.record_cap()),
                max_bytes: budget.byte_budget(),
            };

            tokio::select! {
                biased;

                () = shutdown.cancelled() => {
                    tracing::info!("BatchEngine (governed) shutting down");
                    return Ok(());
                }

                () = ticker.wait() => ticker.fire("governed").await,

                recv_result = receiver.recv_limited(recv_limits) => {
                    let now = std::time::Instant::now();
                    let ingest_interval = last_recv
                        .map(|prev| now.saturating_duration_since(prev))
                        .unwrap_or_default();
                    last_recv = Some(now);

                    let work_batch = recv_result.map_err(EngineError::Transport)?;
                    let block_bytes = work_batch.total_payload_bytes() as u64;
                    let Some(batch) = self.ingest_workbatch(work_batch)? else {
                        // Empty block: still fold the timing so a quiet pipeline
                        // can grow its budget back. No bytes -> treated as slack.
                        budget.observe(0, Duration::ZERO, ingest_interval);
                        continue;
                    };

                    // Re-read the budget for THIS block: low pressure -> big
                    // budget -> one sub-block (no overhead); high pressure ->
                    // shrunk budget -> peak in-flight bounded to one sub-block.
                    let sub_block_bytes = budget.byte_budget();

                    let process_start = std::time::Instant::now();
                    self.drive_block_streaming(
                        receiver, batch, &process, &mut sink, commit, sub_block_bytes,
                    )
                    .await?;
                    let process_time = process_start.elapsed();

                    // Fold the OBSERVED actual block bytes into the AIMD loop. A
                    // memory HARD override inside observe() shrinks immediately
                    // regardless of rho.
                    budget.observe(block_bytes, process_time, ingest_interval);

                    // Observability: surface the current budget + pressure as
                    // gauges so throttling is visible, not mysterious, AND the
                    // ACTUAL received block bytes so the gap between the budget
                    // (`self_regulation_byte_budget`) and reality (`recv_block_bytes`)
                    // is measurable -- a persistent overshoot means the recv byte
                    // cap is not holding. The gate edges (pause/resume) are
                    // logged by the ObservingActuator.
                    #[cfg(feature = "metrics")]
                    {
                        metrics::gauge!("self_regulation_byte_budget")
                            .set(budget.byte_budget() as f64);
                        metrics::gauge!("self_regulation_recv_block_bytes")
                            .set(block_bytes as f64);
                        // `self_regulation_` domain prefix: a bare `pressure_ratio`
                        // collides with MemoryGuard's and ScalingPressure's own
                        // pressure gauges on the same registry.
                        metrics::gauge!("self_regulation_pressure_ratio")
                            .set(budget.pressure().level());
                    }
                }
            }
        }
    }

    /// Unified pre-parsed `WorkBatch` driver -- the opt-in hot path.
    ///
    /// Identical loop shape to [`run_workbatch`](Self::run_workbatch), except the
    /// driver PRE-PARSES the whole block via [`codec::parse`] (SIMD JSON / native
    /// MsgPack) on the worker pool and hands `process_parsed` a [`ParsedBatch`]
    /// (records + aligned parsed payloads + shared
    /// [`FieldInterner`](super::FieldInterner)). This keeps
    /// the batch-parse + interner throughput win for apps that opt in.
    ///
    /// Records that fail to parse are handled per the configured
    /// [`ParseErrorAction`](super::ParseErrorAction) (Dlq -> dlq_entries, Skip ->
    /// drop+counted, FailBatch -> terminal no-commit) -- see [`ParsedBatch`] for
    /// the parse-failure contract. `process_parsed` returns the final
    /// [`WorkBatch`] and MUST preserve the input `commit_tokens`.
    ///
    /// # Errors
    ///
    /// Same as [`run_workbatch`](Self::run_workbatch).
    #[cfg(feature = "transport")]
    #[allow(clippy::too_many_arguments)]
    pub async fn run_workbatch_parsed<R, P, Sink, SinkFut, Ticker, TickerFut>(
        &self,
        receiver: &R,
        shutdown: CancellationToken,
        process_parsed: P,
        mut sink: Sink,
        commit: CommitMode,
        ticker: Option<(Duration, Ticker)>,
    ) -> Result<(), EngineError>
    where
        R: TransportReceiver,
        P: Fn(ParsedBatch<'_, R::Token>) -> Result<WorkBatch<R::Token>, EngineError>,
        Sink: FnMut(&WorkBatch<R::Token>) -> SinkFut,
        SinkFut: std::future::Future<Output = Result<(), EngineError>>,
        Ticker: FnMut() -> TickerFut,
        TickerFut: std::future::Future<Output = Result<(), EngineError>>,
    {
        tracing::info!(
            chunk_size = self.config.max_chunk_size,
            commit = ?commit,
            ticker = ticker.is_some(),
            "BatchEngine (workbatch parsed) starting"
        );

        let mut ticker = LoopTicker::new(ticker);

        loop {
            tokio::select! {
                biased;

                () = shutdown.cancelled() => {
                    tracing::info!("BatchEngine (workbatch parsed) shutting down");
                    return Ok(());
                }

                () = ticker.wait() => ticker.fire("workbatch parsed").await,

                recv_result = receiver.recv(self.config.max_chunk_size) => {
                    let recv_batch = recv_result.map_err(EngineError::Transport)?;
                    let Some(batch) = self.ingest_workbatch(recv_batch)? else {
                        continue;
                    };
                    // Wrap the parse-then-process so drive_block stays generic.
                    // parse_block honours ParseErrorAction: FailBatch surfaces a
                    // terminal EngineError here (no commit), Dlq carries entries
                    // forward for the driver to route, Skip drops silently+counted.
                    let parse = |b: WorkBatch<R::Token>| -> Result<WorkBatch<R::Token>, EngineError> {
                        let parsed = self.parse_block(b)?;
                        process_parsed(parsed)
                    };
                    self.drive_block(receiver, batch, &parse, &mut sink, commit).await?;
                }
            }
        }
    }

    /// Prepare a received [`WorkBatch`] for processing: route its inline-DLQ
    /// entries per the configured policy, then return the batch (with its
    /// `dlq_entries` stripped) ready for the process stage. Returns `None` when
    /// the block has no records (caller should `continue`).
    ///
    /// `recv` now yields a [`WorkBatch`] directly (Task 0.7b), so there is no
    /// `RecvBatch` round-trip: the inbound-filter DLQ entries arrive on
    /// [`WorkBatch::dlq_entries`] and are routed here via
    /// [`apply_workbatch_dlq_policy`](BatchEngine::apply_workbatch_dlq_policy)
    /// before processing. Memory accounting is performed in
    /// [`drive_block`](Self::drive_block).
    #[cfg(feature = "transport")]
    fn ingest_workbatch<T: crate::transport::CommitToken>(
        &self,
        batch: WorkBatch<T>,
    ) -> Result<Option<WorkBatch<T>>, EngineError> {
        // Route/discard/reject inline-DLQ entries per the configured policy --
        // never silently dropped. The batch comes back with its dlq_entries
        // consumed so the process stage sees a clean block.
        let batch = self.apply_workbatch_dlq_policy(batch)?;
        // Skip ONLY a truly-empty block. A block with no records but with
        // commit_tokens is the all-filtered case (every record was dropped/
        // DLQ-routed by an inbound filter): those acks must still be committed
        // so the source advances past the filtered records -- returning None
        // here would strand them (stalled Kafka offset / leaked Redis PEL).
        if batch.records.is_empty() && batch.commit_tokens.is_empty() {
            return Ok(None);
        }
        Ok(Some(batch))
    }

    /// Drive ONE block through `ingress lease -> process -> sink -> commit`.
    ///
    /// Shared by both [`run_workbatch`](Self::run_workbatch) and
    /// [`run_workbatch_parsed`](Self::run_workbatch_parsed); the only difference
    /// between the two is the `process` closure they pass.
    #[cfg(feature = "transport")]
    async fn drive_block<R, P, Sink, SinkFut>(
        &self,
        receiver: &R,
        batch: WorkBatch<R::Token>,
        process: &P,
        sink: &mut Sink,
        commit: CommitMode,
    ) -> Result<(), EngineError>
    where
        R: TransportReceiver,
        P: Fn(WorkBatch<R::Token>) -> Result<WorkBatch<R::Token>, EngineError>,
        Sink: FnMut(&WorkBatch<R::Token>) -> SinkFut,
        SinkFut: std::future::Future<Output = Result<(), EngineError>>,
    {
        // Account the in-flight ingress bytes against the MemoryGuard; the lease
        // releases on every exit path of this block (sink-error early return,
        // commit, ?-return) via Drop.
        #[cfg(feature = "memory")]
        let _ingress_lease = self.lease_ingress_batch(&batch);

        // process() may fan out / fan in; it preserves the input commit_tokens.
        // Capture the input ack count so a contract breach is LOGGED rather than
        // silently freezing the source offset: a closure that rebuilds its
        // output with WorkBatch::from_records (instead of map_records) drops the
        // tokens to zero, the Auto commit below commits `&[]`, and the partition
        // stalls with no diagnostic. One len() compare per block (not per
        // record) -- nil hot-path cost.
        let input_token_count = batch.commit_tokens.len();

        let mut out_batch = process(batch)?;

        if out_batch.commit_tokens.len() != input_token_count {
            tracing::warn!(
                input_tokens = input_token_count,
                output_tokens = out_batch.commit_tokens.len(),
                "process() changed the commit-token count -- the run contract is \
                 that process preserves source acks (transform records, not \
                 tokens). A drop toward zero will under-commit and stall the \
                 source offset; use map_records, not WorkBatch::from_records."
            );
        }

        // Route any parse/process-generated DLQ entries the out-batch carries,
        // through the SAME policy + route point as the inbound-filter entries
        // (apply_workbatch_dlq_policy). This happens AFTER process and BEFORE the
        // sink/commit, so a parse/process dead-letter can never vanish on the
        // path to a source commit. It is FALLIBLE: a route failure (Reject, or a
        // Route sink Err) is a terminal ack-barrier error -- the commit is
        // skipped and the whole block re-delivered, so no later ordered commit
        // advances past these undelivered dead-letters. Silent discard is opt-in
        // only (FilterDlqPolicy::DiscardWithMetric).
        if !out_batch.dlq_entries.is_empty() {
            let entries = std::mem::take(&mut out_batch.dlq_entries);
            if let Err(e) = self.route_dlq_entries(entries) {
                tracing::error!(error = %e, "DLQ route failed (workbatch) -- terminal, stopping the run loop (ack barrier)");
                return Err(e);
            }
        }

        // Sink the WHOLE out-batch. Commit only fires after this returns Ok.
        //
        // ACK BARRIER (at-least-once on an ORDERED commit): a sink failure is a
        // TERMINAL error -- it stops the run loop. The source commit is ordered
        // and CUMULATIVE (Kafka "commit up to offset N"); if the loop merely
        // logged and continued, the NEXT block's commit would advance the
        // committed watermark PAST this block's never-sent offsets, silently
        // skipping records (data loss). Stopping the loop leaves THIS block's
        // tokens uncommitted, so the source re-delivers from the last committed
        // watermark on restart -- no later block can commit ahead of the
        // failure. The app owns restart/retry policy; the engine never invents
        // a silent skip.
        // Skip the sink when there is nothing to send (e.g. every record in the
        // block was filtered out): the sink has no work, but the block's
        // commit_tokens -- which include the filtered records' acks -- must still
        // be committed below so the source advances past them. (The streaming
        // path gets this for free: a zero-record block runs zero sub-blocks and
        // still commits once at the end.)
        if !out_batch.records.is_empty()
            && let Err(e) = sink(&out_batch).await
        {
            tracing::error!(error = %e, "Sink failed (workbatch) -- terminal, stopping the run loop (ack barrier)");
            return Err(e);
        }

        // Commit EXACTLY the input source acks -- never the (possibly fanned-out)
        // output record count. This is the at-least-once block contract.
        match commit {
            CommitMode::Auto => {
                // A commit failure is ALSO a terminal ack-barrier failure: a
                // failed ordered commit must not be followed by a later block's
                // commit advancing the watermark past these uncommitted offsets.
                if let Err(e) = receiver.commit(&out_batch.commit_tokens).await {
                    tracing::error!(error = %e, "Commit failed (workbatch) -- terminal, stopping the run loop (ack barrier)");
                    return Err(EngineError::Transport(e));
                }
            }
            CommitMode::SinkManaged => {
                // The sink owns the commit -- the engine does not commit here.
            }
        }
        Ok(())
    }

    /// Drive ONE block through streaming sub-blocks: peak in-flight memory is
    /// bounded to ONE sub-block, the source acks commit once after the final
    /// sub-block.
    ///
    /// The whole batch's `commit_tokens` are carried ASIDE; each sub-block view is
    /// processed and sunk with EMPTY `commit_tokens` so a fan-out within a
    /// sub-block never multiplies the source acks. Each sub-block's ingress lease
    /// is dropped (releasing those bytes) BEFORE the next sub-block is leased, so
    /// the high-water lease never exceeds one sub-block's bytes -- NOT the whole
    /// block.
    ///
    /// On ANY sub-block sink error the block stops and the commit is skipped (the
    /// WHOLE block is re-delivered -- at-least-once). The error is TERMINAL: it
    /// propagates out and stops the run loop, so no LATER block's ordered commit
    /// can advance the cumulative watermark past these never-committed offsets
    /// (the ack barrier -- see [`drive_block`](Self::drive_block)). The commit
    /// (under [`CommitMode::Auto`]) fires EXACTLY ONCE after the final
    /// sub-block's sink returns `Ok`, with ALL the batch's input source acks; a
    /// commit failure is likewise terminal.
    #[cfg(feature = "transport")]
    async fn drive_block_streaming<R, P, Sink, SinkFut>(
        &self,
        receiver: &R,
        batch: WorkBatch<R::Token>,
        process: &P,
        sink: &mut Sink,
        commit: CommitMode,
        sub_block_bytes: u64,
    ) -> Result<(), EngineError>
    where
        R: TransportReceiver,
        P: Fn(WorkBatch<R::Token>) -> Result<WorkBatch<R::Token>, EngineError>,
        Sink: FnMut(&WorkBatch<R::Token>) -> SinkFut,
        SinkFut: std::future::Future<Output = Result<(), EngineError>>,
    {
        // Carry the WHOLE block's source acks aside; the sub-block views below
        // commit EMPTY tokens. The batch's tokens commit ONCE after the final
        // sub-block (at-least-once on the whole block). dlq_entries were already
        // routed by ingest_workbatch, so the block here carries none.
        let WorkBatch {
            records,
            commit_tokens,
            ..
        } = batch;

        // Drain into consecutive byte-budget-sized sub-blocks LAZILY (floor 1
        // record). `SubBlockDrain` yields ONE sub-block at a time as the loop
        // pulls it -- it never pre-materialises every sub-block vector up front,
        // so the only sub-block resident is the one currently being leased and
        // sunk (the streaming peak-memory contract holds for the SPLIT itself,
        // not just the lease).
        let mut sub_blocks = SubBlockDrain::new(records, sub_block_bytes);

        while let Some(sub_records) = sub_blocks.next_sub_block() {
            // Lease ONLY this sub-block's bytes. The lease releases on EVERY exit
            // path of this iteration (sink-error early return, ?-return, or the
            // end of the loop body) via Drop -- BEFORE the next sub-block leases.
            // Peak in-flight lease is therefore one sub-block, never the block.
            let sub_block: WorkBatch<R::Token> = WorkBatch::from_records(sub_records);
            #[cfg(feature = "memory")]
            let _sub_lease = self.lease_ingress_batch(&sub_block);

            // process() may fan out / fan in within the sub-block; it preserves
            // the (empty) commit_tokens of the sub-block view.
            let mut out_sub = process(sub_block)?;

            // Route any parse/process-generated DLQ entries this sub-block
            // carries BEFORE its sink -- same single policy + route point as the
            // whole-batch path and the inbound-filter entries. Fallible: a route
            // failure is terminal (ack barrier) so the commit for the WHOLE block
            // is skipped and it is re-delivered -- a dead-letter is never lost on
            // the path to a source commit.
            if !out_sub.dlq_entries.is_empty() {
                let entries = std::mem::take(&mut out_sub.dlq_entries);
                if let Err(e) = self.route_dlq_entries(entries) {
                    tracing::error!(error = %e, "DLQ route failed (workbatch streaming) -- terminal, stopping the run loop (ack barrier)");
                    return Err(e);
                }
            }

            // Sink this sub-block. A sink error stops the block and skips the
            // commit so the WHOLE block is re-delivered. TERMINAL (ack barrier):
            // propagate so the run loop stops -- a later block's ordered commit
            // must never advance the cumulative watermark past this block's
            // uncommitted offsets.
            if let Err(e) = sink(&out_sub).await {
                tracing::error!(error = %e, "Sink failed (workbatch streaming) -- terminal, stopping the run loop (ack barrier)");
                return Err(e);
            }
            // _sub_lease drops here -> bytes released before the next sub-block.
        }

        // All sub-blocks sunk Ok. Commit EXACTLY the input source acks ONCE.
        match commit {
            CommitMode::Auto => {
                // Commit failure is terminal (ack barrier) -- same reasoning as
                // the sink-error path above.
                if let Err(e) = receiver.commit(&commit_tokens).await {
                    tracing::error!(error = %e, "Commit failed (workbatch streaming) -- terminal, stopping the run loop (ack barrier)");
                    return Err(EngineError::Transport(e));
                }
            }
            CommitMode::SinkManaged => {
                // The sink owns the commit -- the engine does not commit here.
            }
        }
        Ok(())
    }

    /// Collect a [`SubBlockDrain`] into a `Vec<Vec<Record>>` (test convenience).
    ///
    /// The driver itself uses [`SubBlockDrain`] LAZILY and never collects all
    /// sub-blocks; this wrapper keeps the byte-split unit tests (which assert the
    /// sub-block shapes) ergonomic. Same splitting contract as
    /// [`SubBlockDrain::next_sub_block`].
    #[cfg(all(test, feature = "transport"))]
    fn split_into_sub_blocks(records: Vec<Record>, target_bytes: u64) -> Vec<Vec<Record>> {
        let mut drain = SubBlockDrain::new(records, target_bytes);
        let mut out = Vec::new();
        while let Some(sub) = drain.next_sub_block() {
            out.push(sub);
        }
        out
    }

    /// Pre-parse a whole [`WorkBatch`] into a [`ParsedBatch`] (the hot-path step),
    /// honouring the configured [`ParseErrorAction`](super::ParseErrorAction).
    ///
    /// Parses each record's payload via [`codec::parse`] on the worker pool
    /// (SIMD JSON / native MsgPack), keeping the surviving records aligned 1:1
    /// with their [`ParsedPayload`]s. A record that FAILS to parse is handled per
    /// the engine's `parse_error_action` -- the SAME contract the legacy
    /// `process_mid_tier` honoured (previously the parsed path hardcoded
    /// route-to-DLQ, ignoring the config):
    ///
    /// - [`Dlq`](super::ParseErrorAction::Dlq) (default): the record's bytes are
    ///   appended to the batch's `dlq_entries` (no silent drop) and counted in
    ///   errors + dlq. The driver routes those entries before commit.
    /// - [`Skip`](super::ParseErrorAction::Skip): the record is dropped, counted
    ///   in errors ONLY (a deliberate, configured drop -- not a silent vanish).
    /// - [`FailBatch`](super::ParseErrorAction::FailBatch): the whole block is
    ///   failed via [`EngineError::ParseBatchFailed`] -- terminal/no-commit,
    ///   consistent with the P1 ack barrier, so the block is re-delivered rather
    ///   than partially committed.
    ///
    /// Input `commit_tokens` and any carried-in `dlq_entries` are preserved.
    ///
    /// # Errors
    ///
    /// [`EngineError::ParseBatchFailed`] when a parse failure occurs under
    /// [`ParseErrorAction::FailBatch`](super::ParseErrorAction::FailBatch).
    #[cfg(feature = "transport")]
    fn parse_block<T: crate::transport::CommitToken>(
        &self,
        batch: WorkBatch<T>,
    ) -> Result<ParsedBatch<'_, T>, EngineError> {
        use super::ParseErrorAction;
        use crate::transport::PayloadFormat;

        let WorkBatch {
            records,
            commit_tokens,
            mut dlq_entries,
        } = batch;

        // Parse each record on the pool. The pool's map_owned applies the scaler
        // semaphore per item, so the parse phase obeys the CPU cap exactly as the
        // legacy parsed path does. map_owned preserves input order, so a record
        // and its parse result stay aligned without threading an explicit index.
        let parsed_each: Vec<(Record, Result<ParsedPayload, String>)> =
            self.pool.map_owned(records, |record| {
                let format: PayloadFormat = record.metadata.format;
                let result =
                    codec::parse(&record.payload, format).map_err(|e| format!("parse error: {e}"));
                (record, result)
            });

        let action = self.config.parse_error_action;
        let mut keep_records = Vec::new();
        let mut keep_parsed = Vec::new();
        for (record, result) in parsed_each {
            match result {
                Ok(payload) => {
                    keep_records.push(record);
                    keep_parsed.push(payload);
                }
                Err(reason) => match action {
                    ParseErrorAction::Dlq => {
                        // No silent drop: the unparseable record's bytes go to DLQ.
                        self.stats.incr_errors();
                        self.stats.incr_dlq();
                        dlq_entries.push(crate::transport::filter::FilteredDlqEntry {
                            payload: record.payload.to_vec(),
                            key: record.key.clone(),
                            reason,
                        });
                    }
                    ParseErrorAction::Skip => {
                        // Deliberate, configured drop -- counted in errors but NOT
                        // dead-lettered. This is opt-in loss, not a silent vanish.
                        self.stats.incr_errors();
                    }
                    ParseErrorAction::FailBatch => {
                        // Terminal: the whole block fails its commit (ack barrier).
                        self.stats.incr_errors();
                        return Err(EngineError::ParseBatchFailed(reason));
                    }
                },
            }
        }

        Ok(ParsedBatch {
            records: keep_records,
            parsed: keep_parsed,
            commit_tokens,
            dlq_entries,
            interner: &self.interner,
        })
    }

    /// Account a [`WorkBatch`]'s payload bytes against the [`MemoryGuard`],
    /// returning an RAII lease that releases them on drop.
    ///
    /// Drives the in-flight ingress accounting for the WorkBatch driver: the
    /// lease is taken in [`drive_block`](Self::drive_block) and releases the
    /// bytes on every block exit path via `Drop`.
    ///
    /// [`MemoryGuard`]: crate::memory::MemoryGuard
    #[cfg(feature = "memory")]
    pub(crate) fn lease_ingress_batch<T: crate::transport::CommitToken>(
        &self,
        batch: &WorkBatch<T>,
    ) -> Option<super::IngressLease<'_>> {
        let guard = self.memory_guard.as_ref()?;
        let bytes = batch.total_payload_bytes() as u64;
        guard.add_bytes(bytes);
        Some(super::IngressLease::new(guard, bytes))
    }
}

/// A LAZY sub-block drain: yields one consecutive byte-budget-sized sub-block
/// of [`Record`]s at a time, so the streaming driver never pre-materialises
/// every sub-block vector up front.
///
/// Each call to [`next_sub_block`](Self::next_sub_block) pulls records (in
/// order) from the source until the accumulated `payload.len()` would overshoot
/// `target_bytes`, then returns that sub-block; the remaining records stay
/// un-pulled in the source iterator. Splitting contract:
///
/// - records are kept in order;
/// - FLOOR of one record per sub-block: a record whose payload alone meets or
///   exceeds `target_bytes` is its own single-record sub-block (never stalls);
/// - `target_bytes` of `0` is treated as a floor of one record per sub-block;
/// - an exhausted source yields `None`.
///
/// The lazy shape matters: the previous `Vec<Vec<Record>>` allocated every
/// sub-block vector before the loop processed the first one. Here, at most ONE
/// sub-block vector is allocated at a time -- the one the loop is about to lease
/// and sink -- so the SPLIT no longer defeats the streaming peak-memory bound.
#[cfg(feature = "transport")]
struct SubBlockDrain {
    /// Source records, drained in order. `peeked` holds a record we pulled but
    /// could not fit into the sub-block being built (it starts the next one).
    iter: std::vec::IntoIter<Record>,
    peeked: Option<Record>,
    target_bytes: u64,
}

#[cfg(feature = "transport")]
impl SubBlockDrain {
    fn new(records: Vec<Record>, target_bytes: u64) -> Self {
        Self {
            iter: records.into_iter(),
            peeked: None,
            target_bytes,
        }
    }

    /// Yield the next consecutive sub-block, or `None` when the source is
    /// exhausted. Allocates exactly ONE sub-block `Vec` per call.
    fn next_sub_block(&mut self) -> Option<Vec<Record>> {
        // Start with the record carried over from the previous call (if any),
        // else pull the first record of this sub-block from the source.
        let first = self.peeked.take().or_else(|| self.iter.next())?;
        let mut current_bytes = first.payload.len() as u64;
        let mut current = vec![first];

        // Pull more records while they fit. Floor 1: we already took one record
        // above, so an oversized record is still its own sub-block.
        for record in self.iter.by_ref() {
            let record_bytes = record.payload.len() as u64;
            if current_bytes.saturating_add(record_bytes) > self.target_bytes {
                // Does not fit -- carry it to the next sub-block and stop here.
                self.peeked = Some(record);
                break;
            }
            current_bytes = current_bytes.saturating_add(record_bytes);
            current.push(record);
        }
        Some(current)
    }
}

#[cfg(all(test, feature = "transport-memory"))]
#[path = "driver_tests.rs"]
mod tests;
