// Project:   hyperi-rustlib
// File:      src/transport/kafka/mod.rs
// Purpose:   High-throughput Kafka transport for PB/day workloads
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # Kafka Transport
//!
//! High-throughput Kafka transport optimized for PB/day batch processing.
//! Uses rdkafka (librdkafka wrapper) with batch-first design.
//!
//! ## Performance Characteristics
//!
//! - **Batch-first**: Designed for 10K+ messages per batch
//! - **Zero-copy where possible**: Minimizes allocations in hot path
//! - **Interned topic cache**: shared RwLock map, read-fast-path per message
//! - **Non-blocking batch drain**: Uses zero-timeout poll to drain internal queue
//! - **At-least-once delivery**: Manual commit after processing
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::transport::{KafkaTransport, KafkaConfig, Transport};
//!
//! let config = KafkaConfig {
//!     brokers: vec!["kafka:9092".to_string()],
//!     group: "dfe-loader".to_string(),
//!     topics: vec!["events".to_string()],
//!     ..Default::default()
//! };
//!
//! let transport = KafkaTransport::new(&config).await?;
//!
//! // Batch processing loop
//! loop {
//!     // Poll for up to 10K messages
//!     let batch = transport.recv(10_000).await?;
//!     if batch.is_empty() {
//!         continue;
//!     }
//!
//!     // Process entire batch
//!     process_batch(&batch.records);
//!
//!     // Commit AFTER successful processing (at-least-once)
//!     transport.commit(&batch.commit_tokens).await?;
//! }
//! ```

mod admin;
mod config;
mod metrics;
mod producer;
mod token;
pub mod topic_resolver;

pub use admin::{KafkaAdmin, TopicInfo};
#[allow(deprecated)]
pub use config::{
    ConsumerKnobs, DEVTEST_PROFILE, HIGH_THROUGHPUT_CONSUMER_DEFAULTS, KafkaConfig, KafkaProfile,
    KafkaSizingConfig, LOW_LATENCY_CONSUMER_DEFAULTS, PRODUCER_DEFAULTS, PRODUCER_DEVTEST,
    PRODUCER_EXACTLY_ONCE, PRODUCER_HIGH_THROUGHPUT, PRODUCER_LOW_LATENCY, PRODUCTION_PROFILE,
    ProducerKnobs, SelfRegulationProfile, SuppressionRule, merge_with_overrides,
};
pub use metrics::{
    BrokerMetrics, KafkaMetrics, StatsContext, healthy_broker_count, total_consumer_lag,
};
pub use producer::{KafkaProducer, ProducerMetrics, ProducerProfile};
pub use token::KafkaToken;
pub use topic_resolver::{TopicRefreshHandle, TopicResolver};

use super::error::{TransportError, TransportResult};
use super::traits::{RecvBatch, TransportBase, TransportReceiver, TransportSender};
use super::types::{Message, PayloadFormat, SendResult};
use super::work_batch::WorkBatch;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, CommitMode, Consumer};
use rdkafka::message::Message as KafkaMessage;
use rdkafka::producer::{FutureProducer, FutureRecord};
use rdkafka::topic_partition_list::{Offset, TopicPartitionList};
use rdkafka::util::Timeout;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// High-throughput tuning defaults.
///
/// These are optimized for PB/day batch workloads.
pub mod tuning {
    /// Default batch size for recv() - 10K messages.
    pub const DEFAULT_BATCH_SIZE: usize = 10_000;

    /// Maximum time to spend draining the internal queue (ms).
    /// After this, return what we have to maintain responsiveness.
    pub const MAX_DRAIN_MS: u64 = 100;

    /// Poll timeout when queue is empty - triggers network fetch.
    pub const POLL_TIMEOUT_MS: u64 = 50;

    /// Pre-allocated message vector capacity.
    pub const INITIAL_BATCH_CAPACITY: usize = 10_000;
}

/// High-throughput Kafka transport using rdkafka.
///
/// Optimized for batch-oriented consumption at PB/day scale:
/// - Uses `BaseConsumer` for direct poll control (see recv() decision note)
/// - Interns topic strings in a shared cache (read-fast-path per message)
/// - Drains internal queue with zero-timeout polls
/// - Minimizes allocations in hot path
pub struct KafkaTransport {
    /// librdkafka consumer.
    ///
    /// Behind an `Arc` so the optional [`KafkaGateActuator`] (G3, behind the
    /// `governor` feature) can hold a clone and call `pause`/`resume` on the
    /// ASSIGNED partitions without `unsafe` and without taking ownership away
    /// from the recv poll loop. Every `Consumer` method we use (`poll`,
    /// `subscribe`, `commit`, `assignment`, `pause`, `resume`,
    /// `fetch_group_list`) takes `&self`, so a shared `Arc` serves both the
    /// transport's poll loop and the actuator's pause/resume with no lock --
    /// librdkafka is internally synchronised.
    consumer: Arc<BaseConsumer<StatsContext>>,
    producer: FutureProducer<StatsContext>,
    /// Persistent topic-string interner. Shared across `recv()` calls so a
    /// newly-discovered topic is interned once (not re-`Arc`'d every batch) --
    /// the previous per-recv clone discarded new entries. RwLock: reads
    /// dominate (topics repeat), writes only on first sight of a topic.
    topic_cache: parking_lot::RwLock<HashMap<String, Arc<str>>>,
    closed: AtomicBool,
    /// Shared healthy flag -- read by health registry closure, written by close().
    healthy: Arc<AtomicBool>,
    /// Topics we're subscribed to (for cache warming and Debug).
    /// Behind RwLock so recv() can update after topic refresh re-subscribe.
    subscribed_topics: parking_lot::RwLock<Vec<String>>,
    /// Shutdown token -- cancelled on close() to stop background tasks.
    shutdown_token: tokio_util::sync::CancellationToken,
    /// Periodic topic refresh handle (auto-discovery mode only).
    /// Checked on each recv() call to detect new/removed topics.
    /// Uses parking_lot::Mutex (no poisoning, faster uncontended) since this
    /// is on the recv() hot path.
    topic_refresh: Option<parking_lot::Mutex<TopicRefreshHandle>>,
    /// Transport-level message filter engine.
    filter_engine: super::filter::TransportFilterEngine,
    /// Optional inbound gate (G3, `governor` feature). `None` by default ->
    /// `recv()` makes no gate calls and behaviour is byte-identical to today.
    /// When `Some`, each `recv()` calls [`InboundGate::evaluate`], which drives
    /// the [`KafkaGateActuator`] on pause/resume edges. The poll is ALWAYS
    /// issued regardless of hold state -- paused partitions just return nothing,
    /// keeping the consumer-group heartbeat alive (no rebalance). Phase 3 wires
    /// this on by default; here it is purely additive and opt-in.
    #[cfg(feature = "governor")]
    inbound_gate: Option<crate::governor::InboundGate>,
    /// Diagnostic dedup latch for the `kafka_partition_limited` warning (G3).
    /// Rate-limits the warning to once per cooldown window so a persistently
    /// partition-limited consumer does not spam the log. `None` until the
    /// diagnostic is consulted; behaviour is unchanged when the diagnostic is
    /// never invoked.
    #[cfg(feature = "governor")]
    partition_limited_warn: PartitionLimitedDiagnostic,
}

