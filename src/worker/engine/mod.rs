// Project:   hyperi-rustlib
// File:      src/worker/engine/mod.rs
// Purpose:   SIMD-optimised batch processing engine for DFE pipelines
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

pub mod config;
#[cfg(feature = "transport")]
pub mod driver;
pub mod intern;
pub mod metrics;
pub mod parse;
pub mod pre_route;
pub mod types;

pub use config::{BatchProcessingConfig, ParseErrorAction, PreRouteFilterConfig};
#[cfg(feature = "transport")]
pub use driver::{CommitMode, ParsedBatch};
pub use intern::FieldInterner;
pub use types::{MessageMetadata, ParsedMessage, PreRouteResult};

/// Errors returned by the [`BatchEngine`] `WorkBatch` drivers
/// ([`run_workbatch`](BatchEngine::run_workbatch) /
/// [`run_workbatch_parsed`](BatchEngine::run_workbatch_parsed)).
///
/// Only available when the `transport` feature is enabled.
#[cfg(feature = "transport")]
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// Transport receive or commit failed.
    #[error("transport error: {0}")]
    Transport(#[from] crate::TransportError),
    /// Sink callback returned an error.
    #[error("sink error: {0}")]
    Sink(String),
    /// Shutdown was requested via cancellation token.
    #[error("shutdown")]
    Shutdown,
    /// Inbound-filter DLQ entries appeared but no routing policy was configured
    /// (the default [`FilterDlqPolicy::Reject`]). Metrics are not delivery, so
    /// the engine fails fast rather than silently dropping dead-letters.
    #[error(
        "{0} inbound-filter DLQ entries were produced but no FilterDlqPolicy is \
         configured -- set a policy via BatchEngine::with_filter_dlq_policy \
         (Route to forward, or DiscardWithMetric to deliberately drop)"
    )]
    FilterDlqUnrouted(usize),
}

/// What a [`BatchEngine`] run loop does with inbound-filter DLQ entries
/// ([`RecvBatch::dlq_entries`](crate::transport::RecvBatch)).
///
/// Inbound `action: dlq` filters remove messages from the normal batch; those
/// entries must go somewhere. The default is [`Reject`](Self::Reject) so a
/// data-loss-shaped config never passes silently.
#[cfg(feature = "transport")]
#[derive(Clone, Default)]
pub enum FilterDlqPolicy {
    /// Fail the run loop ([`EngineError::FilterDlqUnrouted`]) if any DLQ entries
    /// appear. The safe default -- forces a deliberate choice.
    #[default]
    Reject,
    /// Deliberately discard DLQ entries, counting them in the
    /// `dfe_engine_filter_dlq_discarded_total` metric. Explicit opt-in.
    DiscardWithMetric,
    /// Hand each batch's DLQ entries to a sink (e.g. enqueue onto a DLQ
    /// transport, or `tokio::spawn` an async send). Called on the run loop, so
    /// keep it cheap -- offload slow work.
    Route(Arc<dyn Fn(Vec<crate::transport::filter::FilteredDlqEntry>) + Send + Sync>),
}

#[cfg(feature = "transport")]
impl std::fmt::Debug for FilterDlqPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reject => f.write_str("Reject"),
            Self::DiscardWithMetric => f.write_str("DiscardWithMetric"),
            Self::Route(_) => f.write_str("Route(..)"),
        }
    }
}

use std::sync::Arc;

use super::pool::AdaptiveWorkerPool;
use super::stats::PipelineStats;

use self::pre_route::filters_from_config;
// Pre-route + parse helpers are used only by the in-process process_* methods,
// which take the canonical transport Record and so are transport-gated.
#[cfg(feature = "transport")]
use self::pre_route::{PreRouteOutcome, apply_filters, extract_routing_field};
#[cfg(feature = "transport")]
use self::types::PayloadFormat;
use super::config::WorkerPoolConfig;

/// Core batch processing engine for DFE pipelines.
///
/// Provides two in-process processing modes (the run-loop drivers live in the
/// `driver` module, gated on the `transport` feature):
///
/// - [`process_mid_tier`](Self::process_mid_tier) -- parse JSON via SIMD, extract
///   known fields, apply pre-route filters, then parallel transform via rayon.
///   The standard path for most DFE apps (loader, archiver, transforms).
///
/// - [`process_raw`](Self::process_raw) -- skip parsing, apply pre-route on raw
///   bytes, then parallel transform via rayon. For apps that handle raw bytes
///   (receiver, binary protocols).
///
/// Both take the canonical [`Record`](crate::transport::Record) slice (the same
/// currency the [`WorkBatch`](crate::transport::WorkBatch) carries), chunk large
/// batches, and track stats atomically.
///
/// Inbound braking under memory pressure is no longer a blocking pause between
/// chunks (the retired `check_memory_pressure` proto-actuator). It is now the
/// self-regulation governor's job: the inbound GATE pauses the source transport
/// and the streaming byte-budget lever bounds peak in-flight memory. See
/// [`run_governed`](Self::run_governed). With the governor OFF, there is no
/// active brake -- that is the deliberate opt-out.
pub struct BatchEngine {
    config: BatchProcessingConfig,
    pool: Arc<AdaptiveWorkerPool>,
    stats: Arc<PipelineStats>,
    interner: Arc<FieldInterner>,
    filters: Vec<pre_route::PreRouteFilter>,
    #[cfg(feature = "memory")]
    memory_guard: Option<Arc<crate::memory::MemoryGuard>>,
    /// What the run loops do with inbound-filter DLQ entries. Default
    /// [`FilterDlqPolicy::Reject`] (no silent data loss).
    #[cfg(feature = "transport")]
    filter_dlq_policy: FilterDlqPolicy,
    /// Self-regulation byte-budget lever (`governor` feature). `None` (the
    /// default, and whenever self-regulation is OFF) keeps the engine on the
    /// whole-batch [`run_workbatch`](Self::run_workbatch) loop -- byte-identical
    /// to pre-governor behaviour. When wired (by `ServiceRuntime` when the
    /// governor is enabled), [`run_governed`](Self::run_governed) streams in
    /// budget-sized sub-blocks and feeds the AIMD loop per block.
    #[cfg(feature = "governor")]
    byte_budget: Option<Arc<crate::governor::ByteBudgetController>>,
}

