// Project:   hyperi-rustlib
// File:      src/worker/engine/mod.rs
// Purpose:   SIMD-optimised batch processing engine for DFE pipelines
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

pub mod config;
pub mod intern;
pub mod parse;
pub mod pre_route;
pub mod types;

pub use config::{BatchProcessingConfig, ParseErrorAction, PreRouteFilterConfig};
pub use types::{MessageMetadata, ParsedMessage, PreRouteResult, RawMessage};

use std::sync::Arc;

use rayon::prelude::*;

use super::pool::AdaptiveWorkerPool;
use super::stats::PipelineStats;

use self::intern::FieldInterner;
use self::pre_route::{PreRouteOutcome, apply_filters, extract_routing_field, filters_from_config};
use self::types::PayloadFormat;
use super::config::WorkerPoolConfig;

/// Core batch processing engine for DFE pipelines.
///
/// Provides two processing modes:
///
/// - [`process_mid_tier`](Self::process_mid_tier) — parse JSON via SIMD, extract
///   known fields, apply pre-route filters, then parallel transform via rayon.
///   The standard path for most DFE apps (loader, archiver, transforms).
///
/// - [`process_raw`](Self::process_raw) — skip parsing, apply pre-route on raw
///   bytes, then parallel transform via rayon. For apps that handle raw bytes
///   (receiver, binary protocols).
///
/// Both modes chunk large batches, track stats atomically, and pause between
/// chunks when memory pressure is detected.
pub struct BatchEngine {
    config: BatchProcessingConfig,
    pool: Arc<AdaptiveWorkerPool>,
    stats: Arc<PipelineStats>,
    interner: Arc<FieldInterner>,
    filters: Vec<pre_route::PreRouteFilter>,
    #[cfg(feature = "memory")]
    memory_guard: Option<Arc<crate::memory::MemoryGuard>>,
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
        }
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
        _metrics: &crate::metrics::MetricsManager,
        #[cfg(feature = "memory")] memory_guard: Option<&Arc<crate::memory::MemoryGuard>>,
    ) {
        // Metrics registration will be implemented in Task 12 — for now wire memory guard.
        #[cfg(feature = "memory")]
        if let Some(guard) = memory_guard {
            self.memory_guard = Some(Arc::clone(guard));
        }
    }

    /// Parse, filter, and transform a batch of raw messages.
    ///
    /// Pipeline phases per chunk:
    /// 1. **Pre-route** — SIMD field extraction + filter evaluation (sequential, ~100 ns/msg)
    /// 2. **Parse** — `sonic_rs::from_slice` + known-field extraction (sequential, ~1-5 µs/msg)
    /// 3. **Transform** — user closure via rayon `par_iter_mut` (parallel)
    ///
    /// Results contain one entry per non-filtered message. Filtered messages are
    /// silently removed (their commit tokens remain accessible via the original
    /// slice). DLQ'd and parse-error messages produce `Err` entries.
    pub fn process_mid_tier<O, E, F>(
        &self,
        messages: &[RawMessage],
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

                // Phase 2: Parse
                let format = match msg.metadata.format {
                    PayloadFormat::Auto => PayloadFormat::detect(&msg.payload),
                    other => other,
                };

                match parse::parse_payload(&msg.payload, format) {
                    Ok(value) => {
                        let extracted = self.interner.extract_known(&value);
                        parsed_msgs.push(Ok(ParsedMessage::Parsed {
                            value,
                            raw: msg.payload.clone(),
                            format,
                            key: msg.key.clone(),
                            headers: msg.headers.clone(),
                            metadata: msg.metadata.clone(),
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

            // Parallel transform
            let transformed: Vec<(usize, Result<O, E>)> = self.pool.install(|| {
                to_transform
                    .into_par_iter()
                    .map(|(idx, mut pm)| {
                        let result = transform(&mut pm);
                        (idx, result)
                    })
                    .collect()
            });

            chunk_results.extend(transformed);

            // Sort by original index to preserve order
            chunk_results.sort_by_key(|(idx, _)| *idx);

            // Update stats
            let ok_count = chunk_results.iter().filter(|(_, r)| r.is_ok()).count();
            self.stats.add_processed(ok_count as u64);

            all_results.extend(chunk_results.into_iter().map(|(_, r)| r));

            // Memory pressure check between chunks
            self.check_memory_pressure();
        }

        all_results
    }

    /// Pre-route and transform a batch of raw messages without parsing.
    ///
    /// The transform closure receives immutable `&RawMessage` references.
    /// Use this for apps that handle raw bytes directly (e.g. receiver forwarding).
    pub fn process_raw<O, E, F>(&self, messages: &[RawMessage], transform: F) -> Vec<Result<O, E>>
    where
        O: Send,
        E: Send + From<String>,
        F: Fn(&RawMessage) -> Result<O, E> + Sync,
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
            let to_process: Vec<&RawMessage> = if has_routing {
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

            self.check_memory_pressure();
        }

        all_results
    }

    /// Pause between chunks when memory pressure is detected.
    ///
    /// Uses `std::thread::sleep` (not tokio) because `process_mid_tier` and
    /// `process_raw` are sync methods that run within rayon context. The pause
    /// happens between chunks (cold path), not per message.
    #[allow(clippy::unused_self)]
    fn check_memory_pressure(&self) {
        #[cfg(feature = "memory")]
        if let Some(guard) = &self.memory_guard {
            if guard.under_pressure() {
                tracing::warn!("BatchEngine: memory pressure detected, pausing between chunks");
                std::thread::sleep(std::time::Duration::from_millis(
                    self.config.memory_pressure_pause_ms,
                ));
            }
        }
    }
}

impl std::fmt::Debug for BatchEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchEngine")
            .field("config", &self.config)
            .field("pool_max_threads", &self.pool.max_threads())
            .field("stats", &self.stats.snapshot())
            .field("interner_len", &self.interner.len())
            .field("filters", &self.filters)
            .finish()
    }
}

