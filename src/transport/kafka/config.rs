// Project:   hyperi-rustlib
// File:      src/transport/kafka/config.rs
// Purpose:   Kafka transport configuration with profiles and config-driven overrides
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Kafka configuration with profile-based defaults and config-driven overrides.
//!
//! ## Profile System
//!
//! HyperI Kafka uses a profile-based configuration system where:
//! 1. A **profile** provides opinionated librdkafka defaults for a use case
//! 2. **User config** can override any librdkafka setting via `librdkafka_overrides`
//! 3. Overrides always win over profile defaults
//!
//! ## Available Profiles
//!
//! - **`production`**: High-throughput, PB/day workloads. Large queues, cooperative
//!   rebalancing, disabled CRC checks, optimized fetch parameters.
//! - **`devtest`**: Development and testing. Relaxed SSL validation, smaller queues,
//!   faster reconnection, debug-friendly settings.
//!
//! ## Example YAML Config
//!
//! ```yaml
//! kafka:
//!   profile: production
//!   brokers:
//!     - kafka1:9092
//!     - kafka2:9092
//!   group: my-consumer-group
//!   topics:
//!     - events
//!   # Override specific librdkafka settings
//!   librdkafka_overrides:
//!     fetch.min.bytes: "2097152"  # 2MB instead of profile's 1MB
//!     statistics.interval.ms: "5000"
//! ```

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

// ============================================================================
// Self-Regulation Profile (Task 0.5: Kafka sizing surface)
// ============================================================================

/// Opinionated sizing profile for the Kafka GET/SEND surface.
///
/// The profile sets default values for all named knobs below. An explicit
/// per-knob value in [`KafkaSizingConfig`] always wins over the profile
/// default. The raw librdkafka escape hatch in [`KafkaConfig`] wins over
/// everything.
///
/// Profiles target the BYTE-level throughput budget and latency envelope:
///
/// | Profile | Use case |
/// |---|---|
/// | `throughput` (default) | PB/day batch ingest, large fanout topics |
/// | `balanced` | Mixed OLTP + analytics, moderate batch size |
/// | `low_latency` | Near-real-time, event-driven, small messages |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelfRegulationProfile {
    /// Maximum throughput: generous byte budgets, tolerates batching delay.
    ///
    /// Consumer: 1 MiB fetch.min.bytes, 50 ms wait, 10 MiB per-partition,
    /// 100 MiB total, 2000 poll-safety cap.
    /// Producer: 128 KiB batch, 20 ms linger, lz4, 64 MiB buffer, 5 in-flight.
    #[default]
    Throughput,

    /// Balanced: moderate batching, 5 ms linger, smaller per-partition budget.
    ///
    /// Consumer: 256 KiB fetch.min.bytes, 25 ms wait, 5 MiB per-partition,
    /// 50 MiB total, 1000 poll-safety cap.
    /// Producer: 64 KiB batch, 5 ms linger, lz4, 32 MiB buffer, 5 in-flight.
    Balanced,

    /// Low latency: minimal batching delay, smaller buffers.
    ///
    /// Consumer: 1 byte fetch.min.bytes, 5 ms wait, 1 MiB per-partition,
    /// 10 MiB total, 500 poll-safety cap.
    /// Producer: 16 KiB batch, 0 ms linger, lz4, 16 MiB buffer, 5 in-flight.
    LowLatency,
}

impl SelfRegulationProfile {
    /// Return the consumer knob defaults for this profile.
    #[must_use]
    pub fn consumer_defaults(self) -> ConsumerKnobs {
        match self {
            Self::Throughput => ConsumerKnobs {
                // 1 MiB -- forces broker to batch at least one full record page.
                fetch_min_bytes: Some(1_048_576),
                // 50 ms -- gives broker time to fill the 1 MiB budget.
                fetch_max_wait_ms: Some(50),
                // 10 MiB -- generous per-partition ceiling for wide topics.
                max_partition_fetch_bytes: Some(10_485_760),
                // 100 MiB -- caps total network fetch per round-trip.
                fetch_max_bytes: Some(104_857_600),
                // 2000 -- poll-safety cap enforced by the recv() loop.
                max_poll_records: Some(2000),
            },
            Self::Balanced => ConsumerKnobs {
                fetch_min_bytes: Some(262_144), // 256 KiB
                fetch_max_wait_ms: Some(25),
                max_partition_fetch_bytes: Some(5_242_880), // 5 MiB
                fetch_max_bytes: Some(52_428_800),          // 50 MiB
                max_poll_records: Some(1000),
            },
            Self::LowLatency => ConsumerKnobs {
                fetch_min_bytes: Some(1),                   // no batching threshold
                fetch_max_wait_ms: Some(5),                 // return fast
                max_partition_fetch_bytes: Some(1_048_576), // 1 MiB
                fetch_max_bytes: Some(10_485_760),          // 10 MiB
                max_poll_records: Some(500),
            },
        }
    }

    /// Return the producer knob defaults for this profile.
    #[must_use]
    pub fn producer_defaults(self) -> ProducerKnobs {
        match self {
            Self::Throughput => ProducerKnobs {
                // 128 KiB per MessageSet -- batches up fast but not excessive.
                batch_size_bytes: Some(131_072),
                // 20 ms -- enough time to fill the 128 KiB batch.
                linger_ms: Some(20),
                // lz4 default; zstd is opt-in for storage-bound topics.
                compression_type: Some("lz4".to_string()),
                // 64 MiB total producer queue (queue.buffering.max.kbytes in KiB).
                buffer_memory_bytes: Some(67_108_864),
                // 5 in-flight per connection -- matches exactly-once safe limit.
                max_in_flight: Some(5),
            },
            Self::Balanced => ProducerKnobs {
                batch_size_bytes: Some(65_536), // 64 KiB
                linger_ms: Some(5),
                compression_type: Some("lz4".to_string()),
                buffer_memory_bytes: Some(33_554_432), // 32 MiB
                max_in_flight: Some(5),
            },
            Self::LowLatency => ProducerKnobs {
                batch_size_bytes: Some(16_384), // 16 KiB
                linger_ms: Some(0),             // send immediately
                compression_type: Some("lz4".to_string()),
                buffer_memory_bytes: Some(16_777_216), // 16 MiB
                max_in_flight: Some(5),
            },
        }
    }
}

/// Named consumer sizing knobs.
///
/// All fields are `Option<T>`: `None` means "use the profile default".
/// An explicit `Some(v)` wins over the profile default.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsumerKnobs {
    /// Minimum bytes the broker must have ready before responding to a Fetch.
    ///
    /// librdkafka: `fetch.min.bytes` (default 1 byte).
    /// Raising this batches more data per round-trip but adds latency when
    /// topic traffic is low.
    #[serde(default)]
    pub fetch_min_bytes: Option<i32>,

    /// Maximum milliseconds the broker may wait to fill `fetch.min.bytes`.
    ///
    /// librdkafka: `fetch.wait.max.ms` (default 500 ms).
    /// Works in tandem with `fetch_min_bytes` -- the broker returns whatever
    /// it has when this timer fires even if `fetch.min.bytes` is not met.
    #[serde(default)]
    pub fetch_max_wait_ms: Option<u32>,

    /// Maximum bytes returned per partition per Fetch request.
    ///
    /// librdkafka: `max.partition.fetch.bytes` (alias `fetch.message.max.bytes`,
    /// default 1 MiB). Must be >= the topic's `max.message.bytes`.
    #[serde(default)]
    pub max_partition_fetch_bytes: Option<i32>,

    /// Maximum total bytes returned by the broker for a single Fetch request
    /// across all partitions.
    ///
    /// librdkafka: `fetch.max.bytes` (default 50 MiB).
    #[serde(default)]
    pub fetch_max_bytes: Option<i32>,

    /// Maximum number of messages the recv() loop returns per call.
    ///
    /// NOTE: `max.poll.records` does NOT exist in librdkafka -- there is no
    /// broker-level property for this. This is a purely CLIENT-SIDE cap,
    /// enforced by passing this value as the `max` argument to
    /// `KafkaTransport::recv()` via `KafkaSizingConfig::effective_poll_cap()`.
    /// It bounds the batch size delivered to the WorkBatch layer, not the
    /// network fetch size (which is byte-governed by the knobs above).
    #[serde(default)]
    pub max_poll_records: Option<usize>,
}