impl BatchEngine {
    /// Create a standalone engine with its own worker pool.
    ///
    /// Uses `WorkerPoolConfig::default()` for the pool. Prefer
    /// [`with_pool`](Self::with_pool) when a `ServiceRuntime` pool exists.
    #[must_use]
    pub fn new(config: BatchProcessingConfig) -> Self {
        let pool = Arc::new(AdaptiveWorkerPool::new(WorkerPoolConfig::default()));
        Self::with_pool(pool, config)
    }

    /// Create an engine that reuses an existing worker pool.
    ///
    /// This is the preferred constructor when `ServiceRuntime` is available,
    /// as it avoids creating a second rayon thread pool.
    #[must_use]
    pub fn with_pool(pool: Arc<AdaptiveWorkerPool>, config: BatchProcessingConfig) -> Self {
        let known_refs: Vec<&str> = config.known_fields.iter().map(String::as_str).collect();
        let interner = Arc::new(FieldInterner::with_known_fields(&known_refs));
        let filters = filters_from_config(&config.pre_route_filters);
        Self {
            config,
            pool,
            stats: Arc::new(PipelineStats::new()),
            interner,
            filters,
            #[cfg(feature = "memory")]
            memory_guard: None,
            #[cfg(feature = "transport")]
            filter_dlq_policy: FilterDlqPolicy::default(),
            #[cfg(feature = "governor")]
            byte_budget: None,
        }
    }

    /// Wire the self-regulation byte-budget lever (`governor` feature).
    ///
    /// Called by `ServiceRuntime::build()` when the governor is enabled. Once
    /// wired, [`run_governed`](Self::run_governed) streams the input in
    /// budget-sized sub-blocks and drives the AIMD loop per block. Without it
    /// (governor off), `run_governed` falls back to the whole-batch loop.
    #[cfg(feature = "governor")]
    pub fn set_byte_budget(&mut self, budget: Arc<crate::governor::ByteBudgetController>) {
        self.byte_budget = Some(budget);
    }

    /// Whether the self-regulation byte-budget lever is wired (`governor`
    /// feature). When `false`, [`run_governed`](Self::run_governed) is the
    /// whole-batch loop.
    #[cfg(feature = "governor")]
    #[must_use]
    pub fn is_self_regulated(&self) -> bool {
        self.byte_budget.is_some()
    }

    /// Set the policy for inbound-filter DLQ entries in the run loops.
    ///
    /// Default is [`FilterDlqPolicy::Reject`] -- the run loop errors if an
    /// inbound `action: dlq` filter produces entries and no routing is set, so
    /// dead-letters are never silently dropped.
    #[cfg(feature = "transport")]
    #[must_use]
    pub fn with_filter_dlq_policy(mut self, policy: FilterDlqPolicy) -> Self {
        self.filter_dlq_policy = policy;
        self
    }

    /// Load configuration from the cascade and create a standalone engine.
    ///
    /// # Errors
    ///
    /// Returns `ConfigError` if the cascade contains invalid data.
    pub fn from_cascade(key: &str) -> Result<Self, crate::config::ConfigError> {
        let config = BatchProcessingConfig::from_cascade(key)?;
        Ok(Self::new(config))
    }

    /// Pipeline statistics (atomic, lock-free).
    #[must_use]
    pub fn stats(&self) -> &Arc<PipelineStats> {
        &self.stats
    }

    /// Underlying worker pool.
    #[must_use]
    pub fn pool(&self) -> &Arc<AdaptiveWorkerPool> {
        &self.pool
    }

    /// Engine configuration.
    #[must_use]
    pub fn config(&self) -> &BatchProcessingConfig {
        &self.config
    }

    /// Auto-wire engine with infrastructure components.
    ///
    /// Called by `ServiceRuntime::build()`. Apps never call this directly.
    pub fn auto_wire(
        &mut self,
        metrics_manager: &crate::metrics::MetricsManager,
        #[cfg(feature = "memory")] memory_guard: Option<&Arc<crate::memory::MemoryGuard>>,
    ) {
        metrics::register(metrics_manager, &self.config);

        #[cfg(feature = "memory")]
        if let Some(guard) = memory_guard {
            self.memory_guard = Some(Arc::clone(guard));
        }
    }

    /// Test-only: wire a `MemoryGuard` directly so memory-lease behaviour can be
    /// exercised without a full `auto_wire`.
    #[cfg(all(test, feature = "memory"))]
    pub(crate) fn set_memory_guard_for_test(&mut self, guard: Arc<crate::memory::MemoryGuard>) {
        self.memory_guard = Some(guard);
    }