impl KafkaTransport {
    /// Create a new high-throughput Kafka transport.
    ///
    /// The transport is optimized for batch consumption at PB/day scale.
    /// Configuration defaults are tuned for high throughput:
    /// - `fetch.max.bytes`: 50MB (controls network batch size)
    /// - `enable.auto.commit`: false (manual commit for at-least-once)
    ///
    /// # Errors
    ///
    /// Returns error if Kafka client creation fails.
    // Large but linear constructor (config -> client -> subscribe -> assemble);
    // the additive governor fields nudged it over the 150-line soft cap.
    #[allow(clippy::too_many_lines)]
    pub async fn new(config: &KafkaConfig) -> TransportResult<Self> {
        // Enforce the production guardrail at construction (Codex review
        // 2026-06-03): reject ssl_skip_verify (and insecure transport without
        // an explicit override) in prod here, not only when an app remembers
        // to call validate() at startup.
        config
            .validate(crate::env::is_production())
            .map_err(TransportError::Config)?;

        let mut client_config = ClientConfig::new();

        // Required settings
        client_config.set("bootstrap.servers", config.brokers.join(","));
        client_config.set("group.id", &config.group);
        client_config.set("enable.auto.commit", config.enable_auto_commit.to_string());
        client_config.set(
            "auto.commit.interval.ms",
            config.auto_commit_interval_ms.to_string(),
        );
        client_config.set("session.timeout.ms", config.session_timeout_ms.to_string());
        client_config.set(
            "heartbeat.interval.ms",
            config.heartbeat_interval_ms.to_string(),
        );
        client_config.set(
            "max.poll.interval.ms",
            config.max_poll_interval_ms.to_string(),
        );
        client_config.set("fetch.min.bytes", config.fetch_min_bytes.to_string());
        client_config.set("fetch.max.bytes", config.fetch_max_bytes.to_string());
        client_config.set(
            "max.partition.fetch.bytes",
            config.max_partition_fetch_bytes.to_string(),
        );
        client_config.set("auto.offset.reset", &config.auto_offset_reset);
        client_config.set(
            "enable.partition.eof",
            config.enable_partition_eof.to_string(),
        );

        // Apply profile defaults (these can be overridden by librdkafka_overrides)
        let rdkafka_config = config.build_librdkafka_config();
        for (key, value) in &rdkafka_config {
            client_config.set(key, value);
        }

        // Apply the Task 0.5 sizing surface:
        //   profile defaults < named consumer knobs < sizing.consumer_librdkafka
        // This is applied AFTER the legacy profile/librdkafka_overrides block so
        // the sizing config takes precedence. The raw sizing.consumer_librdkafka
        // map wins over everything (applied last inside resolved_consumer_map()).
        for (key, value) in config.sizing.resolved_consumer_map() {
            client_config.set(key, value);
        }

        // Security settings
        client_config.set("security.protocol", &config.security_protocol);
        if let Some(ref mechanism) = config.sasl_mechanism {
            client_config.set("sasl.mechanism", mechanism);
        }
        if let Some(ref username) = config.sasl_username {
            client_config.set("sasl.username", username);
        }
        if let Some(ref password) = config.sasl_password {
            client_config.set("sasl.password", password.expose());
        }

        // TLS settings
        if let Some(ref ca) = config.ssl_ca_location {
            client_config.set("ssl.ca.location", ca);
        }
        if let Some(ref cert) = config.ssl_certificate_location {
            client_config.set("ssl.certificate.location", cert);
        }
        if let Some(ref key) = config.ssl_key_location {
            client_config.set("ssl.key.location", key);
        }
        if config.ssl_skip_verify {
            client_config.set("enable.ssl.certificate.verification", "false");
        }

        // Client ID
        client_config.set("client.id", &config.client_id);

        // Ensure statistics callbacks fire (all profiles already set this, but
        // guarantee it as a fallback for manual configs).
        if client_config.get("statistics.interval.ms").is_none() {
            client_config.set("statistics.interval.ms", "5000");
        }

        // StatsContext receives librdkafka statistics callbacks and auto-emits
        // rdkafka_* Prometheus metrics when a recorder is installed.
        // Consumer and producer each get their own context instance.

        // Create consumer with StatsContext for metrics collection. Arc-wrapped
        // so an optional gate actuator (governor feature) can share it for
        // pause/resume without unsafe -- see the field doc.
        let consumer: BaseConsumer<StatsContext> = client_config
            .create_with_context(StatsContext::new())
            .map_err(|e| TransportError::Connection(format!("Failed to create consumer: {e}")))?;
        let consumer = Arc::new(consumer);

        // Resolve effective topics:
        // - Explicit list → subscribe to those
        // - Empty + auto_discover → auto-discover from broker
        // - Empty + !auto_discover → no subscription (producer-only)
        let (effective_topics, topic_refresh, shutdown_token) =
            if config.topics.is_empty() && config.auto_discover {
                tracing::info!("Topics empty -- auto-discovering from broker");
                let resolver = topic_resolver::TopicResolver::new(config)?;
                let discovered = resolver.resolve()?;
                if discovered.is_empty() {
                    return Err(TransportError::Config(
                        "Auto-discovery found no matching topics".into(),
                    ));
                }

                let token = tokio_util::sync::CancellationToken::new();
                let refresh = if config.topic_refresh_secs > 0 {
                    let refresh_resolver = topic_resolver::TopicResolver::new(config)?;
                    let handle = refresh_resolver.start_refresh_loop(
                        Duration::from_secs(config.topic_refresh_secs),
                        token.clone(),
                    );
                    tracing::info!(
                        interval_secs = config.topic_refresh_secs,
                        "Started periodic topic refresh"
                    );
                    Some(parking_lot::Mutex::new(handle))
                } else {
                    None
                };

                (discovered, refresh, token)
            } else {
                (
                    config.topics.clone(),
                    None,
                    tokio_util::sync::CancellationToken::new(),
                )
            };

        // Subscribe to topics
        let subscribed_topics = effective_topics;
        if !subscribed_topics.is_empty() {
            let topics: Vec<&str> = subscribed_topics.iter().map(String::as_str).collect();
            consumer
                .subscribe(&topics)
                .map_err(|e| TransportError::Connection(format!("Failed to subscribe: {e}")))?;
        }

        // Pre-populate topic cache - eliminates locks in hot path
        let mut topic_cache = HashMap::with_capacity(subscribed_topics.len());
        for topic in &subscribed_topics {
            topic_cache.insert(topic.clone(), Arc::from(topic.as_str()));
        }

        // Create producer with StatsContext for metrics collection
        let producer: FutureProducer<StatsContext> = client_config
            .create_with_context(StatsContext::new())
            .map_err(|e| TransportError::Connection(format!("Failed to create producer: {e}")))?;

        let healthy = Arc::new(AtomicBool::new(true));

        let filter_engine = super::filter::TransportFilterEngine::new(
            &config.filters_in,
            &config.filters_out,
            &crate::transport::filter::TransportFilterTierConfig::from_cascade(),
        )?;

        #[cfg(feature = "health")]
        {
            let h = Arc::clone(&healthy);
            crate::health::HealthRegistry::register("transport:kafka", move || {
                if h.load(Ordering::Relaxed) {
                    crate::health::HealthStatus::Healthy
                } else {
                    crate::health::HealthStatus::Unhealthy
                }
            });
        }

        Ok(Self {
            consumer,
            producer,
            topic_cache: parking_lot::RwLock::new(topic_cache),
            closed: AtomicBool::new(false),
            healthy,
            subscribed_topics: parking_lot::RwLock::new(subscribed_topics),
            shutdown_token,
            topic_refresh,
            filter_engine,
            #[cfg(feature = "governor")]
            inbound_gate: None,
            #[cfg(feature = "governor")]
            partition_limited_warn: PartitionLimitedDiagnostic::default(),
        })
    }

