// Project:   hyperi-rustlib
// File:      src/tiered_sink/config.rs
// Purpose:   TieredSink configuration
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! TieredSink configuration.

use crate::tiered_sink::CompressionCodec;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Configuration for TieredSink.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredSinkConfig {
    /// Path to the spool file for disk fallback.
    pub spool_path: PathBuf,

    /// Timeout for primary sink operations.
    /// If exceeded, message is spooled to disk.
    #[serde(default = "default_send_timeout_ms")]
    pub send_timeout_ms: u64,

    /// Compression codec for spooled messages.
    #[serde(default)]
    pub compression: CompressionCodec,

    /// Strategy for draining spooled messages back to primary.
    #[serde(default)]
    pub drain_strategy: DrainStrategy,

    /// Ordering mode for message delivery.
    #[serde(default)]
    pub ordering: OrderingMode,

    /// Maximum spool file size in bytes.
    /// Spool operations fail when exceeded.
    #[serde(default)]
    pub max_spool_bytes: Option<u64>,

    /// Maximum messages in spool.
    #[serde(default)]
    pub max_spool_items: Option<usize>,

    /// Circuit breaker: failures before opening circuit.
    #[serde(default = "default_circuit_failure_threshold")]
    pub circuit_failure_threshold: u32,

    /// Circuit breaker: how long to wait before probing.
    #[serde(default = "default_circuit_reset_timeout_ms")]
    pub circuit_reset_timeout_ms: u64,

    /// Interval for drain task to check spool.
    #[serde(default = "default_drain_interval_ms")]
    pub drain_interval_ms: u64,
}

fn default_send_timeout_ms() -> u64 {
    1000 // 1 second
}

fn default_circuit_failure_threshold() -> u32 {
    5
}

fn default_circuit_reset_timeout_ms() -> u64 {
    30_000 // 30 seconds
}

fn default_drain_interval_ms() -> u64 {
    100 // 100ms
}

impl TieredSinkConfig {
    /// Create a new config with the given spool path.
    #[must_use]
    pub fn new(spool_path: impl Into<PathBuf>) -> Self {
        Self {
            spool_path: spool_path.into(),
            send_timeout_ms: default_send_timeout_ms(),
            compression: CompressionCodec::default(),
            drain_strategy: DrainStrategy::default(),
            ordering: OrderingMode::default(),
            max_spool_bytes: None,
            max_spool_items: None,
            circuit_failure_threshold: default_circuit_failure_threshold(),
            circuit_reset_timeout_ms: default_circuit_reset_timeout_ms(),
            drain_interval_ms: default_drain_interval_ms(),
        }
    }

    /// Set send timeout.
    #[must_use]
    pub fn send_timeout(mut self, timeout: Duration) -> Self {
        self.send_timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        self
    }

    /// Set compression codec.
    #[must_use]
    pub fn compression(mut self, codec: CompressionCodec) -> Self {
        self.compression = codec;
        self
    }

    /// Set drain strategy.
    #[must_use]
    pub fn drain_strategy(mut self, strategy: DrainStrategy) -> Self {
        self.drain_strategy = strategy;
        self
    }

    /// Set ordering mode.
    #[must_use]
    pub fn ordering(mut self, mode: OrderingMode) -> Self {
        self.ordering = mode;
        self
    }

    /// Set maximum spool size.
    #[must_use]
    pub fn max_spool_bytes(mut self, max: u64) -> Self {
        self.max_spool_bytes = Some(max);
        self
    }

    /// Get send timeout as Duration.
    #[must_use]
    pub fn send_timeout_duration(&self) -> Duration {
        Duration::from_millis(self.send_timeout_ms)
    }

    /// Get circuit reset timeout as Duration.
    #[must_use]
    pub fn circuit_reset_timeout(&self) -> Duration {
        Duration::from_millis(self.circuit_reset_timeout_ms)
    }

    /// Get drain interval as Duration.
    #[must_use]
    pub fn drain_interval(&self) -> Duration {
        Duration::from_millis(self.drain_interval_ms)
    }
}

