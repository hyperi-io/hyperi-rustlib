// Project:   hyperi-rustlib
// File:      src/transport/routed.rs
// Purpose:   Per-key routing transport for data originators (receiver, fetcher)
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Per-key routing transport for data originators.
//!
//! Routes `send(key, payload)` to different transport backends based on the
//! key. Used by dfe-receiver and dfe-fetcher where data-based routing
//! determines the destination (topic, endpoint, stream).
//!
//! All other DFE stages (transforms, loader, archiver) use simple 1:1
//! transports and do NOT need this.
//!
//! # Config
//!
//! ```yaml
//! transport:
//!   output:
//!     type: routed
//!     default: kafka
//!     routes:
//!       events.land:
//!         type: grpc
//!         grpc:
//!           endpoint: "http://loader-land:6000"
//!       events.load:
//!         type: kafka
//!       audit.land:
//!         type: grpc
//!         grpc:
//!           endpoint: "http://archiver:6000"
//!     kafka:
//!       brokers: ["kafka:9092"]
//! ```
//!
//! # Usage
//!
//! ```rust,ignore
//! let sender = RoutedSender::from_config("transport.output").await?;
//! // Routes to different backends based on key
//! sender.send("events.land", payload).await;  // → gRPC to loader-land
//! sender.send("events.load", payload).await;  // → Kafka topic
//! sender.send("audit.land", payload).await;   // → gRPC to archiver
//! sender.send("unknown", payload).await;      // → default (Kafka)
//! ```

use std::collections::HashMap;

use super::error::{TransportError, TransportResult};
use super::factory::AnySender;
use super::traits::{TransportBase, TransportSender};
use super::types::SendResult;

/// A routing transport that dispatches `send()` to different backends
/// based on the key parameter.
///
/// Used by dfe-receiver and dfe-fetcher (data originators) where
/// data-based routing determines the destination.
pub struct RoutedSender {
    /// Per-key route overrides.
    routes: HashMap<String, AnySender>,
    /// Default sender for keys not in the routes map.
    default: Option<AnySender>,
    closed: std::sync::atomic::AtomicBool,
}

impl RoutedSender {
    /// Create a new routed sender with explicit routes and optional default.
    pub fn new(routes: HashMap<String, AnySender>, default: Option<AnySender>) -> Self {
        Self {
            routes,
            default,
            closed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Create from a map of key → `TransportConfig` plus a default config.
    ///
    /// Each route gets its own `AnySender` created from the corresponding config.
    pub async fn from_route_configs(
        routes: HashMap<String, super::TransportConfig>,
        default_config: Option<super::TransportConfig>,
    ) -> TransportResult<Self> {
        let mut senders = HashMap::with_capacity(routes.len());
        for (key, config) in routes {
            let sender = AnySender::from_transport_config(&config).await?;
            senders.insert(key, sender);
        }

        let default = match default_config {
            Some(cfg) => Some(AnySender::from_transport_config(&cfg).await?),
            None => None,
        };

        Ok(Self::new(senders, default))
    }

    /// Get the list of configured route keys.
    #[must_use]
    pub fn route_keys(&self) -> Vec<&str> {
        self.routes.keys().map(String::as_str).collect()
    }

    /// Check if a specific route key is configured.
    #[must_use]
    pub fn has_route(&self, key: &str) -> bool {
        self.routes.contains_key(key)
    }

    /// Check if a default fallback sender is configured.
    #[must_use]
    pub fn has_default(&self) -> bool {
        self.default.is_some()
    }

    /// Resolve which sender handles a given key.
    fn resolve(&self, key: &str) -> Option<&AnySender> {
        self.routes.get(key).or(self.default.as_ref())
    }
}

impl TransportBase for RoutedSender {
    async fn close(&self) -> TransportResult<()> {
        self.closed
            .store(true, std::sync::atomic::Ordering::Relaxed);
        // Close all route senders
        for sender in self.routes.values() {
            sender.close().await?;
        }
        if let Some(ref default) = self.default {
            default.close().await?;
        }
        Ok(())
    }

    fn is_healthy(&self) -> bool {
        if self.closed.load(std::sync::atomic::Ordering::Relaxed) {
            return false;
        }
        // Healthy if all configured senders are healthy
        let routes_healthy = self.routes.values().all(|s| s.is_healthy());
        let default_healthy = self.default.as_ref().is_none_or(|s| s.is_healthy());
        routes_healthy && default_healthy
    }

    fn name(&self) -> &'static str {
        "routed"
    }
}

impl TransportSender for RoutedSender {
    async fn send(&self, key: &str, payload: &[u8]) -> SendResult {
        if self.closed.load(std::sync::atomic::Ordering::Relaxed) {
            return SendResult::Fatal(TransportError::Closed);
        }

        let Some(sender) = self.resolve(key) else {
            return SendResult::Fatal(TransportError::Config(format!(
                "no route configured for key '{key}' and no default sender"
            )));
        };

        #[cfg(feature = "metrics")]
        metrics::counter!(
            "dfe_transport_sent_total",
            "transport" => "routed",
            "route" => key.to_string()
        )
        .increment(1);

        sender.send(key, payload).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "transport-memory")]
    use crate::transport::memory::{MemoryConfig, MemoryTransport};

    #[cfg(feature = "transport-memory")]
    fn make_memory_sender() -> AnySender {
        AnySender::Memory(MemoryTransport::new(&MemoryConfig::default()))
    }

    #[tokio::test]
    #[cfg(feature = "transport-memory")]
    async fn routes_to_correct_sender() {
        let mut route_map = HashMap::new();
        route_map.insert("events.land".into(), make_memory_sender());
        route_map.insert("events.load".into(), make_memory_sender());

        let sender = RoutedSender::new(route_map, Some(make_memory_sender()));

        let result_land = sender.send("events.land", b"land-payload").await;
        assert!(result_land.is_ok());

        let result_load = sender.send("events.load", b"load-payload").await;
        assert!(result_load.is_ok());

        // Unknown key falls through to default
        let result_default = sender.send("unknown.key", b"default-payload").await;
        assert!(result_default.is_ok());

        assert!(sender.is_healthy());
        assert_eq!(sender.name(), "routed");
    }

    #[tokio::test]
    async fn no_route_no_default_returns_fatal() {
        let sender = RoutedSender::new(HashMap::new(), None);

        let result = sender.send("unknown", b"payload").await;
        assert!(result.is_fatal());
    }

    #[tokio::test]
    #[cfg(feature = "transport-memory")]
    async fn close_propagates_to_all_senders() {
        let mut route_map = HashMap::new();
        route_map.insert("a".into(), make_memory_sender());
        let sender = RoutedSender::new(route_map, Some(make_memory_sender()));

        assert!(sender.is_healthy());
        sender.close().await.unwrap();
        assert!(!sender.is_healthy());
    }

    #[test]
    fn route_keys_and_has_route() {
        let sender = RoutedSender::new(HashMap::new(), None);
        assert!(sender.route_keys().is_empty());
        assert!(!sender.has_route("anything"));
        assert!(!sender.has_default());
    }

    #[tokio::test]
    async fn send_after_close_returns_fatal() {
        let sender = RoutedSender::new(HashMap::new(), None);
        sender.close().await.unwrap();

        let result = sender.send("key", b"payload").await;
        assert!(result.is_fatal());
    }
}