    /// Attach an [`InboundGate`](crate::governor::InboundGate) to this
    /// transport (G3, `governor` feature).
    ///
    /// ADDITIVE + opt-in: the default is no gate, so a transport built without
    /// this call behaves byte-identically to before. When attached, every
    /// [`recv`](TransportReceiver::recv) calls [`evaluate`](crate::governor::InboundGate::evaluate)
    /// which drives a `KafkaGateActuator` on pause/resume edges. Build the
    /// gate with [`KafkaTransport::gate_actuator`] so it pauses the consumer's
    /// ASSIGNED partitions (member stays in the group -- no rebalance).
    ///
    /// CRUCIAL: even while held, `recv()` still issues the poll -- the
    /// actuator pauses partitions, not the poll, so the heartbeat is preserved.
    #[cfg(feature = "governor")]
    #[must_use]
    pub fn with_inbound_gate(mut self, gate: crate::governor::InboundGate) -> Self {
        self.inbound_gate = Some(gate);
        self
    }

    /// Build a [`GateActuator`](crate::governor::GateActuator) that pauses and
    /// resumes THIS transport's consumer (G3, `governor` feature).
    ///
    /// The returned actuator holds an `Arc` clone of the shared consumer. On
    /// the rising edge it reads the current [`assignment`](rdkafka::consumer::Consumer::assignment)
    /// and [`pause`](rdkafka::consumer::Consumer::pause)s exactly those
    /// partitions; on the falling edge it [`resume`](rdkafka::consumer::Consumer::resume)s
    /// them. Pausing the ASSIGNED set (not unsubscribing) keeps the member in
    /// the consumer group, so no rebalance is triggered while we hold.
    ///
    /// Pass the result to [`InboundGate::new`](crate::governor::InboundGate::new),
    /// then [`with_inbound_gate`](Self::with_inbound_gate) the gate back onto
    /// the transport.
    #[cfg(feature = "governor")]
    #[must_use]
    pub fn gate_actuator(&self) -> Box<dyn crate::governor::GateActuator> {
        Box::new(KafkaGateActuator {
            consumer: Arc::clone(&self.consumer),
        })
    }

    /// Get the consumer's metrics snapshot.
    ///
    /// Returns statistics collected via librdkafka callbacks. Includes
    /// broker RTT, consumer lag, rebalance count, etc.
    #[must_use]
    pub fn stats(&self) -> KafkaMetrics {
        self.consumer.context().get_metrics()
    }

    /// Run the `kafka_partition_limited` DIAGNOSTIC against the live group
    /// (G3, `governor` feature).
    ///
    /// Reads the consumer-group member count (via `fetch_group_list`), the
    /// topic partition count (from cached metadata), and the current total
    /// consumer lag (from `StatsContext`), then evaluates the pure
    /// [`partition_limited`] decision. When limited it:
    /// - sets the `kafka_partition_limited` gauge to `1.0` (else `0.0`),
    /// - records the diagnostic on the health registry, and
    /// - emits ONE rate-limited warning per cooldown window.
    ///
    /// NO topology mutation -- it never calls `createPartitions`. Returns the
    /// decision so callers (and Phase 3 wiring) can act on it. This is a
    /// metadata round-trip; call it periodically (e.g. once per refresh tick),
    /// NOT on the recv hot path.
    ///
    /// # Errors
    ///
    /// Returns an error if the broker metadata / group-list fetch fails.
    #[cfg(feature = "governor")]
    pub fn check_partition_limited(&self) -> TransportResult<bool> {
        // Total consumer lag across all assigned partitions.
        let metrics = self.consumer.context().get_metrics();
        let lag = u64::try_from(total_consumer_lag(&metrics).max(0)).unwrap_or(0);

        // Partition count: sum the assigned partitions from the live assignment.
        // (Cheap, local; avoids a per-topic metadata fetch on the common path.)
        let partitions = self.consumer.assignment().map_or(0, |tpl| tpl.count());

        // Member count: fetch the group metadata. `fetch_group_list(None, ..)`
        // returns all groups the broker knows; we take the largest member count
        // as the worst-case reading. If the broker returns nothing (transient,
        // or older broker) we treat it as a single member -- never a
        // false-positive "limited" reading.
        let members = self
            .consumer
            .fetch_group_list(None, Duration::from_secs(5))
            .ok()
            .and_then(|list| list.groups().iter().map(|g| g.members().len()).max())
            .unwrap_or(1);

        let limited = partition_limited(members, partitions, lag);

        #[cfg(feature = "metrics")]
        ::metrics::gauge!("kafka_partition_limited").set(if limited { 1.0 } else { 0.0 });

        #[cfg(feature = "health")]
        if limited {
            crate::health::HealthRegistry::register("kafka:partition_limited", || {
                crate::health::HealthStatus::Degraded
            });
        }

        if limited
            && self
                .partition_limited_warn
                .should_warn_at(std::time::Instant::now())
        {
            tracing::warn!(
                members,
                partitions,
                lag,
                "kafka consumer group is partition-limited: members >= partitions \
                 with persistent lag -- extra consumers sit idle; the topic needs \
                 more partitions (diagnostic only, no topology change made)"
            );
        }

        Ok(limited)
    }