#[cfg(test)]
mod engine_tests {
    use super::*;
    use bytes::Bytes;

    fn make_json_messages(n: usize) -> Vec<RawMessage> {
        (0..n)
            .map(|i| RawMessage {
                payload: Bytes::from(format!(r#"{{"_table":"events","id":{i}}}"#)),
                key: None,
                headers: vec![],
                metadata: MessageMetadata {
                    timestamp_ms: None,
                    format: types::PayloadFormat::Json,
                    commit_token: None,
                },
            })
            .collect()
    }

    fn default_engine() -> BatchEngine {
        BatchEngine::new(BatchProcessingConfig::default())
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
            RawMessage {
                payload: Bytes::from_static(b"not json {{{"),
                key: None,
                headers: vec![],
                metadata: MessageMetadata {
                    timestamp_ms: None,
                    format: types::PayloadFormat::Json,
                    commit_token: None,
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
        msgs[1] = RawMessage {
            payload: Bytes::from(r#"{"_table":"poison","id":999}"#),
            key: None,
            headers: vec![],
            metadata: MessageMetadata {
                timestamp_ms: None,
                format: types::PayloadFormat::Json,
                commit_token: None,
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
        msgs[1] = RawMessage {
            payload: Bytes::from(r#"{"host":"web1"}"#),
            key: None,
            headers: vec![],
            metadata: MessageMetadata {
                timestamp_ms: None,
                format: types::PayloadFormat::Json,
                commit_token: None,
            },
        };

        let results: Vec<Result<String, String>> =
            engine.process_mid_tier(&msgs, |_pm| Ok("ok".to_string()));

        // Filtered messages are removed — only 2 results
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
        let mgr = crate::metrics::MetricsManager::new("test_auto_wire");
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
}