/// Named producer sizing knobs.
///
/// All fields are `Option<T>`: `None` means "use the profile default".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProducerKnobs {
    /// Maximum bytes per MessageSet (librdkafka `batch.size`, default 1 MiB).
    ///
    /// This is the PER-BATCH ceiling, not the total queue size. Raise this for
    /// fewer, larger network writes. Note: `batch.size` in librdkafka is in
    /// bytes (matches the Java Kafka client name and unit).
    #[serde(default)]
    pub batch_size_bytes: Option<i32>,

    /// Accumulation delay before transmitting a MessageSet.
    ///
    /// librdkafka: `linger.ms` (alias for `queue.buffering.max.ms`, default 5 ms).
    /// Higher values fill larger batches; 0 sends immediately.
    #[serde(default)]
    pub linger_ms: Option<u32>,

    /// Compression codec for MessageSets.
    ///
    /// librdkafka: `compression.type` (alias for `compression.codec`).
    /// Valid values: `none`, `gzip`, `snappy`, `lz4`, `zstd`.
    /// Default (all profiles): `lz4` -- best throughput/ratio tradeoff.
    /// Use `zstd` for storage-bound topics that can absorb the CPU cost.
    #[serde(default)]
    pub compression_type: Option<String>,

    /// Total byte budget for the producer's in-memory queue.
    ///
    /// librdkafka: `queue.buffering.max.kbytes` (in KiB, default 1 GiB).
    /// This is the TOTAL queue, not per-batch. Set lower to bound memory
    /// usage in containers. Stored as bytes in this struct; divided by 1024
    /// when applied to librdkafka.
    #[serde(default)]
    pub buffer_memory_bytes: Option<u64>,

    /// Maximum concurrent in-flight requests per broker connection.
    ///
    /// librdkafka: `max.in.flight.requests.per.connection` (default 1,000,000).
    /// Set to 5 to match the exactly-once safe limit (KIP-98) and to bound
    /// memory/reorder window. Matches the Java Kafka producer default for
    /// idempotent producers.
    #[serde(default)]
    pub max_in_flight: Option<u32>,
}

/// Kafka sizing surface: profile + named per-knob overrides + raw escape hatch.
///
/// Resolution precedence (lowest to highest):
/// 1. `SelfRegulationProfile` defaults
/// 2. Named knobs in `consumer` / `producer` (explicit `Some(v)` wins)
/// 3. Raw librdkafka maps `consumer_librdkafka` / `producer_librdkafka` (wins
///    over everything, applied last via `ClientConfig::set`)
///
/// The raw maps are logged (one line per key) when they override a property
/// that the sizing surface depends on (the fetch byte sizes and
/// `enable.auto.commit`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct KafkaSizingConfig {
    /// Sizing profile (throughput / balanced / low_latency).
    #[serde(default)]
    pub profile: SelfRegulationProfile,

    /// Per-knob consumer overrides (any `Some(v)` beats the profile default).
    #[serde(default)]
    pub consumer: ConsumerKnobs,

    /// Per-knob producer overrides (any `Some(v)` beats the profile default).
    #[serde(default)]
    pub producer: ProducerKnobs,

    /// Raw librdkafka consumer properties applied LAST, winning over everything.
    ///
    /// Keys must be valid librdkafka property names (e.g. `fetch.wait.max.ms`).
    /// An invalid key silently no-ops in librdkafka -- double-check spelling.
    #[serde(default)]
    pub consumer_librdkafka: BTreeMap<String, String>,

    /// Raw librdkafka producer properties applied LAST, winning over everything.
    ///
    /// Keys must be valid librdkafka property names (e.g. `linger.ms`).
    #[serde(default)]
    pub producer_librdkafka: BTreeMap<String, String>,
}

// ============================================================================
// Keys the sizing governor depends on -- logged when raw-overridden.
// ============================================================================

/// Consumer property names whose values the sizing governor reads to compute
/// byte budgets. When the raw escape hatch overrides one of these, we log a
/// warning so the operator knows the governor's assumptions have changed.
const GOVERNOR_CONSUMER_KEYS: &[&str] = &[
    "fetch.min.bytes",
    "fetch.max.bytes",
    "fetch.wait.max.ms",
    "max.partition.fetch.bytes",
    "fetch.message.max.bytes",
    "enable.auto.commit",
];

/// Producer property names the sizing governor sets.
const GOVERNOR_PRODUCER_KEYS: &[&str] = &[
    "batch.size",
    "linger.ms",
    "queue.buffering.max.ms",
    "compression.type",
    "compression.codec",
    "queue.buffering.max.kbytes",
    "max.in.flight.requests.per.connection",
    "partitioner",
    "sticky.partitioning.linger.ms",
];

impl KafkaSizingConfig {
    /// Resolve the effective consumer librdkafka key/value map.
    ///
    /// Precedence: profile defaults < named knobs < raw `consumer_librdkafka`.
    ///
    /// This is a PURE function -- suitable for unit testing without a live
    /// broker. The caller feeds the returned map into `ClientConfig::set`.
    #[must_use]
    pub fn resolved_consumer_map(&self) -> BTreeMap<String, String> {
        let profile_knobs = self.profile.consumer_defaults();

        // Merge: explicit `Some` wins over profile default.
        let fetch_min_bytes = self
            .consumer
            .fetch_min_bytes
            .or(profile_knobs.fetch_min_bytes)
            .unwrap_or(1);
        let fetch_max_wait_ms = self
            .consumer
            .fetch_max_wait_ms
            .or(profile_knobs.fetch_max_wait_ms)
            .unwrap_or(500);
        let max_partition_fetch_bytes = self
            .consumer
            .max_partition_fetch_bytes
            .or(profile_knobs.max_partition_fetch_bytes)
            .unwrap_or(1_048_576);
        let fetch_max_bytes = self
            .consumer
            .fetch_max_bytes
            .or(profile_knobs.fetch_max_bytes)
            .unwrap_or(52_428_800);

        let mut map = BTreeMap::new();
        map.insert("fetch.min.bytes".to_string(), fetch_min_bytes.to_string());
        map.insert(
            "fetch.wait.max.ms".to_string(),
            fetch_max_wait_ms.to_string(),
        );
        map.insert(
            "max.partition.fetch.bytes".to_string(),
            max_partition_fetch_bytes.to_string(),
        );
        map.insert("fetch.max.bytes".to_string(), fetch_max_bytes.to_string());

        // Apply the raw escape hatch last -- it wins.
        for (k, v) in &self.consumer_librdkafka {
            if GOVERNOR_CONSUMER_KEYS.contains(&k.as_str()) {
                tracing::warn!(
                    key = k.as_str(),
                    value = v.as_str(),
                    "kafka sizing: raw consumer_librdkafka overrides a governor key"
                );
            }
            map.insert(k.clone(), v.clone());
        }

        map
    }

    /// Resolve the effective producer librdkafka key/value map.
    ///
    /// Precedence: profile defaults < named knobs < raw `producer_librdkafka`.
    ///
    /// KIP-794 note: librdkafka does not support `partitioner.ignore.keys` (a
    /// Java-client-only property). The librdkafka equivalent for uniform sticky
    /// null-key distribution is `sticky.partitioning.linger.ms` (default 10 ms,
    /// works with the `consistent_random` default partitioner). We set this to
    /// `linger_ms` so null-key batches accumulate for one full linger window
    /// before rotation, which is the closest functional match to KIP-794's
    /// intent for the librdkafka client.
    ///
    /// This is a PURE function -- suitable for unit testing without a live broker.
    #[must_use]
    pub fn resolved_producer_map(&self) -> BTreeMap<String, String> {
        let profile_knobs = self.profile.producer_defaults();

        let batch_size_bytes = self
            .producer
            .batch_size_bytes
            .or(profile_knobs.batch_size_bytes)
            .unwrap_or(1_000_000);
        let linger_ms = self
            .producer
            .linger_ms
            .or(profile_knobs.linger_ms)
            .unwrap_or(5);
        let compression_type = self
            .producer
            .compression_type
            .clone()
            .or(profile_knobs.compression_type)
            .unwrap_or_else(|| "none".to_string());
        let buffer_memory_bytes = self
            .producer
            .buffer_memory_bytes
            .or(profile_knobs.buffer_memory_bytes)
            .unwrap_or(1_073_741_824); // 1 GiB (librdkafka default)
        let max_in_flight = self
            .producer
            .max_in_flight
            .or(profile_knobs.max_in_flight)
            .unwrap_or(1_000_000);

        // queue.buffering.max.kbytes is in KiB -- convert from bytes.
        let buffer_kib = (buffer_memory_bytes / 1024).max(1);

        let mut map = BTreeMap::new();
        map.insert("batch.size".to_string(), batch_size_bytes.to_string());
        map.insert("linger.ms".to_string(), linger_ms.to_string());
        map.insert("compression.type".to_string(), compression_type);
        map.insert(
            "queue.buffering.max.kbytes".to_string(),
            buffer_kib.to_string(),
        );
        map.insert(
            "max.in.flight.requests.per.connection".to_string(),
            max_in_flight.to_string(),
        );

        // KIP-794 / uniform sticky for null-keyed messages.
        // `partitioner.ignore.keys` is a Java-client-only property and does
        // NOT exist in librdkafka. The librdkafka equivalent is to keep the
        // default `consistent_random` partitioner (null keys -> random
        // partition) and set `sticky.partitioning.linger.ms` equal to the
        // linger window so null-key batches stick to one partition until the
        // batch is full, then rotate. This is the closest functional match to
        // KIP-794 available in librdkafka.
        //
        // We do NOT set `partitioner` here to avoid overriding any caller-
        // supplied value (keyed RoutedSender paths set their own partitioner).
        map.insert(
            "sticky.partitioning.linger.ms".to_string(),
            linger_ms.to_string(),
        );

        // Apply the raw escape hatch last -- it wins.
        for (k, v) in &self.producer_librdkafka {
            if GOVERNOR_PRODUCER_KEYS.contains(&k.as_str()) {
                tracing::warn!(
                    key = k.as_str(),
                    value = v.as_str(),
                    "kafka sizing: raw producer_librdkafka overrides a governor key"
                );
            }
            map.insert(k.clone(), v.clone());
        }

        map
    }