    /// Spawn a periodic background task that runs the
    /// [`check_partition_limited`](Self::check_partition_limited) diagnostic on
    /// `interval` until `shutdown` is cancelled (G3, `governor` feature).
    ///
    /// This is the intended caller for the diagnostic: a COLD periodic tick OFF
    /// the hot recv path. Each tick is a broker metadata round-trip
    /// (`fetch_group_list`), so keep `interval` coarse (tens of seconds);
    /// pairing it with the topic-refresh cadence is a sensible default. The task
    /// only updates the `kafka_partition_limited` gauge + a rate-limited warning;
    /// it NEVER mutates topology.
    ///
    /// Wrap the transport in an `Arc` first (the actuator + receiver share it
    /// anyway), then call this with a clone.
    #[cfg(feature = "governor")]
    pub fn spawn_partition_limited_tick(
        self: Arc<Self>,
        interval: Duration,
        shutdown: tokio_util::sync::CancellationToken,
    ) {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            tick.tick().await; // consume the immediate first tick
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    _ = tick.tick() => {
                        if let Err(e) = self.check_partition_limited() {
                            tracing::debug!(error = %e, "partition-limited diagnostic tick failed");
                        }
                    }
                }
            }
        });
    }
}

impl TransportBase for KafkaTransport {
    async fn close(&self) -> TransportResult<()> {
        self.closed.store(true, Ordering::Relaxed);
        self.healthy.store(false, Ordering::Relaxed);
        self.shutdown_token.cancel();
        // rdkafka handles cleanup on drop
        Ok(())
    }

    fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    fn name(&self) -> &'static str {
        "kafka"
    }
}

impl TransportSender for KafkaTransport {
    async fn send(&self, key: &str, payload: bytes::Bytes) -> SendResult {
        if self.closed.load(Ordering::Relaxed) {
            return SendResult::Fatal(TransportError::Closed);
        }

        // Outbound filter check
        if self.filter_engine.has_outbound_filters() {
            match self.filter_engine.apply_outbound(&payload) {
                super::filter::FilterDisposition::Pass => {}
                super::filter::FilterDisposition::Drop => return SendResult::Ok,
                super::filter::FilterDisposition::Dlq => return SendResult::FilteredDlq,
            }
        }

        let record: FutureRecord<'_, str, [u8]> = FutureRecord::to(key).payload(payload.as_ref());

        // Inject W3C traceparent into Kafka message headers for distributed tracing
        #[cfg(feature = "transport-trace")]
        let record = if let Some(tp) = super::propagation::current_traceparent() {
            let headers = rdkafka::message::OwnedHeaders::new().insert(rdkafka::message::Header {
                key: super::propagation::TRACEPARENT_HEADER,
                value: Some(tp.as_str()),
            });
            record.headers(headers)
        } else {
            record
        };

        #[cfg(feature = "metrics")]
        let start = std::time::Instant::now();

        let result = match self
            .producer
            .send(record, Timeout::After(Duration::from_secs(5)))
            .await
        {
            Ok(_) => {
                #[cfg(feature = "metrics")]
                ::metrics::counter!("dfe_transport_sent_total", "transport" => "kafka")
                    .increment(1);
                SendResult::Ok
            }
            Err((err, _)) => {
                let err_str = err.to_string();
                if err_str.contains("queue full") || err_str.contains("Local: Queue full") {
                    #[cfg(feature = "metrics")]
                    ::metrics::counter!(
                        "dfe_transport_backpressured_total",
                        "transport" => "kafka"
                    )
                    .increment(1);
                    SendResult::Backpressured
                } else {
                    #[cfg(feature = "metrics")]
                    ::metrics::counter!(
                        "dfe_transport_send_errors_total",
                        "transport" => "kafka"
                    )
                    .increment(1);
                    SendResult::Fatal(TransportError::Send(err_str))
                }
            }
        };

        #[cfg(feature = "metrics")]
        ::metrics::histogram!(
            "dfe_transport_send_duration_seconds",
            "transport" => "kafka"
        )
        .record(start.elapsed().as_secs_f64());

        result
    }
}

impl TransportReceiver for KafkaTransport {
    type Token = KafkaToken;