    /// Parse, filter, and transform a batch of raw messages.
    ///
    /// Pipeline phases per chunk:
    /// 1. **Pre-route** -- SIMD field extraction + filter evaluation (sequential, ~100 ns/msg)
    /// 2. **Parse** -- `sonic_rs::from_slice` + known-field extraction (sequential, ~1-5 µs/msg)
    /// 3. **Transform** -- user closure via rayon `par_iter_mut` (parallel)
    ///
    /// Results contain one entry per non-filtered message. Filtered messages are
    /// silently removed (their commit tokens remain accessible via the original
    /// slice). DLQ'd and parse-error messages produce `Err` entries.
    #[cfg(feature = "transport")]
    pub fn process_mid_tier<O, E, F>(
        &self,
        messages: &[crate::transport::Record],
        transform: F,
    ) -> Vec<Result<O, E>>
    where
        O: Send,
        E: Send + From<String>,
        F: Fn(&mut ParsedMessage) -> Result<O, E> + Sync,
    {
        if messages.is_empty() {
            return Vec::new();
        }

        let chunk_size = if self.config.max_chunk_size == 0 {
            messages.len()
        } else {
            self.config.max_chunk_size
        };

        let has_routing = self.config.routing_field.is_some();
        let mut all_results = Vec::with_capacity(messages.len());

        for chunk in messages.chunks(chunk_size) {
            self.stats.add_received(chunk.len() as u64);

            // Accumulate bytes received
            let chunk_bytes: u64 = chunk.iter().map(|m| m.payload.len() as u64).sum();
            self.stats.add_bytes_received(chunk_bytes);

            // Phase 1 + 2: Pre-route and parse, building ParsedMessage vec.
            // Track which messages are included (not filtered).
            let mut parsed_msgs: Vec<Result<ParsedMessage, String>> =
                Vec::with_capacity(chunk.len());

            for msg in chunk {
                // Phase 1: Pre-route
                if has_routing {
                    let field_name = self.config.routing_field.as_ref().expect("checked above");
                    let extraction = extract_routing_field(&msg.payload, field_name);
                    let outcome = apply_filters(&extraction, &self.filters);

                    match outcome {
                        PreRouteOutcome::Continue => {}
                        PreRouteOutcome::Filtered => {
                            self.stats.incr_filtered();
                            continue; // skip this message entirely
                        }
                        PreRouteOutcome::Dlq(reason) => {
                            self.stats.incr_dlq();
                            self.stats.incr_errors();
                            parsed_msgs.push(Err(reason));
                            continue;
                        }
                    }
                }

                // Phase 2: Parse. The Record carries the transport PayloadFormat;
                // convert it to the engine's local enum, resolving Auto.
                let format: PayloadFormat = match PayloadFormat::from(msg.metadata.format) {
                    PayloadFormat::Auto => PayloadFormat::detect(&msg.payload),
                    other => other,
                };

                match parse::parse_payload(&msg.payload, format) {
                    Ok(value) => {
                        let extracted = self.interner.extract_known(&value);
                        // Rebuild a MessageMetadata for ParsedMessage. Commit
                        // tokens live on the WorkBatch, not on individual records.
                        let metadata = MessageMetadata {
                            timestamp_ms: msg.metadata.timestamp_ms,
                            format,
                        };
                        parsed_msgs.push(Ok(ParsedMessage::Parsed {
                            value,
                            raw: msg.payload.clone(),
                            format,
                            key: msg.key.clone(),
                            headers: msg.headers.clone(),
                            metadata,
                            extracted,
                        }));
                    }
                    Err(e) => {
                        self.stats.incr_errors();
                        match self.config.parse_error_action {
                            ParseErrorAction::Dlq => {
                                self.stats.incr_dlq();
                                parsed_msgs.push(Err(format!("parse error: {e}")));
                            }
                            ParseErrorAction::Skip => {
                                // Counted in errors above, not added to results
                            }
                            ParseErrorAction::FailBatch => {
                                // Return all accumulated results + this error,
                                // then stop processing the chunk.
                                parsed_msgs.push(Err(format!("parse error (fail_batch): {e}")));
                                let results: Vec<Result<O, E>> = parsed_msgs
                                    .into_iter()
                                    .map(|r| match r {
                                        Ok(_) => Err(E::from(
                                            "batch failed due to parse error".to_string(),
                                        )),
                                        Err(reason) => Err(E::from(reason)),
                                    })
                                    .collect();
                                all_results.extend(results);
                                return all_results;
                            }
                        }
                    }
                }
            }

            // Phase 3: Parallel transform via rayon.
            // Split into ok/err: transform only the Ok entries.
            let mut indexed: Vec<(usize, Result<ParsedMessage, String>)> =
                parsed_msgs.into_iter().enumerate().collect();

            // Separate errors from parseable messages
            let mut chunk_results: Vec<(usize, Result<O, E>)> = Vec::with_capacity(indexed.len());
            let mut to_transform: Vec<(usize, ParsedMessage)> = Vec::with_capacity(indexed.len());

            for (idx, item) in indexed.drain(..) {
                match item {
                    Ok(pm) => to_transform.push((idx, pm)),
                    Err(reason) => chunk_results.push((idx, Err(E::from(reason)))),
                }
            }

            // Parallel transform, throttled by the scaler target (map_owned
            // applies the semaphore per item -- unlike the old install() path,
            // which bypassed it and let the parsed path ignore the CPU cap).
            let transformed: Vec<(usize, Result<O, E>)> =
                self.pool.map_owned(to_transform, |(idx, mut pm)| {
                    let result = transform(&mut pm);
                    (idx, result)
                });

            chunk_results.extend(transformed);

            // Sort by original index to preserve order
            chunk_results.sort_by_key(|(idx, _)| *idx);

            // Update stats
            let ok_count = chunk_results.iter().filter(|(_, r)| r.is_ok()).count();
            self.stats.add_processed(ok_count as u64);

            all_results.extend(chunk_results.into_iter().map(|(_, r)| r));
        }

        all_results
    }