    /// Return the effective poll-safety cap (max messages per recv() call).
    ///
    /// This is a CLIENT-SIDE cap only -- there is no librdkafka property for
    /// `max.poll.records`. The value is passed as the `max` argument to
    /// `KafkaTransport::recv()` by the ServiceRuntime / WorkBatch layer.
    #[must_use]
    pub fn effective_poll_cap(&self) -> usize {
        self.consumer
            .max_poll_records
            .or(self.profile.consumer_defaults().max_poll_records)
            .unwrap_or(10_000)
    }
}

// ============================================================================
// Topic Resolution Types
// ============================================================================

/// Topic suppression rule for auto-discovery.
///
/// When auto-discovering topics, if a topic with `preferred_suffix` exists
/// for a base name, the topic with `suppressed_suffix` for that same base
/// is removed. Default: `_load` suppresses `_land`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuppressionRule {
    /// The suffix of the preferred (kept) topic.
    pub preferred_suffix: String,
    /// The suffix of the suppressed (removed) topic.
    pub suppressed_suffix: String,
}

// ============================================================================
// Profile System
// ============================================================================

/// Kafka configuration profile.
///
/// Profiles provide opinionated librdkafka defaults for specific use cases.
/// Users can override any setting via `librdkafka_overrides`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KafkaProfile {
    /// Production profile: lean baseline for all DFE services.
    ///
    /// Only sets values that differ from librdkafka defaults.
    /// Services add overrides via `librdkafka_overrides`.
    #[default]
    Production,

    /// Development/test profile: fast iteration, low memory.
    ///
    /// Cooperative rebalancing, fast reconnects, debug logging.
    /// SSL certificate verification disabled by default.
    DevTest,
}

impl FromStr for KafkaProfile {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "production" | "prod" => Ok(Self::Production),
            "devtest" | "dev" | "test" | "development" => Ok(Self::DevTest),
            _ => Err(format!(
                "Unknown Kafka profile: {s}. Valid: production, devtest"
            )),
        }
    }
}

impl std::fmt::Display for KafkaProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Production => write!(f, "production"),
            Self::DevTest => write!(f, "devtest"),
        }
    }
}

// ============================================================================
// Merge Helper
// ============================================================================

/// Merge profile defaults with user overrides.
///
/// Starts with `profile` defaults, then applies `overrides` on top.
/// User overrides always win. Returns the final merged map.
///
/// # Example
///
/// ```rust,ignore
/// use std::collections::HashMap;
/// use hyperi_rustlib::transport::kafka::config::{merge_with_overrides, PRODUCTION_PROFILE};
///
/// let mut overrides = HashMap::new();
/// overrides.insert("fetch.min.bytes".to_string(), "2097152".to_string());
///
/// let merged = merge_with_overrides(PRODUCTION_PROFILE, &overrides);
/// assert_eq!(merged.get("fetch.min.bytes").unwrap(), "2097152");
/// assert_eq!(merged.get("partition.assignment.strategy").unwrap(), "cooperative-sticky");
/// ```
#[must_use]
pub fn merge_with_overrides<S: std::hash::BuildHasher>(
    profile: &[(&str, &str)],
    overrides: &HashMap<String, String, S>,
) -> HashMap<String, String> {
    let mut config = HashMap::with_capacity(profile.len() + overrides.len());

    for (key, value) in profile {
        config.insert((*key).to_string(), (*value).to_string());
    }
    for (key, value) in overrides {
        config.insert(key.clone(), value.clone());
    }

    config
}

// ============================================================================
// Profile Defaults
// ============================================================================

/// Production consumer profile -- lean baseline.
///
/// Only settings that differ from librdkafka defaults with clear justification.
/// Services override on an exception basis via `librdkafka_overrides`.
///
/// | Setting | Value | librdkafka default | Why |
/// |---|---|---|---|
/// | `partition.assignment.strategy` | `cooperative-sticky` | `range,roundrobin` | KIP-429: avoids stop-the-world rebalances |
/// | `fetch.min.bytes` | 1 MiB | 1 byte | Batch fetches for throughput |
/// | `fetch.wait.max.ms` | 100 ms | 500 ms | Bound latency when fetch.min.bytes not met |
/// | `queued.min.messages` | 20000 | 100000 | 10-20K batches are most efficient |
/// | `enable.auto.commit` | false | true | DFE services manage offset commits |
/// | `statistics.interval.ms` | 1000 ms | 0 (disabled) | Enable Prometheus metrics |
pub const PRODUCTION_PROFILE: &[(&str, &str)] = &[
    ("partition.assignment.strategy", "cooperative-sticky"),
    ("fetch.min.bytes", "1048576"),
    ("fetch.wait.max.ms", "100"),
    ("queued.min.messages", "20000"),
    ("enable.auto.commit", "false"),
    ("statistics.interval.ms", "1000"),
];

/// Development/test consumer profile -- minimal latency, low memory.
///
/// Inherits the same "only non-defaults" philosophy. Optimised for fast
/// iteration on developer machines: no fetch batching, smaller queues,
/// fast reconnects, debug logging.
///
/// | Setting | Value | librdkafka default | Why |
/// |---|---|---|---|
/// | `partition.assignment.strategy` | `cooperative-sticky` | `range,roundrobin` | Consistent across all environments |
/// | `queued.min.messages` | 1000 | 100000 | Lower memory for dev machines |
/// | `enable.auto.commit` | false | true | DFE services manage commits |
/// | `reconnect.backoff.ms` | 10 ms | 100 ms | Fast reconnect for quick iteration |
/// | `reconnect.backoff.max.ms` | 100 ms | 10000 ms | Cap quickly |
/// | `log.connection.close` | true | false | Debug-friendly |
/// | `statistics.interval.ms` | 1000 ms | 0 (disabled) | Enable metrics even in dev |
pub const DEVTEST_PROFILE: &[(&str, &str)] = &[
    ("partition.assignment.strategy", "cooperative-sticky"),
    ("queued.min.messages", "1000"),
    ("enable.auto.commit", "false"),
    ("reconnect.backoff.ms", "10"),
    ("reconnect.backoff.max.ms", "100"),
    ("log.connection.close", "true"),
    ("statistics.interval.ms", "1000"),
];

// ============================================================================
// Producer Profile Defaults
// ============================================================================

/// High-throughput producer -- lean baseline.
///
/// Only settings that differ from librdkafka defaults.
/// Services override via `librdkafka_overrides`.
///
/// | Setting | Value | librdkafka default | Why |
/// |---|---|---|---|
/// | `linger.ms` | 100 ms | 5 ms | Accumulate larger batches |
/// | `compression.type` | zstd | none | Best ratio with good CPU |
/// | `socket.nagle.disable` | true | false | Kafka batches at app level |
/// | `statistics.interval.ms` | 1000 ms | 0 (disabled) | Enable Prometheus metrics |
pub const PRODUCER_HIGH_THROUGHPUT: &[(&str, &str)] = &[
    ("linger.ms", "100"),
    ("compression.type", "zstd"),
    ("socket.nagle.disable", "true"),
    ("statistics.interval.ms", "1000"),
];