    /// Receive a batch of messages.
    ///
    /// This is optimized for high-throughput batch processing:
    /// - Uses zero-timeout polls to drain librdkafka's internal queue
    /// - Returns up to `max` messages per call
    /// - Pre-populates topic cache to avoid allocations
    ///
    /// For PB/day workloads, call with `max = 10_000` or higher.
    async fn recv(&self, max: usize) -> TransportResult<WorkBatch<Self::Token>> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(TransportError::Closed);
        }

        // G3 inbound gate (governor feature, opt-in). Evaluate the gate so it
        // drives the actuator on pause/resume EDGES (pausing/resuming the
        // assigned partitions). We do NOT branch on the result: the poll below
        // ALWAYS runs. Paused partitions simply return no records, which keeps
        // the consumer-group heartbeat alive (no rebalance) while pressure
        // drains. Default `None` -> no call -> byte-identical to before.
        #[cfg(feature = "governor")]
        if let Some(ref gate) = self.inbound_gate {
            let _ = gate.evaluate();
        }

        // Check for topic changes from the background refresh loop
        if let Some(ref refresh) = self.topic_refresh
            && let Some(new_topics) = refresh.lock().check_changed()
        {
            let topics: Vec<&str> = new_topics.iter().map(String::as_str).collect();
            match self.consumer.subscribe(&topics) {
                Ok(()) => {
                    tracing::info!(?new_topics, "Re-subscribed after topic refresh");
                    *self.subscribed_topics.write() = new_topics;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to re-subscribe after topic refresh");
                }
            }
        }

        let timeout = Duration::from_millis(tuning::POLL_TIMEOUT_MS);
        let max_msgs = max;

        // DECISION (at-scale hardening 2.6): we KEEP synchronous
        // `BaseConsumer::poll` inside this async `recv` rather than switching
        // to `StreamConsumer`. Rationale:
        //   - librdkafka does the network fetch on its OWN background threads;
        //     `poll` just dequeues from an in-memory queue. The initial poll
        //     only blocks the worker (<= POLL_TIMEOUT_MS = 50ms) when the queue
        //     is EMPTY -- i.e. idle/low-traffic, when nothing else contends.
        //     Under load the poll returns immediately.
        //   - The drain loop uses ZERO-timeout polls bounded by MAX_DRAIN_MS
        //     (100ms); that is useful ingest work, not a block.
        //   - `StreamConsumer` would change commit/rebalance semantics on the
        //     critical ingress path -- real regression risk for an unmeasured
        //     latency benefit. So we MEASURE first (the poll-duration metric
        //     below); only escalate to spawn_blocking/StreamConsumer if it
        //     shows real Tokio-worker starvation. See the plan decision ledger.
        #[cfg(feature = "metrics")]
        let poll_start = std::time::Instant::now();

        // --- recv-arena (Task 0.4.3) ---------------------------------------
        // Instead of `payload.to_vec()` per message (N copies + N heap allocs),
        // we copy every record's payload ONCE into a single growable arena and
        // collect OWNED span metadata. After the polls we freeze the arena to
        // one refcounted `Bytes` and slice it -- so the whole batch shares ONE
        // allocation. See `build_batch_from_spans` and the per-arm safety note.
        let span_cap = max_msgs.min(tuning::INITIAL_BATCH_CAPACITY);
        let mut spans: Vec<Span> = Vec::with_capacity(span_cap);
        // Arena byte estimate: ~256 bytes/record is a conservative starting
        // guess (typical JSON event); it grows as needed. One up-front alloc
        // beats N small ones even when the guess is off.
        let mut arena: Vec<u8> = Vec::with_capacity(span_cap.saturating_mul(256));
        let drain_deadline =
            std::time::Instant::now() + Duration::from_millis(tuning::MAX_DRAIN_MS);

        // Phase 1: Initial poll (drains librdkafka's queue; blocks <= timeout
        // only when the queue is empty).
        if let Some(result) = self.consumer.poll(timeout) {
            match result {
                Ok(msg) => {
                    // Extract W3C traceparent from Kafka headers (first message only,
                    // to associate the batch span with the upstream trace)
                    #[cfg(feature = "transport-trace")]
                    if let Some(headers) = msg.headers() {
                        use rdkafka::message::Headers;
                        for idx in 0..headers.count() {
                            if let Some(Ok(header)) = headers.try_get_as::<[u8]>(idx)
                                && header.key == super::propagation::TRACEPARENT_HEADER
                            {
                                if let Some(value) = header.value
                                    && let Ok(tp) = std::str::from_utf8(value)
                                    && super::propagation::is_valid_traceparent(tp)
                                {
                                    tracing::Span::current().record("traceparent", tp);
                                }
                                break;
                            }
                        }
                    }

                    let topic_str = msg.topic();
                    let topic: Arc<str> = get_or_insert_topic(&self.topic_cache, topic_str);
                    // SAFETY (lifetime, not unsafe): `msg.payload()` borrows
                    // librdkafka's internal buffer and is valid ONLY while this
                    // `BorrowedMessage` lives (until the next poll / its drop).
                    // We copy it OUT into the arena RIGHT HERE -- this is the
                    // one unavoidable copy out of the borrowed buffer -- and we
                    // never store the `&[u8]` or `msg` past this arm.
                    let start = arena.len();
                    arena.extend_from_slice(msg.payload().unwrap_or(&[]));
                    let end = arena.len();
                    // Extract OWNED metadata before `msg` drops at arm end.
                    let partition = msg.partition();
                    let offset = msg.offset();
                    let timestamp_ms = msg.timestamp().to_millis();

                    spans.push(Span {
                        key: Some(topic.clone()),
                        token: KafkaToken::new(topic, partition, offset),
                        timestamp_ms,
                        format: PayloadFormat::Auto,
                        range: start..end,
                    });
                }
                Err(e) => {
                    return Err(TransportError::Recv(e.to_string()));
                }
            }
        } else {
            #[cfg(feature = "metrics")]
            ::metrics::histogram!("kafka_poll_duration_seconds")
                .record(poll_start.elapsed().as_secs_f64());
            // No message available -- empty arena, empty spans, empty batch.
            return Ok(RecvBatch::from_messages(build_batch_from_spans(
                bytes::Bytes::new(),
                spans,
            ))
            .into());
        }

        // Phase 2: Drain queue with zero-timeout polls
        // This is where the batch magic happens - librdkafka has already
        // fetched a batch from the network, we just drain it fast.
        while spans.len() < max_msgs {
            if std::time::Instant::now() >= drain_deadline {
                break;
            }

            match self.consumer.poll(Duration::ZERO) {
                Some(Ok(msg)) => {
                    let topic_str = msg.topic();
                    let topic: Arc<str> = get_or_insert_topic(&self.topic_cache, topic_str);
                    // Same lifetime contract as Phase 1: copy the borrowed
                    // payload into the arena HERE, extract owned metadata HERE,
                    // never let `msg`/`&[u8]` escape this arm.
                    let start = arena.len();
                    arena.extend_from_slice(msg.payload().unwrap_or(&[]));
                    let end = arena.len();
                    let partition = msg.partition();
                    let offset = msg.offset();
                    let timestamp_ms = msg.timestamp().to_millis();

                    spans.push(Span {
                        key: Some(topic.clone()),
                        token: KafkaToken::new(topic, partition, offset),
                        timestamp_ms,
                        format: PayloadFormat::Auto,
                        range: start..end,
                    });
                }
                Some(Err(e)) => {
                    if spans.is_empty() {
                        return Err(TransportError::Recv(e.to_string()));
                    }
                    break;
                }
                None => break,
            }
        }

        // Freeze the arena to ONE refcounted Bytes, then rebuild messages as
        // zero-copy slices into it. All borrowed Kafka buffers are long gone --
        // every byte we keep was copied into `arena` inside a poll arm above.
        let arena: bytes::Bytes = bytes::Bytes::from(arena);
        let messages = build_batch_from_spans(arena, spans);

        // Apply inbound filters via the shared partition helper; DLQ entries
        // are returned in the RecvBatch for the caller to route onward.
        let batch =
            self.filter_engine
                .partition_batch(messages, |m| m.payload.as_ref(), |m| m.key.clone());
        let messages = batch.messages;
        let dlq_entries = batch.dlq_entries;

        Ok(RecvBatch {
            messages,
            dlq_entries,
        }
        .into())
    }

    /// Commit offsets for processed messages.
    ///
    /// Uses async commit for better throughput. The commit is batched
    /// by partition - only the highest offset per partition is committed.
    async fn commit(&self, tokens: &[Self::Token]) -> TransportResult<()> {
        if tokens.is_empty() {
            return Ok(());
        }

        // Build topic-partition-offset list
        // For each partition, commit the highest offset + 1 (next to be read)
        let mut tpl = TopicPartitionList::new();
        let mut partition_offsets: HashMap<(&str, i32), i64> =
            HashMap::with_capacity(tokens.len() / 100);

        for token in tokens {
            let key = (token.topic.as_ref(), token.partition);
            partition_offsets
                .entry(key)
                .and_modify(|current| {
                    if token.offset > *current {
                        *current = token.offset;
                    }
                })
                .or_insert(token.offset);
        }

        for ((topic, partition), offset) in partition_offsets {
            tpl.add_partition_offset(topic, partition, Offset::Offset(offset + 1))
                .map_err(|e| TransportError::Commit(format!("Failed to build TPL: {e}")))?;
        }

        // Async commit for better throughput
        self.consumer
            .commit(&tpl, CommitMode::Async)
            .map_err(|e| TransportError::Commit(e.to_string()))?;

        Ok(())
    }
}