    /// Pre-route and transform a batch of records without parsing.
    ///
    /// The transform closure receives immutable [`Record`](crate::transport::Record)
    /// references. Use this for apps that handle raw bytes directly (e.g. receiver
    /// forwarding).
    #[cfg(feature = "transport")]
    pub fn process_raw<O, E, F>(
        &self,
        messages: &[crate::transport::Record],
        transform: F,
    ) -> Vec<Result<O, E>>
    where
        O: Send,
        E: Send + From<String>,
        F: Fn(&crate::transport::Record) -> Result<O, E> + Sync,
    {
        if messages.is_empty() {
            return Vec::new();
        }

        let chunk_size = if self.config.max_chunk_size == 0 {
            messages.len()
        } else {
            self.config.max_chunk_size
        };

        let has_routing = self.config.routing_field.is_some();
        let mut all_results = Vec::with_capacity(messages.len());

        for chunk in messages.chunks(chunk_size) {
            self.stats.add_received(chunk.len() as u64);

            let chunk_bytes: u64 = chunk.iter().map(|m| m.payload.len() as u64).sum();
            self.stats.add_bytes_received(chunk_bytes);

            // Phase 1: Pre-route filter
            let to_process: Vec<&crate::transport::Record> = if has_routing {
                let field_name = self.config.routing_field.as_ref().expect("checked above");
                let mut passed = Vec::with_capacity(chunk.len());
                for msg in chunk {
                    let extraction = extract_routing_field(&msg.payload, field_name);
                    let outcome = apply_filters(&extraction, &self.filters);
                    match outcome {
                        PreRouteOutcome::Continue => passed.push(msg),
                        PreRouteOutcome::Filtered => {
                            self.stats.incr_filtered();
                        }
                        PreRouteOutcome::Dlq(reason) => {
                            self.stats.incr_dlq();
                            self.stats.incr_errors();
                            all_results.push(Err(E::from(reason)));
                        }
                    }
                }
                passed
            } else {
                chunk.iter().collect()
            };

            // Phase 2: Parallel transform via process_batch
            let results = self.pool.process_batch(&to_process, |msg| transform(msg));

            let ok_count = results.iter().filter(|r| r.is_ok()).count();
            self.stats.add_processed(ok_count as u64);

            all_results.extend(results);
        }

        all_results
    }

    /// Apply the [`FilterDlqPolicy`] to a received [`WorkBatch`](crate::transport::WorkBatch),
    /// routing/discarding/rejecting its inline-DLQ entries per the policy and
    /// returning the batch with `dlq_entries` consumed.
    ///
    /// Now that `recv` yields a `WorkBatch` directly (Task 0.7b), the
    /// inbound-filter DLQ entries arrive on
    /// [`WorkBatch::dlq_entries`](crate::transport::WorkBatch) rather than on a
    /// `RecvBatch`. Records are never touched -- only the DLQ entries are routed
    /// -- so dead-letters are never silently dropped.
    ///
    /// # Errors
    ///
    /// [`EngineError::FilterDlqUnrouted`] when entries appear under
    /// [`FilterDlqPolicy::Reject`].
    #[cfg(feature = "transport")]
    fn apply_workbatch_dlq_policy<T: crate::transport::CommitToken>(
        &self,
        mut batch: crate::transport::WorkBatch<T>,
    ) -> Result<crate::transport::WorkBatch<T>, EngineError> {
        if !batch.dlq_entries.is_empty() {
            let entries = std::mem::take(&mut batch.dlq_entries);
            match &self.filter_dlq_policy {
                FilterDlqPolicy::Reject => {
                    return Err(EngineError::FilterDlqUnrouted(entries.len()));
                }
                FilterDlqPolicy::DiscardWithMetric => {
                    #[cfg(feature = "metrics")]
                    ::metrics::counter!("dfe_engine_filter_dlq_discarded_total")
                        .increment(entries.len() as u64);
                }
                FilterDlqPolicy::Route(sink) => sink(entries),
            }
        }
        Ok(batch)
    }
}

/// RAII lease over in-flight ingress bytes tracked by a [`MemoryGuard`].
///
/// Created by the WorkBatch driver's `lease_ingress_batch`; releases the
/// accounted bytes back to the guard on drop so no block exit path can leak
/// the reservation.
///
/// [`MemoryGuard`]: crate::memory::MemoryGuard
#[cfg(feature = "memory")]
pub(crate) struct IngressLease<'a> {
    guard: &'a crate::memory::MemoryGuard,
    bytes: u64,
}