/// Exactly-once producer -- idempotence + ordering.
///
/// Only settings that differ from librdkafka defaults.
/// `acks=all` and `max.in.flight=5` are already defaults but explicit
/// here because they are *invariants* for exactly-once correctness.
///
/// | Setting | Value | librdkafka default | Why |
/// |---|---|---|---|
/// | `enable.idempotence` | true | false | Exactly-once within partition |
/// | `acks` | all | all (-1) | Invariant for EOS (explicit) |
/// | `max.in.flight.requests.per.connection` | 5 | 1000000 | Max for idempotent producer |
/// | `linger.ms` | 20 ms | 5 ms | Moderate batching |
/// | `compression.type` | zstd | none | Best ratio |
/// | `socket.nagle.disable` | true | false | Kafka batches at app level |
/// | `statistics.interval.ms` | 1000 ms | 0 | Enable metrics |
pub const PRODUCER_EXACTLY_ONCE: &[(&str, &str)] = &[
    ("enable.idempotence", "true"),
    ("acks", "all"),
    ("max.in.flight.requests.per.connection", "5"),
    ("linger.ms", "20"),
    ("compression.type", "zstd"),
    ("socket.nagle.disable", "true"),
    ("statistics.interval.ms", "1000"),
];

/// Low-latency producer -- minimal delay, leader-ack only.
///
/// Only settings that differ from librdkafka defaults.
///
/// | Setting | Value | librdkafka default | Why |
/// |---|---|---|---|
/// | `acks` | 1 | all (-1) | Leader ack only for speed |
/// | `linger.ms` | 0 ms | 5 ms | Send immediately |
/// | `compression.type` | lz4 | none | LZ4 is fastest codec |
/// | `socket.nagle.disable` | true | false | No TCP coalescing |
/// | `statistics.interval.ms` | 1000 ms | 0 | Enable metrics |
pub const PRODUCER_LOW_LATENCY: &[(&str, &str)] = &[
    ("acks", "1"),
    ("linger.ms", "0"),
    ("compression.type", "lz4"),
    ("socket.nagle.disable", "true"),
    ("statistics.interval.ms", "1000"),
];

/// DevTest producer -- fast acks, no compression.
///
/// Only settings that differ from librdkafka defaults.
///
/// | Setting | Value | librdkafka default | Why |
/// |---|---|---|---|
/// | `acks` | 1 | all (-1) | Faster for dev |
/// | `linger.ms` | 5 ms | 5 ms | Default is fine for dev |
/// | `compression.type` | none | none | No overhead in dev |
/// | `socket.nagle.disable` | true | false | No TCP coalescing |
/// | `statistics.interval.ms` | 1000 ms | 0 | Enable metrics in dev |
pub const PRODUCER_DEVTEST: &[(&str, &str)] = &[
    ("acks", "1"),
    ("socket.nagle.disable", "true"),
    ("statistics.interval.ms", "1000"),
];

/// Legacy producer defaults -- now aliases `PRODUCER_HIGH_THROUGHPUT`.
#[deprecated(since = "2.0.0", note = "Use PRODUCER_HIGH_THROUGHPUT instead")]
pub const PRODUCER_DEFAULTS: &[(&str, &str)] = PRODUCER_HIGH_THROUGHPUT;

// ============================================================================
// Legacy Constants (for backward compatibility)
// ============================================================================

/// Alias for `PRODUCTION_PROFILE` (backward compatibility).
#[deprecated(since = "2.0.0", note = "Use PRODUCTION_PROFILE instead")]
pub const HIGH_THROUGHPUT_CONSUMER_DEFAULTS: &[(&str, &str)] = PRODUCTION_PROFILE;

/// Low-latency consumer -- minimal fetch delay.
///
/// Only settings that differ from librdkafka defaults.
///
/// | Setting | Value | librdkafka default | Why |
/// |---|---|---|---|
/// | `partition.assignment.strategy` | `cooperative-sticky` | `range,roundrobin` | Consistent across envs |
/// | `fetch.wait.max.ms` | 10 ms | 500 ms | Return quickly |
/// | `queued.min.messages` | 1000 | 100000 | Smaller pre-fetch queue |
/// | `enable.auto.commit` | false | true | DFE manages commits |
/// | `reconnect.backoff.ms` | 10 ms | 100 ms | Fast reconnect |
/// | `reconnect.backoff.max.ms` | 100 ms | 10000 ms | Cap quickly |
/// | `statistics.interval.ms` | 1000 ms | 0 | Enable metrics |
pub const LOW_LATENCY_CONSUMER_DEFAULTS: &[(&str, &str)] = &[
    ("partition.assignment.strategy", "cooperative-sticky"),
    ("fetch.wait.max.ms", "10"),
    ("queued.min.messages", "1000"),
    ("enable.auto.commit", "false"),
    ("reconnect.backoff.ms", "10"),
    ("reconnect.backoff.max.ms", "100"),
    ("statistics.interval.ms", "1000"),
];

// ============================================================================
// Configuration Struct
// ============================================================================

/// Kafka transport configuration.
///
/// Uses a profile-based system where profiles provide opinionated defaults,
/// and `librdkafka_overrides` allows overriding any setting.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // Kafka config legitimately has many boolean flags
pub struct KafkaConfig {
    /// Configuration profile (production, devtest).
    ///
    /// The profile provides baseline librdkafka settings optimized for the use case.
    /// Use `librdkafka_overrides` to customize specific settings.
    #[serde(default)]
    pub profile: KafkaProfile,

    /// Kafka broker addresses.
    #[serde(default = "default_brokers")]
    pub brokers: Vec<String>,

    /// Consumer group ID.
    #[serde(default = "default_group")]
    pub group: String,

    /// Client ID for identification in broker logs.
    #[serde(default = "default_client_id")]
    pub client_id: String,

    /// Topics to subscribe to.
    #[serde(default)]
    pub topics: Vec<String>,

    /// Enable auto-discovery when `topics` is empty.
    /// When false (default), empty `topics` means no subscription (producer-only).
    /// When true, empty `topics` triggers broker auto-discovery with
    /// `topic_include`/`topic_exclude` filters and suppression rules.
    #[serde(default)]
    pub auto_discover: bool,

    /// Regex patterns for topic include filtering (empty = accept all).
    /// Topics must match at least one pattern (OR logic).
    #[serde(default)]
    pub topic_include: Vec<String>,

    /// Regex patterns for topic exclude filtering.
    /// Topics matching any pattern are excluded. Exclude wins over include.
    /// Default: `["^__"]` (excludes Kafka internal topics like `__consumer_offsets`).
    #[serde(default = "default_topic_exclude")]
    pub topic_exclude: Vec<String>,

    /// Periodic topic refresh interval in seconds (0 = disabled).
    /// Only applies when `topics` is empty (auto-discovery mode).
    #[serde(default = "default_topic_refresh_secs")]
    pub topic_refresh_secs: u64,

    /// Suppression rules: if a topic with preferred_suffix exists,
    /// suppress the topic with suppressed_suffix for the same base name.
    /// Default: _load suppresses _land (DFE convention).
    #[serde(default = "default_topic_suppression_rules")]
    pub topic_suppression_rules: Vec<SuppressionRule>,

    /// Security protocol (plaintext, ssl, sasl_plaintext, sasl_ssl).
    #[serde(default = "default_security_protocol")]
    pub security_protocol: String,

    /// SASL mechanism (PLAIN, SCRAM-SHA-256, SCRAM-SHA-512, OAUTHBEARER).
    #[serde(default)]
    pub sasl_mechanism: Option<String>,

    /// SASL username.
    #[serde(default)]
    pub sasl_username: Option<String>,

    /// SASL password.
    #[serde(default)]
    pub sasl_password: Option<crate::SensitiveString>,

    // --- TLS Configuration ---
    //
    // Kafka TLS is configured by FILE PATH because librdkafka (the C library)
    // owns the TLS stack and reads PEM files directly -- it does not accept a
    // Rust rustls `ClientConfig`, so the unified `crate::tls` module does not
    // apply here. The mapping from the unified `TlsTrust` vocabulary is:
    //   TlsTrust.extra_roots (private CA, single bundle ok) -> ssl_ca_location
    //   client cert / key (mTLS)                             -> ssl_certificate_location / ssl_key_location
    //   native/webpki roots                                  -> librdkafka uses the system store by default (omit ssl_ca_location)
    //   exclusive private-CA pin                             -> set ssl_ca_location to the private CA only
    /// SSL CA certificate file path (private-CA bundle; maps to
    /// `TlsTrust.extra_roots` -- a single combined root+intermediate PEM is
    /// accepted).
    #[serde(default)]
    pub ssl_ca_location: Option<String>,