/// One record's worth of OWNED metadata collected during a poll, plus the
/// byte range of its payload inside the recv-arena.
///
/// Crucially this carries NO borrowed data: the `BorrowedMessage` and the
/// `&[u8]` payload it lends are valid only until the next poll / until that
/// message drops, so every field here is owned (`Arc<str>`, `i64`, indices).
/// The payload itself has already been copied into the shared arena; `range`
/// is where. See `recv()` for the poll-arm safety argument.
struct Span {
    /// Routing key (interned topic Arc), mirrors `Message::key`.
    key: Option<Arc<str>>,
    /// Commit token (topic/partition/offset), owned.
    token: KafkaToken,
    /// Transport timestamp in millis since epoch, if present.
    timestamp_ms: Option<i64>,
    /// Detected payload format (matches the legacy per-message default).
    format: PayloadFormat,
    /// Half-open byte range of this record's payload within the frozen arena.
    range: core::ops::Range<usize>,
}

/// Rebuild a batch of `Message`s from a frozen recv-arena and its spans.
///
/// The arena is ONE `Bytes` holding every record's payload back-to-back; each
/// span's `range` indexes into it. We build each `Message::payload` via
/// `arena.slice(range)` -- a refcount bump into the shared backing buffer, NOT
/// a per-record copy. So the whole batch shares ONE allocation and frees once
/// when the last record drops (the "whole batch shares one allocation"
/// contract).
///
/// This is a free function so the correctness-critical assembly is unit
/// testable WITHOUT a live Kafka broker.
fn build_batch_from_spans(arena: bytes::Bytes, spans: Vec<Span>) -> Vec<Message<KafkaToken>> {
    spans
        .into_iter()
        .map(|span| Message {
            key: span.key,
            // Zero-copy slice into the shared arena (refcount bump only).
            payload: arena.slice(span.range),
            token: span.token,
            timestamp_ms: span.timestamp_ms,
            format: span.format,
        })
        .collect()
}

/// Get or insert topic Arc into cache.
///
/// Inline helper for hot path - avoids method call overhead.
#[inline]
fn get_or_insert_topic(
    cache: &parking_lot::RwLock<HashMap<String, Arc<str>>>,
    topic: &str,
) -> Arc<str> {
    // Fast path: shared read lock, hit on the common case (topic seen before).
    if let Some(arc) = cache.read().get(topic) {
        return arc.clone();
    }
    // First sight: take the write lock and intern. A benign race (two callers
    // miss and both insert) just converges on equivalent Arcs -- harmless.
    let arc: Arc<str> = Arc::from(topic);
    cache.write().insert(topic.to_string(), arc.clone());
    arc
}

// --- G3: inbound gate actuator (governor feature) ---------------------------

/// A [`GateActuator`](crate::governor::GateActuator) that pauses/resumes a
/// shared Kafka consumer's ASSIGNED partitions.
///
/// Holds an `Arc` clone of the transport's consumer (see
/// [`KafkaTransport::gate_actuator`]). On the rising edge it reads the live
/// assignment and pauses exactly those partitions; on the falling edge it
/// resumes them. Pausing the assignment (not unsubscribing) keeps the member
/// in the group, so no rebalance fires while held. Pause/resume failures are
/// logged but never panic -- the gate's edge bookkeeping has already advanced,
/// and a missed pause degrades to "kept ingesting", never a deadlock.
#[cfg(feature = "governor")]
struct KafkaGateActuator {
    consumer: Arc<BaseConsumer<StatsContext>>,
}

#[cfg(feature = "governor")]
impl crate::governor::GateActuator for KafkaGateActuator {
    fn pause(&self) {
        match self.consumer.assignment() {
            Ok(tpl) => {
                if let Err(e) = self.consumer.pause(&tpl) {
                    tracing::warn!(error = %e, "kafka gate: pause(assignment) failed");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "kafka gate: assignment() failed on pause");
            }
        }
    }

    fn resume(&self) {
        match self.consumer.assignment() {
            Ok(tpl) => {
                if let Err(e) = self.consumer.resume(&tpl) {
                    tracing::warn!(error = %e, "kafka gate: resume(assignment) failed");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "kafka gate: assignment() failed on resume");
            }
        }
    }
}

// --- G3: kafka_partition_limited diagnostic ---------------------------------

/// PURE decision for the `kafka_partition_limited` diagnostic.
///
/// A consumer group is "partition limited" when it has at least as many member
/// consumers as the topic has partitions AND there is still backlog: extra
/// members sit idle (Kafka assigns at most one consumer per partition) yet lag
/// persists, so adding consumers cannot help -- the topic needs more
/// partitions. This is a DIAGNOSTIC only: it never mutates topology.
///
/// Truth table:
/// - `members >= partitions && lag > 0` -> `true`  (over-provisioned + backlog)
/// - `members <  partitions`            -> `false` (headroom to scale out)
/// - `lag == 0`                         -> `false` (no backlog, not limited)
/// - `partitions == 0`                  -> `false` (no topic info / not limited)
///
/// Kept free-standing and side-effect-free so it is unit-testable without a
/// live broker.
#[cfg(feature = "governor")]
#[must_use]
pub fn partition_limited(members: usize, partitions: usize, lag: u64) -> bool {
    partitions > 0 && members >= partitions && lag > 0
}

/// Time-windowed dedup latch for the `kafka_partition_limited` warning.
///
/// The kafka `SuppressionRule` in this crate is a topic-suffix suppressor
/// (auto-discovery), NOT a rate-limiter -- so the once-per-window dedup is a
/// small purpose-built latch here. [`should_warn`](Self::should_warn) returns
/// `true` at most once per `cooldown`, so a persistently partition-limited
/// consumer logs once per window rather than every recv.
#[cfg(feature = "governor")]
struct PartitionLimitedDiagnostic {
    last_warn: parking_lot::Mutex<Option<std::time::Instant>>,
    cooldown: Duration,
}

#[cfg(feature = "governor")]
impl Default for PartitionLimitedDiagnostic {
    fn default() -> Self {
        Self {
            last_warn: parking_lot::Mutex::new(None),
            // 5 minutes: long enough to avoid spam, short enough to re-surface
            // a persistent condition in logs/alerts. (from_secs over the
            // unstable from_mins; allow the readability lint.)
            #[allow(clippy::duration_suboptimal_units)]
            cooldown: Duration::from_secs(300),
        }
    }
}