#[cfg(feature = "memory")]
impl<'a> IngressLease<'a> {
    /// Construct a lease over already-accounted ingress bytes. The caller must
    /// have already called `guard.add_bytes(bytes)`; `Drop` releases them. Used
    /// by the WorkBatch driver's `lease_ingress_batch`.
    fn new(guard: &'a crate::memory::MemoryGuard, bytes: u64) -> Self {
        Self { guard, bytes }
    }
}

#[cfg(feature = "memory")]
impl Drop for IngressLease<'_> {
    fn drop(&mut self) {
        self.guard.release(self.bytes);
    }
}

impl std::fmt::Debug for BatchEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("BatchEngine");
        s.field("config", &self.config)
            .field("pool_max_threads", &self.pool.max_threads())
            .field("stats", &self.stats.snapshot())
            .field("interner_len", &self.interner.len())
            .field("filters", &self.filters);
        #[cfg(feature = "memory")]
        s.field("memory_guard", &self.memory_guard.is_some());
        #[cfg(feature = "transport")]
        s.field("filter_dlq_policy", &self.filter_dlq_policy);
        #[cfg(feature = "governor")]
        s.field("self_regulated", &self.byte_budget.is_some());
        s.finish()
    }
}

#[cfg(all(test, feature = "transport"))]
mod engine_tests {
    use super::*;
    use crate::transport::{PayloadFormat as TPayloadFormat, Record, RecordMeta};
    use bytes::Bytes;