/// Strategy for draining spooled messages back to the primary sink.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DrainStrategy {
    /// Adaptive rate: starts slow, speeds up based on success rate.
    /// This is the default and recommended strategy.
    Adaptive {
        /// Initial drain rate (messages per second).
        #[serde(default = "default_initial_rate")]
        initial_rate: usize,
        /// Maximum drain rate (messages per second).
        #[serde(default = "default_max_rate")]
        max_rate: usize,
    },

    /// Fixed rate limit (messages per second).
    RateLimited {
        /// Messages per second.
        msgs_per_sec: usize,
    },

    /// Drain as fast as possible.
    /// Use with caution - may overwhelm a recovering sink.
    Greedy,
}

fn default_initial_rate() -> usize {
    100
}

fn default_max_rate() -> usize {
    10_000
}

impl Default for DrainStrategy {
    fn default() -> Self {
        Self::Adaptive {
            initial_rate: default_initial_rate(),
            max_rate: default_max_rate(),
        }
    }
}

impl DrainStrategy {
    /// Create adaptive strategy with custom rates.
    #[must_use]
    pub fn adaptive(initial_rate: usize, max_rate: usize) -> Self {
        Self::Adaptive {
            initial_rate,
            max_rate,
        }
    }

    /// Create rate-limited strategy.
    #[must_use]
    pub fn rate_limited(msgs_per_sec: usize) -> Self {
        Self::RateLimited { msgs_per_sec }
    }
}

/// Ordering mode for message delivery during drain.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderingMode {
    /// New messages go hot path, spool drains in background (default).
    /// Maximizes throughput with slight ordering relaxation.
    /// New messages may arrive before older spooled messages.
    #[default]
    Interleaved,

    /// Drain spool completely before new messages use hot path.
    /// Guarantees strict FIFO ordering but blocks new traffic during drain.
    StrictFifo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = TieredSinkConfig::new("/tmp/test.queue");
        assert_eq!(config.send_timeout_ms, 1000);
        assert_eq!(config.compression, CompressionCodec::Lz4);
        assert!(matches!(
            config.drain_strategy,
            DrainStrategy::Adaptive { .. }
        ));
        assert_eq!(config.ordering, OrderingMode::Interleaved);
        assert_eq!(config.circuit_failure_threshold, 5);
    }

    #[test]
    fn test_builder_pattern() {
        let config = TieredSinkConfig::new("/tmp/test.queue")
            .send_timeout(Duration::from_secs(5))
            .compression(CompressionCodec::Snappy)
            .drain_strategy(DrainStrategy::Greedy)
            .ordering(OrderingMode::StrictFifo)
            .max_spool_bytes(1024 * 1024 * 100);

        assert_eq!(config.send_timeout_ms, 5000);
        assert_eq!(config.compression, CompressionCodec::Snappy);
        assert!(matches!(config.drain_strategy, DrainStrategy::Greedy));
        assert_eq!(config.ordering, OrderingMode::StrictFifo);
        assert_eq!(config.max_spool_bytes, Some(100 * 1024 * 1024));
    }

    #[test]
    fn test_drain_strategy_constructors() {
        let adaptive = DrainStrategy::adaptive(50, 5000);
        assert!(matches!(
            adaptive,
            DrainStrategy::Adaptive {
                initial_rate: 50,
                max_rate: 5000
            }
        ));

        let rate_limited = DrainStrategy::rate_limited(1000);
        assert!(matches!(
            rate_limited,
            DrainStrategy::RateLimited { msgs_per_sec: 1000 }
        ));
    }

    #[test]
    fn test_duration_conversions() {
        let config = TieredSinkConfig::new("/tmp/test.queue");
        assert_eq!(config.send_timeout_duration(), Duration::from_secs(1));
        assert_eq!(config.circuit_reset_timeout(), Duration::from_secs(30));
        assert_eq!(config.drain_interval(), Duration::from_millis(100));
    }
}