#[cfg(feature = "governor")]
impl PartitionLimitedDiagnostic {
    /// Whether to emit the warning now, given the current monotonic time.
    ///
    /// Returns `true` on the first call and then at most once per `cooldown`.
    /// `now` is injected so the dedup window is unit-testable without sleeping.
    fn should_warn_at(&self, now: std::time::Instant) -> bool {
        let mut last = self.last_warn.lock();
        match *last {
            Some(prev) if now.duration_since(prev) < self.cooldown => false,
            _ => {
                *last = Some(now);
                true
            }
        }
    }
}

impl std::fmt::Debug for KafkaTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KafkaTransport")
            .field("subscribed_topics", &*self.subscribed_topics.read())
            .field("closed", &self.closed.load(Ordering::Relaxed))
            .field("healthy", &self.healthy.load(Ordering::Relaxed))
            .field("topic_refresh_active", &self.topic_refresh.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tuning_constants() {
        assert_eq!(tuning::DEFAULT_BATCH_SIZE, 10_000);
        assert_eq!(tuning::MAX_DRAIN_MS, 100);
        assert_eq!(tuning::POLL_TIMEOUT_MS, 50);
    }

    #[test]
    fn test_get_or_insert_topic_cached() {
        let mut map = HashMap::new();
        map.insert("events".to_string(), Arc::from("events"));
        let cache = parking_lot::RwLock::new(map);

        let arc1 = get_or_insert_topic(&cache, "events");
        let arc2 = get_or_insert_topic(&cache, "events");

        // Should return same Arc (pointer equality)
        assert!(Arc::ptr_eq(&arc1, &arc2));
    }

    #[test]
    fn test_get_or_insert_topic_new() {
        let cache = parking_lot::RwLock::new(HashMap::new());

        let arc = get_or_insert_topic(&cache, "new-topic");
        assert_eq!(&*arc, "new-topic");
        assert!(cache.read().contains_key("new-topic"));
        // Insert persists across calls (the previous per-recv clone lost it).
        let arc2 = get_or_insert_topic(&cache, "new-topic");
        assert!(Arc::ptr_eq(&arc, &arc2));
    }

    #[test]
    fn test_kafka_config_defaults() {
        let config = KafkaConfig::default();
        assert_eq!(config.fetch_max_bytes, 52_428_800); // 50MB
        assert!(!config.enable_auto_commit); // Manual commit
    }

    #[tokio::test]
    async fn test_topic_refresh_check_changed_detects_updates() {
        // Simulate the watch channel that TopicRefreshHandle uses internally
        let (tx, rx) = tokio::sync::watch::channel(vec!["events_load".to_string()]);

        let mut handle = topic_resolver::TopicRefreshHandle::new_for_test(rx);

        // Initially no change (first check sees initial value as "no change")
        assert!(handle.check_changed().is_none());

        // Send new topics
        tx.send(vec!["events_load".to_string(), "logs_load".to_string()])
            .unwrap();

        // Now check_changed should return the new list
        let changed = handle.check_changed();
        assert!(changed.is_some());
        let topics = changed.unwrap();
        assert_eq!(topics.len(), 2);
        assert!(topics.contains(&"logs_load".to_string()));

        // Second check with no new changes should return None
        assert!(handle.check_changed().is_none());
    }

    // --- recv-arena: build_batch_from_spans (Task 0.4.3) ------------------
    //
    // These de-risk the recv-arena WITHOUT a live broker: they prove the free
    // function rebuilds messages as zero-copy slices into one shared arena.

    /// Assert that `slice` is a zero-copy view INTO `blob` (a refcounted slice,
    /// not a fresh allocation): its byte range must fall within `blob`'s range.
    /// Mirrors the helper in `work_batch.rs` tests.
    fn assert_within(slice: &bytes::Bytes, blob: &bytes::Bytes) {
        let blob_start = blob.as_ptr() as usize;
        let blob_end = blob_start + blob.len();
        let slice_start = slice.as_ptr() as usize;
        let slice_end = slice_start + slice.len();
        assert!(
            slice_start >= blob_start && slice_end <= blob_end,
            "slice [{slice_start:#x}, {slice_end:#x}) is not within arena \
             [{blob_start:#x}, {blob_end:#x}) -- it is a copy, not a view"
        );
    }

    /// Build an arena + spans by appending payloads back-to-back, exactly as
    /// the poll arms do, returning the frozen arena and the spans.
    fn arena_with(payloads: &[&[u8]]) -> (bytes::Bytes, Vec<Span>) {
        let mut arena: Vec<u8> = Vec::new();
        let mut spans: Vec<Span> = Vec::new();
        for (i, p) in payloads.iter().enumerate() {
            let start = arena.len();
            arena.extend_from_slice(p);
            let end = arena.len();
            let offset = i64::try_from(i).expect("test index fits i64");
            spans.push(Span {
                key: Some(Arc::from("events")),
                token: KafkaToken::new(Arc::from("events"), 0, offset),
                timestamp_ms: Some(1_000 + offset),
                format: PayloadFormat::Auto,
                range: start..end,
            });
        }
        (bytes::Bytes::from(arena), spans)
    }

    #[test]
    fn build_batch_payloads_match_and_in_order() {
        let payloads: &[&[u8]] = &[b"{\"a\":1}", b"hello world", b"[1,2,3]"];
        let (arena, spans) = arena_with(payloads);
        let msgs = build_batch_from_spans(arena, spans);

        assert_eq!(msgs.len(), 3);
        for (i, expected) in payloads.iter().enumerate() {
            assert_eq!(msgs[i].payload.as_ref(), *expected, "payload {i} mismatch");
            // Metadata carried through the span, in order.
            let offset = i64::try_from(i).expect("test index fits i64");
            assert_eq!(msgs[i].token.offset, offset);
            assert_eq!(msgs[i].timestamp_ms, Some(1_000 + offset));
        }
    }

    #[test]
    fn build_batch_payloads_are_views_into_shared_arena() {
        let payloads: &[&[u8]] = &[b"first-record", b"second", b"third-payload-xyz"];
        let (arena, spans) = arena_with(payloads);
        // Keep a clone of the arena Bytes to compare pointer ranges against.
        let arena_ref = arena.clone();
        let msgs = build_batch_from_spans(arena, spans);

        // (b) every payload pointer lies WITHIN the arena -- zero-copy slicing,
        // not copies. This is the core recv-arena contract.
        for m in &msgs {
            assert_within(&m.payload, &arena_ref);
        }
        // All records share the SAME backing allocation (one arena).
        let base = arena_ref.as_ptr() as usize;
        for m in &msgs {
            let off = m.payload.as_ptr() as usize - base;
            assert!(off < arena_ref.len() || m.payload.is_empty());
        }
    }

    #[test]
    fn build_batch_empty_payload_span_yields_empty_slice() {
        // (c) a record with an empty payload (start == end) -> empty slice.
        let payloads: &[&[u8]] = &[b"before", b"", b"after"];
        let (arena, spans) = arena_with(payloads);
        let msgs = build_batch_from_spans(arena, spans);

        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].payload.as_ref(), b"before");
        assert!(
            msgs[1].payload.is_empty(),
            "empty span must yield empty slice"
        );
        assert_eq!(msgs[2].payload.as_ref(), b"after");
    }

    #[test]
    fn build_batch_no_spans_yields_empty_batch() {
        // (d) empty spans -> empty batch (the empty-poll early-return path).
        let msgs = build_batch_from_spans(bytes::Bytes::new(), Vec::new());
        assert!(msgs.is_empty());
    }

    #[test]
    fn build_batch_preserves_key_token_format() {
        let (arena, mut spans) = arena_with(&[b"{\"k\":1}"]);
        // Override the single span's format to assert it is carried through.
        spans[0].format = PayloadFormat::Json;
        let msgs = build_batch_from_spans(arena, spans);
        assert_eq!(msgs[0].key.as_deref(), Some("events"));
        assert_eq!(msgs[0].token.topic.as_ref(), "events");
        assert_eq!(msgs[0].format, PayloadFormat::Json);
    }

    // --- G3: partition_limited diagnostic + gate wiring -------------------

    #[cfg(feature = "governor")]
    #[test]
    fn partition_limited_truth_table() {
        // members >= partitions && lag > 0 -> limited.
        assert!(
            partition_limited(4, 4, 10),
            "equal members+partitions, lag>0"
        );
        assert!(partition_limited(6, 4, 1), "more members than partitions");

        // members < partitions -> headroom to scale out, NOT limited.
        assert!(
            !partition_limited(2, 4, 10),
            "fewer members -> can scale out"
        );

        // lag == 0 -> no backlog, not limited even when over-provisioned.
        assert!(!partition_limited(8, 4, 0), "no lag -> not limited");

        // partitions == 0 (no topic info) -> never a false positive.
        assert!(
            !partition_limited(4, 0, 10),
            "no partition info -> not limited"
        );
        assert!(!partition_limited(0, 0, 0), "all zero -> not limited");
    }

    #[cfg(feature = "governor")]
    #[test]
    fn partition_limited_warn_dedups_within_window() {
        use std::time::{Duration, Instant};

        let diag = PartitionLimitedDiagnostic {
            last_warn: parking_lot::Mutex::new(None),
            #[allow(clippy::duration_suboptimal_units)]
            cooldown: Duration::from_secs(300),
        };
        let t0 = Instant::now();

        // First call in the window -> warns.
        assert!(diag.should_warn_at(t0), "first warning fires");
        // Same window -> suppressed.
        assert!(
            !diag.should_warn_at(t0 + Duration::from_secs(10)),
            "second warning within cooldown is suppressed"
        );
        assert!(
            !diag.should_warn_at(t0 + Duration::from_secs(299)),
            "still within cooldown -> suppressed"
        );
        // Past the cooldown -> warns again exactly once.
        assert!(
            diag.should_warn_at(t0 + Duration::from_secs(301)),
            "after cooldown the warning re-fires once"
        );
        assert!(
            !diag.should_warn_at(t0 + Duration::from_secs(305)),
            "new window re-armed; immediate repeat suppressed"
        );
    }

    /// The Kafka gate actuator drives pause/resume EXACTLY ONCE per edge
    /// through an `InboundGate`, broker-free. We prove the EDGE wiring (the
    /// risky part) with a counting actuator; the live consumer pause/resume
    /// path is left to a broker integration test (Phase 4). The `recv` gate
    /// hook is verified `None`-default no-op by every existing recv test.
    #[cfg(feature = "governor")]
    #[test]
    fn inbound_gate_edge_wiring_drives_actuator_once_per_edge() {
        use crate::governor::{Admit, GateActuator, Hysteresis, InboundGate, UnifiedPressure};
        use crate::governor::{Pressure, PressureSource};
        use std::sync::atomic::{AtomicU64, AtomicUsize};

        struct MockSource(AtomicU64);
        impl PressureSource for MockSource {
            fn name(&self) -> &'static str {
                "mock"
            }
            fn sample(&self) -> Pressure {
                Pressure::new(f64::from_bits(self.0.load(Ordering::Relaxed)))
            }
            fn is_hard(&self) -> bool {
                true
            }
        }

        struct Counter {
            pauses: AtomicUsize,
            resumes: AtomicUsize,
        }
        struct Forward(Arc<Counter>);
        impl GateActuator for Forward {
            fn pause(&self) {
                self.0.pauses.fetch_add(1, Ordering::Relaxed);
            }
            fn resume(&self) {
                self.0.resumes.fetch_add(1, Ordering::Relaxed);
            }
        }

        let src = Arc::new(MockSource(AtomicU64::new(0.1_f64.to_bits())));
        let pressure = Arc::new(UnifiedPressure::new(
            vec![Arc::clone(&src) as Arc<dyn PressureSource>],
            Hysteresis::new(0.80, 0.65).expect("valid band"),
        ));
        let counter = Arc::new(Counter {
            pauses: AtomicUsize::new(0),
            resumes: AtomicUsize::new(0),
        });
        let gate = InboundGate::new(
            Arc::clone(&pressure),
            Box::new(Forward(Arc::clone(&counter))),
        );

        // Low -> open, no calls.
        assert_eq!(gate.evaluate(), Admit::Yes);
        assert_eq!(counter.pauses.load(Ordering::Relaxed), 0);

        // Rising edge -> pause once even across repeated evaluates.
        src.0.store(0.95_f64.to_bits(), Ordering::Relaxed);
        assert_eq!(gate.evaluate(), Admit::Hold);
        assert_eq!(gate.evaluate(), Admit::Hold);
        assert_eq!(
            counter.pauses.load(Ordering::Relaxed),
            1,
            "pause once per edge"
        );

        // Falling edge -> resume once.
        src.0.store(0.10_f64.to_bits(), Ordering::Relaxed);
        assert_eq!(gate.evaluate(), Admit::Yes);
        assert_eq!(
            counter.resumes.load(Ordering::Relaxed),
            1,
            "resume once per edge"
        );
    }

    #[test]
    fn test_subscribed_topics_rwlock_update() {
        // Verify the RwLock pattern used in recv() for subscribed_topics
        let topics = parking_lot::RwLock::new(vec!["events_load".to_string()]);

        // Read path (Debug, metrics)
        assert_eq!(topics.read().len(), 1);

        // Write path (after topic refresh re-subscribe)
        *topics.write() = vec!["events_load".to_string(), "logs_load".to_string()];
        assert_eq!(topics.read().len(), 2);
        assert_eq!(topics.read()[1], "logs_load");
    }
}