    /// SSL client certificate file path (mTLS).
    #[serde(default)]
    pub ssl_certificate_location: Option<String>,

    /// SSL client key file path (mTLS).
    #[serde(default)]
    pub ssl_key_location: Option<String>,

    /// Skip SSL certificate verification.
    ///
    /// Automatically enabled for `devtest` profile.
    #[serde(default)]
    pub ssl_skip_verify: bool,

    /// Deliberately permit an unencrypted transport (`plaintext` /
    /// `sasl_plaintext`) in production. Default `false`: production
    /// [`validate`](KafkaConfig::validate) rejects unencrypted transports so a
    /// misconfiguration cannot ship data/credentials in the clear. Set `true`
    /// only for an audited case (e.g. mesh-encrypted in-cluster traffic).
    #[serde(default)]
    pub allow_insecure_transport: bool,

    // --- Consumer Settings (explicit fields for common options) ---
    /// Enable auto-commit (default: false for manual commit).
    #[serde(default)]
    pub enable_auto_commit: bool,

    /// Auto-commit interval in milliseconds.
    #[serde(default = "default_auto_commit_interval")]
    pub auto_commit_interval_ms: u32,

    /// Session timeout in milliseconds.
    #[serde(default = "default_session_timeout")]
    pub session_timeout_ms: u32,

    /// Heartbeat interval in milliseconds.
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval_ms: u32,

    /// Maximum poll interval in milliseconds.
    #[serde(default = "default_max_poll_interval")]
    pub max_poll_interval_ms: u32,

    /// Fetch minimum bytes.
    #[serde(default = "default_fetch_min_bytes")]
    pub fetch_min_bytes: i32,

    /// Fetch maximum bytes.
    #[serde(default = "default_fetch_max_bytes")]
    pub fetch_max_bytes: i32,

    /// Maximum messages per partition per poll.
    #[serde(default = "default_max_partition_fetch_bytes")]
    pub max_partition_fetch_bytes: i32,

    /// Auto offset reset (earliest, latest, none).
    #[serde(default = "default_auto_offset_reset")]
    pub auto_offset_reset: String,

    /// Enable partition EOF events.
    #[serde(default)]
    pub enable_partition_eof: bool,

    /// Kafka sizing surface: profile + named knobs + raw librdkafka escape hatch.
    ///
    /// Controls the byte-budget and latency envelope for GET (consumer) and
    /// SEND (producer) paths. See [`KafkaSizingConfig`] for full documentation.
    ///
    /// Example YAML:
    /// ```yaml
    /// kafka:
    ///   sizing:
    ///     profile: throughput
    ///     consumer:
    ///       fetch_min_bytes: 2097152  # 2 MiB, overrides profile default
    ///     producer:
    ///       compression_type: zstd    # opt into zstd for storage-bound topics
    ///     consumer_librdkafka:
    ///       fetch.wait.max.ms: "75"   # raw override wins over everything
    ///     producer_librdkafka:
    ///       linger.ms: "50"
    /// ```
    #[serde(default)]
    pub sizing: KafkaSizingConfig,

    /// Librdkafka configuration overrides.
    ///
    /// These settings override both the profile defaults and explicit config fields.
    /// Use this to customize any librdkafka setting not exposed as an explicit field.
    ///
    /// Example:
    /// ```yaml
    /// librdkafka_overrides:
    ///   statistics.interval.ms: "5000"
    ///   fetch.min.bytes: "2097152"
    /// ```
    #[serde(default)]
    pub librdkafka_overrides: HashMap<String, String>,

    /// Legacy field - use `librdkafka_overrides` instead.
    #[serde(default)]
    #[deprecated(since = "1.3.0", note = "Use `librdkafka_overrides` instead")]
    pub extra_config: HashMap<String, String>,

    /// Inbound message filters (applied on recv before caller sees messages).
    #[serde(default)]
    pub filters_in: Vec<crate::transport::filter::FilterRule>,

    /// Outbound message filters (applied on send before transport dispatches).
    #[serde(default)]
    pub filters_out: Vec<crate::transport::filter::FilterRule>,
}

fn default_topic_exclude() -> Vec<String> {
    vec!["^__".to_string()]
}

fn default_topic_refresh_secs() -> u64 {
    60
}

fn default_topic_suppression_rules() -> Vec<SuppressionRule> {
    vec![SuppressionRule {
        preferred_suffix: "_load".into(),
        suppressed_suffix: "_land".into(),
    }]
}

fn default_brokers() -> Vec<String> {
    vec!["localhost:9092".to_string()]
}

fn default_group() -> String {
    "hyperi-rustlib-consumer".to_string()
}

fn default_client_id() -> String {
    "hyperi-rustlib".to_string()
}

fn default_security_protocol() -> String {
    "plaintext".to_string()
}

fn default_auto_commit_interval() -> u32 {
    5000
}

fn default_session_timeout() -> u32 {
    45000
}

fn default_heartbeat_interval() -> u32 {
    3000
}

fn default_max_poll_interval() -> u32 {
    300_000
}

fn default_fetch_min_bytes() -> i32 {
    1
}

fn default_fetch_max_bytes() -> i32 {
    52_428_800 // 50 MB
}

fn default_max_partition_fetch_bytes() -> i32 {
    1_048_576 // 1 MB
}

fn default_auto_offset_reset() -> String {
    "earliest".to_string()
}

impl Default for KafkaConfig {
    fn default() -> Self {
        #[allow(deprecated)]
        Self {
            profile: KafkaProfile::default(),
            brokers: default_brokers(),
            group: default_group(),
            client_id: default_client_id(),
            topics: Vec::new(),
            auto_discover: false,
            topic_include: Vec::new(),
            topic_exclude: default_topic_exclude(),
            topic_refresh_secs: default_topic_refresh_secs(),
            topic_suppression_rules: default_topic_suppression_rules(),
            security_protocol: default_security_protocol(),
            sasl_mechanism: None,
            sasl_username: None,
            sasl_password: None,
            ssl_ca_location: None,
            ssl_certificate_location: None,
            ssl_key_location: None,
            ssl_skip_verify: false,
            allow_insecure_transport: false,
            enable_auto_commit: false,
            auto_commit_interval_ms: default_auto_commit_interval(),
            session_timeout_ms: default_session_timeout(),
            heartbeat_interval_ms: default_heartbeat_interval(),
            max_poll_interval_ms: default_max_poll_interval(),
            fetch_min_bytes: default_fetch_min_bytes(),
            fetch_max_bytes: default_fetch_max_bytes(),
            max_partition_fetch_bytes: default_max_partition_fetch_bytes(),
            auto_offset_reset: default_auto_offset_reset(),
            enable_partition_eof: false,
            sizing: KafkaSizingConfig::default(),
            librdkafka_overrides: HashMap::new(),
            extra_config: HashMap::new(),
            filters_in: Vec::new(),
            filters_out: Vec::new(),
        }
    }
}

impl KafkaConfig {
    /// Create a config with the production profile.
    #[must_use]
    pub fn production() -> Self {
        Self {
            profile: KafkaProfile::Production,
            ..Default::default()
        }
    }

    /// Create a config with the devtest profile.
    ///
    /// Automatically enables SSL skip verify.
    #[must_use]
    pub fn devtest() -> Self {
        Self {
            profile: KafkaProfile::DevTest,
            ssl_skip_verify: true,
            ..Default::default()
        }
    }

    /// Create a minimal config for testing.
    #[must_use]
    pub fn for_testing(brokers: &str, group: &str, topics: Vec<String>) -> Self {
        Self {
            profile: KafkaProfile::DevTest,
            brokers: vec![brokers.to_string()],
            group: group.to_string(),
            topics,
            ssl_skip_verify: true,
            ..Default::default()
        }
    }

    /// Set the configuration profile.
    #[must_use]
    pub fn with_profile(mut self, profile: KafkaProfile) -> Self {
        self.profile = profile;
        if profile == KafkaProfile::DevTest {
            self.ssl_skip_verify = true;
        }
        self
    }