    fn make_json_messages(n: usize) -> Vec<Record> {
        (0..n)
            .map(|i| Record {
                payload: Bytes::from(format!(r#"{{"_table":"events","id":{i}}}"#)),
                key: None,
                headers: vec![],
                metadata: RecordMeta {
                    timestamp_ms: None,
                    format: TPayloadFormat::Json,
                },
            })
            .collect()
    }

    fn default_engine() -> BatchEngine {
        BatchEngine::new(BatchProcessingConfig::default())
    }

    #[cfg(feature = "transport")]
    #[test]
    fn filter_dlq_policy_routes_discards_or_rejects() {
        use crate::transport::WorkBatch;
        use crate::transport::filter::FilteredDlqEntry;
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        // Minimal CommitToken for the generic helper.
        #[derive(Clone, Debug)]
        struct TestTok;
        impl std::fmt::Display for TestTok {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("test")
            }
        }
        impl crate::transport::CommitToken for TestTok {}

        let entry = || FilteredDlqEntry {
            payload: b"x".to_vec(),
            key: None,
            reason: "r".to_string(),
        };
        let batch_with = |n: usize| {
            WorkBatch::<TestTok>::from_records(vec![])
                .with_dlq_entries((0..n).map(|_| entry()).collect())
        };

        // Reject (default): any DLQ entries -> fail fast, not silent drop.
        let eng = default_engine();
        assert!(matches!(
            eng.apply_workbatch_dlq_policy(batch_with(1)),
            Err(EngineError::FilterDlqUnrouted(1))
        ));
        // Reject + no entries -> ok (batch passes through with no DLQ entries).
        let passed = eng
            .apply_workbatch_dlq_policy(WorkBatch::<TestTok>::from_records(vec![]))
            .expect("no entries -> ok");
        assert!(passed.dlq_entries.is_empty());

        // DiscardWithMetric -> ok (deliberately dropped); entries consumed.
        let eng = default_engine().with_filter_dlq_policy(FilterDlqPolicy::DiscardWithMetric);
        let passed = eng
            .apply_workbatch_dlq_policy(batch_with(1))
            .expect("discard -> ok");
        assert!(
            passed.dlq_entries.is_empty(),
            "entries consumed after routing"
        );

        // Route -> the sink receives every entry.
        let seen = StdArc::new(AtomicUsize::new(0));
        let s = StdArc::clone(&seen);
        let eng = default_engine().with_filter_dlq_policy(FilterDlqPolicy::Route(StdArc::new(
            move |e: Vec<FilteredDlqEntry>| {
                s.fetch_add(e.len(), Ordering::Relaxed);
            },
        )));
        let passed = eng
            .apply_workbatch_dlq_policy(batch_with(2))
            .expect("route -> ok");
        assert!(passed.dlq_entries.is_empty());
        assert_eq!(
            seen.load(Ordering::Relaxed),
            2,
            "Route sink received all entries"
        );
    }

    /// Build a `WorkBatch` of `n` JSON records (no commit tokens) for the
    /// `WorkBatch`-shaped ingress-lease tests.
    #[cfg(all(feature = "memory", feature = "transport"))]
    fn make_record_batch(n: usize) -> crate::transport::WorkBatch<TestTok> {
        use crate::transport::{PayloadFormat, Record, RecordMeta};
        let records = (0..n)
            .map(|i| Record {
                payload: Bytes::from(format!(r#"{{"_table":"events","id":{i}}}"#)),
                key: None,
                headers: vec![],
                metadata: RecordMeta {
                    timestamp_ms: None,
                    format: PayloadFormat::Json,
                },
            })
            .collect();
        crate::transport::WorkBatch::from_records(records)
    }

    /// Minimal commit token for the `WorkBatch` ingress-lease tests.
    #[cfg(all(feature = "memory", feature = "transport"))]
    #[derive(Debug, Clone)]
    struct TestTok;
    #[cfg(all(feature = "memory", feature = "transport"))]
    impl std::fmt::Display for TestTok {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("test")
        }
    }
    #[cfg(all(feature = "memory", feature = "transport"))]
    impl crate::transport::CommitToken for TestTok {}

    #[cfg(all(feature = "memory", feature = "transport"))]
    #[test]
    fn ingress_lease_accounts_and_releases() {
        use crate::memory::{MemoryGuard, MemoryGuardConfig};

        let mut engine = default_engine();
        let guard = Arc::new(MemoryGuard::new(MemoryGuardConfig {
            limit_bytes: 1024 * 1024,
            ..Default::default()
        }));
        engine.memory_guard = Some(Arc::clone(&guard));

        let batch = make_record_batch(10);
        let expected = batch.total_payload_bytes() as u64;
        assert_eq!(guard.current_bytes(), 0, "starts at zero");

        {
            let _lease = engine.lease_ingress_batch(&batch).expect("guard present");
            assert_eq!(
                guard.current_bytes(),
                expected,
                "bytes accounted while lease held"
            );
        }
        // Lease dropped -> bytes released.
        assert_eq!(guard.current_bytes(), 0, "bytes released on drop");
    }

    #[cfg(all(feature = "memory", feature = "transport"))]
    #[test]
    fn ingress_lease_none_without_guard() {
        let engine = default_engine();
        let batch = make_record_batch(5);
        assert!(
            engine.lease_ingress_batch(&batch).is_none(),
            "no lease when no guard wired"
        );
    }

    #[test]
    fn process_mid_tier_basic() {
        let engine = default_engine();
        let msgs = make_json_messages(100);

        let results: Vec<Result<String, String>> = engine.process_mid_tier(&msgs, |pm| {
            Ok(pm
                .field("_table")
                .and_then(|v| sonic_rs::JsonValueTrait::as_str(v))
                .unwrap_or("unknown")
                .to_string())
        });

        assert_eq!(results.len(), 100);
        assert!(results.iter().all(|r| r.is_ok()));
        assert_eq!(results[0].as_ref().unwrap(), "events");
    }

    #[test]
    fn process_mid_tier_parse_error() {
        let engine = default_engine();
        let mut msgs = make_json_messages(2);
        // Insert an invalid JSON message
        msgs.insert(
            1,
            Record {
                payload: Bytes::from_static(b"not json {{{"),
                key: None,
                headers: vec![],
                metadata: RecordMeta {
                    timestamp_ms: None,
                    format: TPayloadFormat::Json,
                },
            },
        );

        let results: Vec<Result<String, String>> =
            engine.process_mid_tier(&msgs, |pm| Ok(pm.raw_payload().len().to_string()));

        // 2 successful + 1 error (DLQ by default)
        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert!(results[1].is_err());
        assert!(results[1].as_ref().unwrap_err().contains("parse error"));
        assert!(results[2].is_ok());
    }

    #[test]
    fn process_mid_tier_empty_batch() {
        let engine = default_engine();
        let results: Vec<Result<(), String>> = engine.process_mid_tier(&[], |_| Ok(()));
        assert!(results.is_empty());
    }

    #[test]
    fn process_mid_tier_respects_chunk_size() {
        let config = BatchProcessingConfig {
            max_chunk_size: 50,
            ..Default::default()
        };
        let engine = BatchEngine::new(config);
        let msgs = make_json_messages(120);

        let results: Vec<Result<usize, String>> =
            engine.process_mid_tier(&msgs, |pm| Ok(pm.raw_payload().len()));

        assert_eq!(results.len(), 120);
        assert!(results.iter().all(|r| r.is_ok()));
        // Stats should show 120 received across 3 chunks (50+50+20)
        let snap = engine.stats().snapshot();
        assert_eq!(snap.received, 120);
    }

    #[test]
    fn stats_updated_after_processing() {
        let engine = default_engine();
        let msgs = make_json_messages(10);

        let _results: Vec<Result<(), String>> = engine.process_mid_tier(&msgs, |_| Ok(()));

        let snap = engine.stats().snapshot();
        assert_eq!(snap.received, 10);
        assert_eq!(snap.processed, 10);
        assert_eq!(snap.errors, 0);
        assert_eq!(snap.filtered, 0);
    }

    #[test]
    fn process_raw_passthrough() {
        let engine = default_engine();
        let msgs = make_json_messages(50);

        let results: Vec<Result<usize, String>> =
            engine.process_raw(&msgs, |msg| Ok(msg.payload.len()));

        assert_eq!(results.len(), 50);
        assert!(results.iter().all(|r| r.is_ok()));
        // All JSON messages have the same format: {"_table":"events","id":N}
        assert!(results[0].as_ref().unwrap() > &0);

        let snap = engine.stats().snapshot();
        assert_eq!(snap.received, 50);
        assert_eq!(snap.processed, 50);
    }

    #[test]
    fn process_mid_tier_with_pre_route() {
        let config = BatchProcessingConfig {
            routing_field: Some("_table".to_string()),
            pre_route_filters: vec![config::PreRouteFilterConfig::DlqFieldValue {
                field: "_table".to_string(),
                value: "poison".to_string(),
            }],
            ..Default::default()
        };
        let engine = BatchEngine::new(config);

        let mut msgs = make_json_messages(3);
        // Replace middle message with a poison value
        msgs[1] = Record {
            payload: Bytes::from(r#"{"_table":"poison","id":999}"#),
            key: None,
            headers: vec![],
            metadata: RecordMeta {
                timestamp_ms: None,
                format: TPayloadFormat::Json,
            },
        };

        let results: Vec<Result<String, String>> = engine.process_mid_tier(&msgs, |pm| {
            Ok(pm
                .field("_table")
                .and_then(|v| sonic_rs::JsonValueTrait::as_str(v))
                .unwrap_or("?")
                .to_string())
        });

        // 2 ok + 1 DLQ error
        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert!(results[1].is_err());
        assert!(results[1].as_ref().unwrap_err().contains("DLQ"));
        assert!(results[2].is_ok());

        let snap = engine.stats().snapshot();
        assert_eq!(snap.dlq, 1);
        assert_eq!(snap.errors, 1);
    }

    #[test]
    fn process_mid_tier_filtered_not_in_results() {
        let config = BatchProcessingConfig {
            routing_field: Some("_table".to_string()),
            pre_route_filters: vec![config::PreRouteFilterConfig::DropFieldMissing {
                field: "_table".to_string(),
            }],
            ..Default::default()
        };
        let engine = BatchEngine::new(config);

        let mut msgs = make_json_messages(3);
        // Replace middle message with one missing _table
        msgs[1] = Record {
            payload: Bytes::from(r#"{"host":"web1"}"#),
            key: None,
            headers: vec![],
            metadata: RecordMeta {
                timestamp_ms: None,
                format: TPayloadFormat::Json,
            },
        };

        let results: Vec<Result<String, String>> =
            engine.process_mid_tier(&msgs, |_pm| Ok("ok".to_string()));

        // Filtered messages are removed -- only 2 results
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.is_ok()));

        let snap = engine.stats().snapshot();
        assert_eq!(snap.filtered, 1);
        assert_eq!(snap.received, 3);
    }

    #[test]
    fn from_cascade_creates_engine() {
        let engine = BatchEngine::from_cascade("batch_processing").unwrap();
        assert_eq!(engine.config().max_chunk_size, 10_000);
    }

    #[test]
    fn accessors_return_expected_types() {
        let engine = default_engine();
        let _stats = engine.stats();
        let _pool = engine.pool();
        let _config = engine.config();
        assert_eq!(engine.stats().snapshot().received, 0);
    }

    #[test]
    fn auto_wire_does_not_panic() {
        let mut engine = default_engine();
        let mgr = crate::metrics::MetricsManager::new_for_test("test_auto_wire");
        engine.auto_wire(
            &mgr,
            #[cfg(feature = "memory")]
            None,
        );
        // Engine should still work after auto_wire
        let msgs = make_json_messages(5);
        let results: Vec<Result<(), String>> = engine.process_mid_tier(&msgs, |_| Ok(()));
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn debug_impl_works() {
        let engine = default_engine();
        let debug = format!("{engine:?}");
        assert!(debug.contains("BatchEngine"));
        assert!(debug.contains("config"));
    }

    /// The driver run loops (`run_workbatch` / `run_workbatch_parsed`) replaced
    /// the four legacy loops in Task 0.7b. These tests exercise the same
    /// behaviours -- process+sink, ticker, on-demand (no-parse) pass-through, and
    /// sink-error resilience -- through the surviving WorkBatch driver.
    #[cfg(feature = "transport-memory")]
    mod driver_engine_tests {
        use super::*;
        use crate::transport::WorkBatch;
        use crate::worker::engine::CommitMode;
        use std::sync::atomic::{AtomicU64, Ordering};

        fn json_payload(table: &str, id: usize) -> Vec<u8> {
            format!(r#"{{"_table":"{table}","id":{id}}}"#).into_bytes()
        }

        /// No-ticker placeholder for the `ticker` argument.
        #[allow(clippy::type_complexity)]
        fn no_ticker() -> Option<(
            std::time::Duration,
            fn() -> std::future::Ready<Result<(), EngineError>>,
        )> {
            None
        }

        fn cancel_after(shutdown: tokio_util::sync::CancellationToken, ms: u64) {
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                shutdown.cancel();
            });
        }

        #[tokio::test]
        async fn run_workbatch_processes_and_passes_tokens_to_sink() {
            let config = crate::transport::memory::MemoryConfig {
                recv_timeout_ms: 50,
                ..Default::default()
            };
            let transport = crate::transport::memory::MemoryTransport::new(&config)
                .expect("memory transport with valid config must construct");
            for i in 0..5 {
                transport
                    .inject(None, json_payload("events", i))
                    .await
                    .unwrap();
            }

            let engine = default_engine();
            let shutdown = tokio_util::sync::CancellationToken::new();
            cancel_after(shutdown.clone(), 200);

            let record_count = Arc::new(AtomicU64::new(0));
            let token_count = Arc::new(AtomicU64::new(0));
            let rc = Arc::clone(&record_count);
            let tc = Arc::clone(&token_count);

            let result = engine
                .run_workbatch(
                    &transport,
                    shutdown,
                    |batch| Ok(batch),
                    |out: &WorkBatch<_>| {
                        let rc = Arc::clone(&rc);
                        let tc = Arc::clone(&tc);
                        let records = out.records.len();
                        let tokens = out.commit_tokens.len();
                        async move {
                            rc.fetch_add(records as u64, Ordering::Relaxed);
                            tc.fetch_add(tokens as u64, Ordering::Relaxed);
                            Ok(())
                        }
                    },
                    // SinkManaged mirrors the legacy run_async sink-owns-commit shape.
                    CommitMode::SinkManaged,
                    no_ticker(),
                )
                .await;

            assert!(result.is_ok());
            assert_eq!(record_count.load(Ordering::Relaxed), 5);
            assert_eq!(token_count.load(Ordering::Relaxed), 5);
        }

        #[tokio::test]
        async fn run_workbatch_ticker_fires() {
            let config = crate::transport::memory::MemoryConfig {
                recv_timeout_ms: 50,
                ..Default::default()
            };
            let transport = crate::transport::memory::MemoryTransport::new(&config)
                .expect("memory transport with valid config must construct");
            let engine = default_engine();
            let shutdown = tokio_util::sync::CancellationToken::new();
            cancel_after(shutdown.clone(), 350);

            let tick_count = Arc::new(AtomicU64::new(0));
            let tick_count_clone = Arc::clone(&tick_count);

            let result = engine
                .run_workbatch(
                    &transport,
                    shutdown,
                    |batch| Ok(batch),
                    |_out: &WorkBatch<_>| async { Ok(()) },
                    CommitMode::Auto,
                    Some((std::time::Duration::from_millis(100), move || {
                        let tc = Arc::clone(&tick_count_clone);
                        async move {
                            tc.fetch_add(1, Ordering::Relaxed);
                            Ok(())
                        }
                    })),
                )
                .await;

            assert!(result.is_ok());
            let ticks = tick_count.load(Ordering::Relaxed);
            assert!(ticks >= 2, "Expected at least 2 ticks, got {ticks}");
        }

        #[tokio::test]
        async fn run_workbatch_passthrough_without_parse() {
            let config = crate::transport::memory::MemoryConfig {
                recv_timeout_ms: 50,
                ..Default::default()
            };
            let transport = crate::transport::memory::MemoryTransport::new(&config)
                .expect("memory transport with valid config must construct");
            for i in 0..3 {
                transport
                    .inject(None, json_payload("logs", i))
                    .await
                    .unwrap();
            }

            let engine = default_engine();
            let shutdown = tokio_util::sync::CancellationToken::new();
            cancel_after(shutdown.clone(), 200);

            let total_bytes = Arc::new(AtomicU64::new(0));
            let total_bytes_clone = Arc::clone(&total_bytes);

            // On-demand driver: pass-through process pays no parse cost.
            let result = engine
                .run_workbatch(
                    &transport,
                    shutdown,
                    |batch| Ok(batch),
                    |out: &WorkBatch<_>| {
                        let tb = Arc::clone(&total_bytes_clone);
                        let sum: u64 = out.records.iter().map(|r| r.payload.len() as u64).sum();
                        async move {
                            tb.fetch_add(sum, Ordering::Relaxed);
                            Ok(())
                        }
                    },
                    CommitMode::Auto,
                    no_ticker(),
                )
                .await;

            assert!(result.is_ok());
            assert!(total_bytes.load(Ordering::Relaxed) > 0);
        }

        #[tokio::test]
        async fn run_workbatch_parsed_reads_field() {
            // The parsed path is the analogue of the old mid-tier run_async: the
            // driver pre-parses and the process closure reads a routing field.
            let config = crate::transport::memory::MemoryConfig {
                recv_timeout_ms: 50,
                ..Default::default()
            };
            let transport = crate::transport::memory::MemoryTransport::new(&config)
                .expect("memory transport with valid config must construct");
            for i in 0..4 {
                transport
                    .inject(None, json_payload("events", i))
                    .await
                    .unwrap();
            }

            let engine = default_engine();
            let shutdown = tokio_util::sync::CancellationToken::new();
            cancel_after(shutdown.clone(), 200);

            let hits = Arc::new(AtomicU64::new(0));
            let hc = Arc::clone(&hits);

            let result = engine
                .run_workbatch_parsed(
                    &transport,
                    shutdown,
                    move |pb| {
                        let field = pb.intern("_table");
                        let mut local = 0u64;
                        for parsed in &pb.parsed {
                            if parsed.field_str(&field) == Some("events") {
                                local += 1;
                            }
                        }
                        hc.fetch_add(local, Ordering::Relaxed);
                        Ok(WorkBatch::new(pb.records, pb.commit_tokens)
                            .with_dlq_entries(pb.dlq_entries))
                    },
                    |_out: &WorkBatch<_>| async { Ok(()) },
                    CommitMode::Auto,
                    no_ticker(),
                )
                .await;

            assert!(result.is_ok());
            assert_eq!(hits.load(Ordering::Relaxed), 4);
        }

        #[tokio::test]
        async fn run_workbatch_sink_error_does_not_crash() {
            let config = crate::transport::memory::MemoryConfig {
                recv_timeout_ms: 50,
                ..Default::default()
            };
            let transport = crate::transport::memory::MemoryTransport::new(&config)
                .expect("memory transport with valid config must construct");
            transport
                .inject(None, json_payload("events", 0))
                .await
                .unwrap();

            let engine = default_engine();
            let shutdown = tokio_util::sync::CancellationToken::new();
            cancel_after(shutdown.clone(), 200);

            // Sink always errors -- the driver logs and continues (no crash).
            let result = engine
                .run_workbatch(
                    &transport,
                    shutdown,
                    |batch| Ok(batch),
                    |_out: &WorkBatch<_>| async {
                        Err(EngineError::Sink("test sink error".into()))
                    },
                    CommitMode::Auto,
                    no_ticker(),
                )
                .await;

            assert!(result.is_ok());
        }
    }
}