    /// Get the profile's librdkafka defaults.
    #[must_use]
    pub fn profile_defaults(&self) -> &'static [(&'static str, &'static str)] {
        match self.profile {
            KafkaProfile::Production => PRODUCTION_PROFILE,
            KafkaProfile::DevTest => DEVTEST_PROFILE,
        }
    }

    /// Build the final librdkafka config map.
    ///
    /// Order of precedence (lowest to highest):
    /// 1. Profile defaults
    /// 2. Explicit config fields (fetch_min_bytes, etc.)
    /// 3. `extra_config` (legacy, deprecated)
    /// 4. `librdkafka_overrides` (highest priority)
    #[must_use]
    #[allow(deprecated)]
    pub fn build_librdkafka_config(&self) -> HashMap<String, String> {
        let mut config = HashMap::new();

        // 1. Apply profile defaults
        for (key, value) in self.profile_defaults() {
            config.insert((*key).to_string(), (*value).to_string());
        }

        // 2. Explicit config fields override profile
        // (These are only applied if they differ from the "default" to avoid
        // overriding profile settings unintentionally)
        // Note: In practice, the transport layer applies these directly

        // 3. Legacy extra_config
        for (key, value) in &self.extra_config {
            config.insert(key.clone(), value.clone());
        }

        // 4. librdkafka_overrides (highest priority)
        for (key, value) in &self.librdkafka_overrides {
            config.insert(key.clone(), value.clone());
        }

        config
    }

    /// Add a librdkafka override.
    ///
    /// This has the highest priority and will override profile defaults.
    #[must_use]
    pub fn with_override(mut self, key: &str, value: &str) -> Self {
        self.librdkafka_overrides
            .insert(key.to_string(), value.to_string());
        self
    }

    /// Add multiple librdkafka overrides.
    #[must_use]
    pub fn with_overrides(mut self, overrides: &[(&str, &str)]) -> Self {
        for (key, value) in overrides {
            self.librdkafka_overrides
                .insert((*key).to_string(), (*value).to_string());
        }
        self
    }

    // ========================================================================
    // Authentication Methods
    // ========================================================================

    /// Create a config with SASL/SCRAM authentication.
    #[must_use]
    pub fn with_scram(mut self, mechanism: &str, username: &str, password: &str) -> Self {
        self.security_protocol = "sasl_plaintext".to_string();
        self.sasl_mechanism = Some(mechanism.to_string());
        self.sasl_username = Some(username.to_string());
        self.sasl_password = Some(crate::SensitiveString::new(password));
        self
    }

    /// Create a config with SASL/SSL authentication.
    #[must_use]
    pub fn with_scram_ssl(mut self, mechanism: &str, username: &str, password: &str) -> Self {
        self.security_protocol = "sasl_ssl".to_string();
        self.sasl_mechanism = Some(mechanism.to_string());
        self.sasl_username = Some(username.to_string());
        self.sasl_password = Some(crate::SensitiveString::new(password));
        self
    }

    /// Add TLS configuration.
    #[must_use]
    pub fn with_tls(mut self, ca_location: Option<&str>) -> Self {
        if self.security_protocol == "plaintext" {
            self.security_protocol = "ssl".to_string();
        } else if self.security_protocol == "sasl_plaintext" {
            self.security_protocol = "sasl_ssl".to_string();
        }
        self.ssl_ca_location = ca_location.map(String::from);
        self
    }

    /// Add client certificate for mutual TLS.
    #[must_use]
    pub fn with_client_cert(mut self, cert_location: &str, key_location: &str) -> Self {
        self.ssl_certificate_location = Some(cert_location.to_string());
        self.ssl_key_location = Some(key_location.to_string());
        self
    }

    /// Skip SSL certificate verification.
    ///
    /// **WARNING**: Only use in development/test environments! Rejected in
    /// production by [`validate`](Self::validate).
    #[must_use]
    pub fn with_ssl_skip_verify(mut self) -> Self {
        self.ssl_skip_verify = true;
        self
    }

    /// Validate the Kafka config against the deployment profile.
    ///
    /// `ssl_skip_verify` disables TLS certificate verification (MITM-exposed),
    /// and is set by the `devtest`/`for_testing` profiles by design. It is
    /// permitted only in dev/test; under a production profile this returns an
    /// error. Call at startup with [`crate::env::is_production`].
    ///
    /// NOTE: `ssl_skip_verify` is slated for removal at GA -- supply the broker
    /// CA via `ssl_ca_location` (private-CA trust) instead.
    ///
    /// # Errors
    ///
    /// Returns `Err` when `is_production` and either `ssl_skip_verify` is set,
    /// or an unencrypted transport (`plaintext`/`sasl_plaintext`) is configured
    /// without the explicit `allow_insecure_transport` opt-in.
    pub fn validate(&self, is_production: bool) -> Result<(), String> {
        if !is_production {
            return Ok(());
        }
        if self.ssl_skip_verify {
            return Err(
                "kafka: ssl_skip_verify (TLS verification disabled) is not permitted \
                 in production -- configure ssl_ca_location for private-CA trust instead"
                    .to_string(),
            );
        }
        // An unencrypted transport ships data (and SASL/PLAIN credentials) in
        // the clear. Reject in prod unless deliberately opted into.
        let proto = self.security_protocol.to_ascii_lowercase();
        if !self.allow_insecure_transport && (proto == "plaintext" || proto == "sasl_plaintext") {
            return Err(format!(
                "kafka: security_protocol='{}' sends data/credentials unencrypted and is not \
                 permitted in production -- use 'ssl'/'sasl_ssl', or set \
                 allow_insecure_transport=true to deliberately opt in (e.g. mesh-encrypted \
                 in-cluster traffic)",
                self.security_protocol
            ));
        }
        Ok(())
    }

    /// Enable SSL but accept any certificate (for dev/test with self-signed certs).
    #[must_use]
    pub fn with_ssl_insecure(mut self) -> Self {
        if self.security_protocol == "plaintext" {
            self.security_protocol = "ssl".to_string();
        } else if self.security_protocol == "sasl_plaintext" {
            self.security_protocol = "sasl_ssl".to_string();
        }
        self.ssl_skip_verify = true;
        self
    }

    // ========================================================================
    // Convenience Methods (apply common patterns as overrides)
    // ========================================================================

    /// Apply producer defaults.
    #[must_use]
    #[deprecated(since = "2.0.0", note = "Use producer profile constants directly")]
    #[allow(deprecated)]
    pub fn with_producer_defaults(mut self) -> Self {
        for (key, value) in PRODUCER_HIGH_THROUGHPUT {
            self.extra_config
                .entry((*key).to_string())
                .or_insert_with(|| (*value).to_string());
        }
        self
    }

    /// Apply high-throughput consumer defaults.
    #[must_use]
    #[deprecated(since = "2.0.0", note = "Use KafkaConfig::production() instead")]
    #[allow(deprecated)]
    pub fn with_high_throughput(mut self) -> Self {
        for (key, value) in PRODUCTION_PROFILE {
            self.extra_config
                .entry((*key).to_string())
                .or_insert_with(|| (*value).to_string());
        }
        self
    }

    /// Apply low-latency consumer defaults.
    #[must_use]
    #[deprecated(since = "2.0.0", note = "Use LOW_LATENCY_CONSUMER_DEFAULTS directly")]
    #[allow(deprecated)]
    pub fn with_low_latency(mut self) -> Self {
        for (key, value) in LOW_LATENCY_CONSUMER_DEFAULTS {
            self.extra_config
                .entry((*key).to_string())
                .or_insert_with(|| (*value).to_string());
        }
        self
    }

    /// Enable statistics collection at the specified interval.
    #[must_use]
    pub fn with_statistics(mut self, interval_ms: u32) -> Self {
        self.librdkafka_overrides.insert(
            "statistics.interval.ms".to_string(),
            interval_ms.to_string(),
        );
        self
    }

    /// Apply cloud-optimized connection settings.
    #[must_use]
    pub fn with_cloud_connection_tuning(mut self) -> Self {
        let cloud_settings = [
            ("socket.keepalive.enable", "true"),
            ("metadata.max.age.ms", "180000"),
            ("socket.connection.setup.timeout.ms", "30000"),
            ("connections.max.idle.ms", "540000"),
        ];
        for (key, value) in cloud_settings {
            self.librdkafka_overrides
                .entry(key.to_string())
                .or_insert_with(|| value.to_string());
        }
        self
    }

    // ========================================================================
    // Environment Loading
    // ========================================================================

    /// Load configuration from environment variables with prefix.
    ///
    /// Reads environment variables with the given prefix:
    /// - `{PREFIX}_PROFILE` -> profile (production, devtest)
    /// - `{PREFIX}_BOOTSTRAP_SERVERS` -> brokers (legacy: `{PREFIX}_BROKERS`)
    /// - `{PREFIX}_GROUP_ID` -> group
    /// - `{PREFIX}_SECURITY_PROTOCOL` -> security_protocol
    /// - `{PREFIX}_SASL_MECHANISM` -> sasl_mechanism
    /// - `{PREFIX}_SASL_USERNAME` -> sasl_username (legacy: `{PREFIX}_SASL_USER`)
    /// - `{PREFIX}_SASL_PASSWORD` -> sasl_password
    /// - `{PREFIX}_SSL_SKIP_VERIFY` -> ssl_skip_verify
    /// - `{PREFIX}_TOPICS` -> topics (comma-separated)
    ///
    /// Also supports standard `KAFKA_*` environment variables as fallback
    /// when using a custom prefix.
    #[cfg(feature = "config")]
    #[must_use]
    pub fn from_env(prefix: &str) -> Self {
        use crate::config::env_compat::EnvVar;

        let mut config = Self::default();

        // Helper to create prefixed env var with legacy support
        let prefixed = |name: &str, legacy: &[&str]| {
            let mut var = EnvVar::new(&format!("{prefix}_{name}"));
            for l in legacy {
                var = var.with_legacy(&format!("{prefix}_{l}"));
            }
            // Also check KAFKA_* as fallback
            var = var.with_legacy(&format!("KAFKA_{name}"));
            var
        };

        // Profile
        if let Some(val) = prefixed("PROFILE", &[]).get()
            && let Ok(profile) = val.parse()
        {
            config.profile = profile;
            if config.profile == KafkaProfile::DevTest {
                config.ssl_skip_verify = true;
            }
        }

        // Bootstrap servers (with BROKERS as legacy)
        if let Some(brokers) = prefixed("BOOTSTRAP_SERVERS", &["BROKERS"]).get_list() {
            config.brokers = brokers;
        }

        // Group ID
        if let Some(val) = prefixed("GROUP_ID", &["GROUP", "CONSUMER_GROUP"]).get() {
            config.group = val;
        }

        // Client ID
        if let Some(val) = prefixed("CLIENT_ID", &[]).get() {
            config.client_id = val;
        }

        // Security protocol
        if let Some(val) = prefixed("SECURITY_PROTOCOL", &[]).get() {
            config.security_protocol = val;
        }

        // SASL mechanism
        if let Some(val) = prefixed("SASL_MECHANISM", &[]).get() {
            config.sasl_mechanism = Some(val);
        }

        // SASL username (with SASL_USER as legacy)
        if let Some(val) = prefixed("SASL_USERNAME", &["SASL_USER"]).get() {
            config.sasl_username = Some(val);
        }

        // SASL password
        if let Some(val) = prefixed("SASL_PASSWORD", &[]).get() {
            config.sasl_password = Some(crate::SensitiveString::from(val));
        }

        // SSL CA location
        if let Some(val) = prefixed("SSL_CA_LOCATION", &["CA_CERT", "SSL_CA"]).get() {
            config.ssl_ca_location = Some(val);
        }

        // SSL skip verify
        if let Some(val) = prefixed("SSL_SKIP_VERIFY", &["SSL_INSECURE", "INSECURE"]).get_bool() {
            config.ssl_skip_verify = val;
        }

        // Topics
        if let Some(topics) = prefixed("TOPICS", &["TOPIC"]).get_list() {
            config.topics = topics;
        }

        config
    }

    /// Load configuration from standard `KAFKA_*` environment variables.
    ///
    /// This is a convenience method that uses the standard Kafka prefix.
    /// Supports legacy aliases with deprecation warnings.
    ///
    /// Standard variables:
    /// - `KAFKA_BOOTSTRAP_SERVERS` (legacy: `KAFKA_BROKERS`)
    /// - `KAFKA_SASL_USERNAME` (legacy: `KAFKA_SASL_USER`)
    /// - `KAFKA_SECURITY_PROTOCOL`
    /// - `KAFKA_SASL_MECHANISM`
    /// - `KAFKA_SASL_PASSWORD`
    /// - `KAFKA_SSL_SKIP_VERIFY`
    /// - `KAFKA_TOPICS`
    /// - `KAFKA_GROUP_ID`
    /// - `KAFKA_CLIENT_ID`
    /// - `KAFKA_PROFILE`
    #[cfg(feature = "config")]
    #[must_use]
    pub fn from_env_standard() -> Self {
        Self::from_env("KAFKA")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_ssl_skip_verify_in_production() {
        // devtest sets ssl_skip_verify -- fine in dev, rejected in prod.
        let dev = KafkaConfig::devtest();
        assert!(dev.ssl_skip_verify);
        assert!(dev.validate(false).is_ok(), "dev allows skip_verify");
        assert!(
            dev.validate(true).is_err(),
            "production must reject ssl_skip_verify"
        );

        // A TLS-verifying config validates in production.
        let prod = KafkaConfig {
            security_protocol: "ssl".to_string(),
            ..Default::default()
        };
        assert!(!prod.ssl_skip_verify);
        assert!(prod.validate(true).is_ok());
    }

    #[test]
    fn validate_rejects_unencrypted_transport_in_production() {
        // Default is plaintext -> rejected in prod, allowed in dev.
        let cfg = KafkaConfig::default();
        assert_eq!(cfg.security_protocol, "plaintext");
        assert!(cfg.validate(false).is_ok(), "dev allows plaintext");
        assert!(
            cfg.validate(true).is_err(),
            "production must reject plaintext transport"
        );

        // sasl_plaintext is likewise rejected in prod.
        let sasl = KafkaConfig {
            security_protocol: "sasl_plaintext".to_string(),
            ..Default::default()
        };
        assert!(sasl.validate(true).is_err());

        // The explicit, auditable override permits it (e.g. mesh-encrypted).
        let opted_in = KafkaConfig {
            security_protocol: "plaintext".to_string(),
            allow_insecure_transport: true,
            ..Default::default()
        };
        assert!(
            opted_in.validate(true).is_ok(),
            "allow_insecure_transport opts into plaintext in prod"
        );
    }

    #[test]
    fn kafka_config_topic_resolution_defaults() {
        let config = KafkaConfig::default();
        assert!(config.topic_include.is_empty());
        assert_eq!(config.topic_exclude, vec!["^__".to_string()]);
        assert!(!config.auto_discover);
        assert_eq!(config.topic_refresh_secs, 60);
        assert_eq!(config.topic_suppression_rules.len(), 1);
        assert_eq!(config.topic_suppression_rules[0].preferred_suffix, "_load");
        assert_eq!(config.topic_suppression_rules[0].suppressed_suffix, "_land");
    }

    // =========================================================================
    // Task 0.5: SelfRegulationProfile + KafkaSizingConfig tests
    // =========================================================================

    /// Helper: build a KafkaSizingConfig with the given profile and no
    /// per-knob overrides or raw maps. Tests the pure profile -> resolved map
    /// path.
    fn sizing_for_profile(profile: SelfRegulationProfile) -> KafkaSizingConfig {
        KafkaSizingConfig {
            profile,
            ..Default::default()
        }
    }

    // --- Consumer knob defaults by profile ---

    #[test]
    fn throughput_profile_consumer_knobs() {
        let s = sizing_for_profile(SelfRegulationProfile::Throughput);
        let map = s.resolved_consumer_map();
        assert_eq!(map["fetch.min.bytes"], "1048576", "1 MiB fetch.min.bytes");
        assert_eq!(map["fetch.wait.max.ms"], "50");
        assert_eq!(
            map["max.partition.fetch.bytes"], "10485760",
            "10 MiB per-partition"
        );
        assert_eq!(map["fetch.max.bytes"], "104857600", "100 MiB total");
        assert_eq!(
            s.effective_poll_cap(),
            2000,
            "throughput poll-safety cap = 2000"
        );
    }

    #[test]
    fn low_latency_profile_consumer_knobs() {
        let s = sizing_for_profile(SelfRegulationProfile::LowLatency);
        let map = s.resolved_consumer_map();
        assert_eq!(map["fetch.min.bytes"], "1", "no batching threshold");
        assert_eq!(map["fetch.wait.max.ms"], "5", "return fast");
        assert_eq!(map["max.partition.fetch.bytes"], "1048576", "1 MiB");
        assert_eq!(map["fetch.max.bytes"], "10485760", "10 MiB total");
        assert_eq!(s.effective_poll_cap(), 500);
    }

    #[test]
    fn balanced_profile_consumer_knobs() {
        let s = sizing_for_profile(SelfRegulationProfile::Balanced);
        let map = s.resolved_consumer_map();
        // Balanced sits between throughput and low_latency.
        let fmb: i32 = map["fetch.min.bytes"].parse().unwrap();
        let ll_min: i32 = 1;
        let tp_min: i32 = 1_048_576;
        assert!(
            fmb > ll_min && fmb < tp_min,
            "balanced fetch.min.bytes={fmb} should be between low_latency({ll_min}) and throughput({tp_min})"
        );
        assert_eq!(s.effective_poll_cap(), 1000);
    }

    /// Throughput and low_latency must differ on every key consumer knob.
    #[test]
    fn throughput_vs_low_latency_consumer_differ() {
        let tp = sizing_for_profile(SelfRegulationProfile::Throughput).resolved_consumer_map();
        let ll = sizing_for_profile(SelfRegulationProfile::LowLatency).resolved_consumer_map();
        for key in &["fetch.min.bytes", "fetch.wait.max.ms", "fetch.max.bytes"] {
            assert_ne!(
                tp[*key], ll[*key],
                "throughput and low_latency must differ on {key}"
            );
        }
    }

    // --- Producer knob defaults by profile ---

    #[test]
    fn throughput_profile_producer_knobs() {
        let s = sizing_for_profile(SelfRegulationProfile::Throughput);
        let map = s.resolved_producer_map();
        assert_eq!(map["batch.size"], "131072", "128 KiB batch");
        assert_eq!(map["linger.ms"], "20");
        assert_eq!(map["compression.type"], "lz4");
        // 64 MiB -> 65536 KiB
        assert_eq!(map["queue.buffering.max.kbytes"], "65536");
        assert_eq!(map["max.in.flight.requests.per.connection"], "5");
    }

    #[test]
    fn low_latency_profile_producer_knobs() {
        let s = sizing_for_profile(SelfRegulationProfile::LowLatency);
        let map = s.resolved_producer_map();
        assert_eq!(map["linger.ms"], "0", "send immediately");
        assert_eq!(map["compression.type"], "lz4");
        let batch: i32 = map["batch.size"].parse().unwrap();
        assert!(batch < 131_072, "low_latency batch should be < throughput");
    }

    /// Throughput and low_latency must differ on every key producer knob.
    #[test]
    fn throughput_vs_low_latency_producer_differ() {
        let tp = sizing_for_profile(SelfRegulationProfile::Throughput).resolved_producer_map();
        let ll = sizing_for_profile(SelfRegulationProfile::LowLatency).resolved_producer_map();
        for key in &["batch.size", "linger.ms", "queue.buffering.max.kbytes"] {
            assert_ne!(
                tp[*key], ll[*key],
                "throughput and low_latency must differ on {key}"
            );
        }
    }

    // --- Named knob overrides beat the profile ---

    #[test]
    fn explicit_consumer_knob_beats_profile() {
        let s = KafkaSizingConfig {
            profile: SelfRegulationProfile::Throughput,
            consumer: ConsumerKnobs {
                fetch_min_bytes: Some(2_097_152), // 2 MiB, not the 1 MiB profile default
                ..Default::default()
            },
            ..Default::default()
        };
        let map = s.resolved_consumer_map();
        assert_eq!(
            map["fetch.min.bytes"], "2097152",
            "explicit override must win over profile default"
        );
        // Other knobs still come from the throughput profile.
        assert_eq!(map["fetch.wait.max.ms"], "50");
    }

    #[test]
    fn explicit_producer_knob_beats_profile() {
        let s = KafkaSizingConfig {
            profile: SelfRegulationProfile::Throughput,
            producer: ProducerKnobs {
                linger_ms: Some(99), // override the 20 ms throughput default
                compression_type: Some("zstd".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let map = s.resolved_producer_map();
        assert_eq!(map["linger.ms"], "99");
        assert_eq!(map["compression.type"], "zstd");
        // sticky linger tracks the overridden linger_ms.
        assert_eq!(map["sticky.partitioning.linger.ms"], "99");
        // batch.size still comes from the throughput profile.
        assert_eq!(map["batch.size"], "131072");
    }

    // --- Raw escape hatch wins over named knob ---

    #[test]
    fn raw_consumer_librdkafka_wins_over_named_knob() {
        let mut consumer_raw = BTreeMap::new();
        consumer_raw.insert("fetch.min.bytes".to_string(), "9999".to_string());

        let s = KafkaSizingConfig {
            profile: SelfRegulationProfile::Throughput,
            consumer: ConsumerKnobs {
                fetch_min_bytes: Some(2_097_152), // named knob
                ..Default::default()
            },
            consumer_librdkafka: consumer_raw,
            ..Default::default()
        };
        let map = s.resolved_consumer_map();
        // Raw map must win over both profile AND named knob.
        assert_eq!(
            map["fetch.min.bytes"], "9999",
            "raw consumer_librdkafka must win over named knob"
        );
    }

    #[test]
    fn raw_producer_librdkafka_wins_over_named_knob() {
        let mut producer_raw = BTreeMap::new();
        producer_raw.insert("linger.ms".to_string(), "777".to_string());
        producer_raw.insert("compression.type".to_string(), "gzip".to_string());

        let s = KafkaSizingConfig {
            profile: SelfRegulationProfile::Throughput,
            producer: ProducerKnobs {
                linger_ms: Some(20),                       // named knob
                compression_type: Some("lz4".to_string()), // named knob
                ..Default::default()
            },
            producer_librdkafka: producer_raw,
            ..Default::default()
        };
        let map = s.resolved_producer_map();
        assert_eq!(
            map["linger.ms"], "777",
            "raw producer_librdkafka linger must win"
        );
        assert_eq!(
            map["compression.type"], "gzip",
            "raw producer_librdkafka compression must win"
        );
    }

    // --- KIP-794 / sticky partitioner ---

    #[test]
    fn producer_map_sets_sticky_partitioning_linger() {
        let s = sizing_for_profile(SelfRegulationProfile::Throughput);
        let map = s.resolved_producer_map();
        // sticky.partitioning.linger.ms should be set (not absent).
        assert!(
            map.contains_key("sticky.partitioning.linger.ms"),
            "sticky.partitioning.linger.ms must be present"
        );
        // Its value must match the resolved linger.ms.
        assert_eq!(
            map["sticky.partitioning.linger.ms"], map["linger.ms"],
            "sticky linger must track linger.ms"
        );
        // partitioner must NOT be set by default (we don't override the caller).
        assert!(
            !map.contains_key("partitioner"),
            "producer map must NOT set partitioner to preserve caller's choice"
        );
    }

    // --- compression.type present and consistent ---

    #[test]
    fn compression_type_present_in_all_profiles() {
        for profile in [
            SelfRegulationProfile::Throughput,
            SelfRegulationProfile::Balanced,
            SelfRegulationProfile::LowLatency,
        ] {
            let map = sizing_for_profile(profile).resolved_producer_map();
            assert!(
                map.contains_key("compression.type"),
                "profile {profile:?} must set compression.type"
            );
            assert!(
                !map["compression.type"].is_empty(),
                "compression.type must not be empty"
            );
        }
    }

    // --- poll cap (max.poll.records is client-side only) ---

    #[test]
    fn poll_cap_override_beats_profile() {
        let s = KafkaSizingConfig {
            profile: SelfRegulationProfile::Throughput,
            consumer: ConsumerKnobs {
                max_poll_records: Some(500), // override throughput's 2000
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(s.effective_poll_cap(), 500);
    }

    #[test]
    fn poll_cap_absent_falls_back_to_profile() {
        // Throughput profile => 2000
        let s = sizing_for_profile(SelfRegulationProfile::Throughput);
        assert_eq!(s.effective_poll_cap(), 2000);
    }

    // --- buffer_memory_bytes KiB conversion ---

    #[test]
    fn buffer_memory_converts_to_kib() {
        let s = KafkaSizingConfig {
            profile: SelfRegulationProfile::Throughput,
            producer: ProducerKnobs {
                buffer_memory_bytes: Some(1_048_576), // exactly 1 MiB
                ..Default::default()
            },
            ..Default::default()
        };
        let map = s.resolved_producer_map();
        assert_eq!(
            map["queue.buffering.max.kbytes"], "1024",
            "1 MiB = 1024 KiB"
        );
    }

    // --- KafkaConfig.sizing field is default-initialised ---

    #[test]
    fn kafka_config_default_has_sizing_field() {
        let cfg = KafkaConfig::default();
        // Default profile is Throughput.
        assert_eq!(cfg.sizing.profile, SelfRegulationProfile::Throughput);
        // Raw maps are empty.
        assert!(cfg.sizing.consumer_librdkafka.is_empty());
        assert!(cfg.sizing.producer_librdkafka.is_empty());
    }
}
