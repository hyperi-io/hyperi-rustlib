// Project:   hyperi-rustlib
// File:      src/dlq/dlq.rs
// Purpose:   DLQ orchestrator with cascade/fan-out backend routing
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! DLQ orchestrator.
//!
//! Routes entries to one or more [`DlqBackend`] implementations based
//! on the configured [`DlqMode`].
//!
//! ## Modes
//!
//! - **Cascade**: Try backends in order; stop on first success.
//!   Default order: Kafka first, file fallback.
//! - **Fan-out**: Write to all enabled backends; return Ok if at least one succeeds.
//! - **FileOnly**: File backend only (no Kafka dependency).
//! - **KafkaOnly**: Kafka backend only.

use tracing::{debug, error, warn};

use super::backend::DlqBackend;
use super::config::{DlqConfig, DlqMode};
use super::entry::DlqEntry;
use super::error::DlqError;
use super::file::FileDlq;

/// Unified DLQ with pluggable backends.
///
/// Services create a `Dlq` at startup and use it throughout their pipeline
/// to route failed messages. The orchestrator handles backend selection
/// based on the configured mode.
///
/// # Example
///
/// ```rust,no_run
/// use hyperi_rustlib::dlq::{Dlq, DlqConfig, DlqEntry};
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let config = DlqConfig::default();
/// let dlq = Dlq::file_only(&config, "my-service")?;
///
/// let entry = DlqEntry::new("my-service", "parse_error", b"bad data".to_vec());
/// dlq.send(entry).await?;
/// # Ok(())
/// # }
/// ```
pub struct Dlq {
    backends: Vec<Box<dyn DlqBackend>>,
    mode: DlqMode,
    enabled: bool,
}

impl std::fmt::Debug for Dlq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dlq")
            .field("mode", &self.mode)
            .field("enabled", &self.enabled)
            .field(
                "backends",
                &self.backends.iter().map(|b| b.name()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Dlq {
    /// Create a DLQ with file backend only.
    ///
    /// # Errors
    ///
    /// Returns an error if the file backend cannot be initialised.
    pub fn file_only(config: &DlqConfig, service_name: &str) -> Result<Self, DlqError> {
        let mut backends: Vec<Box<dyn DlqBackend>> = Vec::new();

        if config.file.enabled {
            let file = FileDlq::new(&config.file, service_name)?;
            backends.push(Box::new(file));
        }

        if backends.is_empty() && config.enabled {
            warn!("DLQ enabled but no backends configured");
        }

        debug!(
            mode = ?config.mode,
            backends = backends.len(),
            "DLQ initialised (file only)"
        );

        Ok(Self {
            backends,
            mode: DlqMode::FileOnly,
            enabled: config.enabled,
        })
    }

    /// Create a DLQ with Kafka and file backends.
    ///
    /// Backend order: Kafka first, file second (for cascade mode).
    ///
    /// # Errors
    ///
    /// Returns an error if any enabled backend cannot be initialised.
    #[cfg(feature = "dlq-kafka")]
    pub fn with_kafka(
        config: &DlqConfig,
        service_name: &str,
        kafka_config: &crate::transport::KafkaConfig,
    ) -> Result<Self, DlqError> {
        let mut backends: Vec<Box<dyn DlqBackend>> = Vec::new();
        let mode = config.mode;

        // Add Kafka backend first (primary in cascade mode)
        let want_kafka = matches!(
            mode,
            DlqMode::Cascade | DlqMode::FanOut | DlqMode::KafkaOnly
        );
        if want_kafka && config.kafka.enabled {
            let kafka = super::kafka::KafkaDlq::new(kafka_config, &config.kafka)?;
            backends.push(Box::new(kafka));
        }

        // Add file backend second (fallback in cascade mode)
        let want_file = matches!(mode, DlqMode::Cascade | DlqMode::FanOut | DlqMode::FileOnly);
        if want_file && config.file.enabled {
            let file = FileDlq::new(&config.file, service_name)?;
            backends.push(Box::new(file));
        }

        if backends.is_empty() && config.enabled {
            warn!("DLQ enabled but no backends configured");
        }

        debug!(
            mode = ?mode,
            backends = ?backends.iter().map(|b| b.name()).collect::<Vec<_>>(),
            "DLQ initialised"
        );

        Ok(Self {
            backends,
            mode,
            enabled: config.enabled,
        })
    }

    /// Add a custom backend.
    ///
    /// Use this to register future backends (S3, ClickHouse, webhook, etc.)
    /// without modifying the core DLQ module.
    pub fn add_backend(&mut self, backend: Box<dyn DlqBackend>) {
        debug!(backend = backend.name(), "Custom DLQ backend added");
        self.backends.push(backend);
    }

    /// Whether the DLQ is enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Send an entry to the DLQ using the configured mode.
    ///
    /// # Errors
    ///
    /// Returns an error if all backends fail (cascade) or if no backends
    /// are configured.
    pub async fn send(&self, entry: DlqEntry) -> Result<(), DlqError> {
        if !self.enabled {
            return Ok(());
        }

        if self.backends.is_empty() {
            return Err(DlqError::NotConfigured);
        }

        match self.mode {
            DlqMode::Cascade | DlqMode::FileOnly | DlqMode::KafkaOnly => {
                self.send_cascade(&entry).await
            }
            DlqMode::FanOut => self.send_fanout(&entry).await,
        }
    }

    /// Send a batch of entries to the DLQ.
    ///
    /// # Errors
    ///
    /// Returns an error if all backends fail.
    pub async fn send_batch(&self, entries: &[DlqEntry]) -> Result<(), DlqError> {
        if !self.enabled || entries.is_empty() {
            return Ok(());
        }

        if self.backends.is_empty() {
            return Err(DlqError::NotConfigured);
        }

        match self.mode {
            DlqMode::Cascade | DlqMode::FileOnly | DlqMode::KafkaOnly => {
                self.send_batch_cascade(entries).await
            }
            DlqMode::FanOut => self.send_batch_fanout(entries).await,
        }
    }

    /// Cascade: try each backend in order, stop on first success.
    async fn send_cascade(&self, entry: &DlqEntry) -> Result<(), DlqError> {
        let mut last_error = None;

        for backend in &self.backends {
            match backend.send(entry).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    warn!(
                        backend = backend.name(),
                        error = %e,
                        "DLQ backend failed, trying next"
                    );
                    last_error = Some(e);
                }
            }
        }

        let err = last_error.map_or_else(
            || DlqError::NotConfigured,
            |e| DlqError::AllBackendsFailed(e.to_string()),
        );
        error!(error = %err, "All DLQ backends failed");
        Err(err)
    }

    /// Fan-out: send to all backends, return Ok if at least one succeeds.
    async fn send_fanout(&self, entry: &DlqEntry) -> Result<(), DlqError> {
        let mut any_success = false;
        let mut errors = Vec::new();

        for backend in &self.backends {
            match backend.send(entry).await {
                Ok(()) => any_success = true,
                Err(e) => {
                    warn!(
                        backend = backend.name(),
                        error = %e,
                        "DLQ fan-out backend failed"
                    );
                    errors.push(format!("{}:{}", backend.name(), e));
                }
            }
        }

        if any_success {
            Ok(())
        } else {
            let msg = errors.join("; ");
            error!(error = %msg, "All DLQ backends failed in fan-out mode");
            Err(DlqError::AllBackendsFailed(msg))
        }
    }

    /// Cascade batch: try each backend in order.
    async fn send_batch_cascade(&self, entries: &[DlqEntry]) -> Result<(), DlqError> {
        let mut last_error = None;

        for backend in &self.backends {
            match backend.send_batch(entries).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    warn!(
                        backend = backend.name(),
                        error = %e,
                        count = entries.len(),
                        "DLQ batch backend failed, trying next"
                    );
                    last_error = Some(e);
                }
            }
        }

        let err = last_error.map_or_else(
            || DlqError::NotConfigured,
            |e| DlqError::AllBackendsFailed(e.to_string()),
        );
        Err(err)
    }

    /// Fan-out batch: send to all backends.
    async fn send_batch_fanout(&self, entries: &[DlqEntry]) -> Result<(), DlqError> {
        let mut any_success = false;
        let mut errors = Vec::new();

        for backend in &self.backends {
            match backend.send_batch(entries).await {
                Ok(()) => any_success = true,
                Err(e) => {
                    warn!(
                        backend = backend.name(),
                        error = %e,
                        count = entries.len(),
                        "DLQ fan-out batch backend failed"
                    );
                    errors.push(format!("{}:{}", backend.name(), e));
                }
            }
        }

        if any_success {
            Ok(())
        } else {
            Err(DlqError::AllBackendsFailed(errors.join("; ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Mock backend for testing orchestrator logic.
    struct MockBackend {
        name: &'static str,
        should_fail: bool,
        send_count: Arc<AtomicU64>,
    }

    impl MockBackend {
        fn new(name: &'static str, should_fail: bool) -> Self {
            Self {
                name,
                should_fail,
                send_count: Arc::new(AtomicU64::new(0)),
            }
        }
    }

    #[async_trait::async_trait]
    impl DlqBackend for MockBackend {
        async fn send(&self, _entry: &DlqEntry) -> Result<(), DlqError> {
            if self.should_fail {
                Err(DlqError::File(format!("{} mock failure", self.name)))
            } else {
                self.send_count.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
        }

        fn name(&self) -> &'static str {
            self.name
        }
    }

    fn test_entry() -> DlqEntry {
        DlqEntry::new("test", "test_reason", b"test_payload".to_vec())
    }

    #[tokio::test]
    async fn test_disabled_dlq_is_noop() {
        let dlq = Dlq {
            backends: vec![],
            mode: DlqMode::Cascade,
            enabled: false,
        };
        // Should succeed (noop) even with no backends
        dlq.send(test_entry()).await.expect("disabled is noop");
    }

    #[tokio::test]
    async fn test_no_backends_returns_error() {
        let dlq = Dlq {
            backends: vec![],
            mode: DlqMode::Cascade,
            enabled: true,
        };
        let err = dlq.send(test_entry()).await.unwrap_err();
        assert!(matches!(err, DlqError::NotConfigured));
    }

    #[tokio::test]
    async fn test_cascade_first_success_stops() {
        let b1 = MockBackend::new("first", false);
        let b1_count = Arc::clone(&b1.send_count);
        let b2 = MockBackend::new("second", false);
        let b2_count = Arc::clone(&b2.send_count);

        let dlq = Dlq {
            backends: vec![Box::new(b1), Box::new(b2)],
            mode: DlqMode::Cascade,
            enabled: true,
        };

        dlq.send(test_entry()).await.expect("cascade");
        assert_eq!(b1_count.load(Ordering::Relaxed), 1);
        assert_eq!(b2_count.load(Ordering::Relaxed), 0); // Not called
    }

    #[tokio::test]
    async fn test_cascade_fallback_on_failure() {
        let b1 = MockBackend::new("kafka", true); // Fails
        let b2 = MockBackend::new("file", false); // Succeeds
        let b2_count = Arc::clone(&b2.send_count);

        let dlq = Dlq {
            backends: vec![Box::new(b1), Box::new(b2)],
            mode: DlqMode::Cascade,
            enabled: true,
        };

        dlq.send(test_entry()).await.expect("fallback");
        assert_eq!(b2_count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_cascade_all_fail() {
        let b1 = MockBackend::new("kafka", true);
        let b2 = MockBackend::new("file", true);

        let dlq = Dlq {
            backends: vec![Box::new(b1), Box::new(b2)],
            mode: DlqMode::Cascade,
            enabled: true,
        };

        let err = dlq.send(test_entry()).await.unwrap_err();
        assert!(matches!(err, DlqError::AllBackendsFailed(_)));
    }

    #[tokio::test]
    async fn test_fanout_writes_to_all() {
        let b1 = MockBackend::new("kafka", false);
        let b1_count = Arc::clone(&b1.send_count);
        let b2 = MockBackend::new("file", false);
        let b2_count = Arc::clone(&b2.send_count);

        let dlq = Dlq {
            backends: vec![Box::new(b1), Box::new(b2)],
            mode: DlqMode::FanOut,
            enabled: true,
        };

        dlq.send(test_entry()).await.expect("fanout");
        assert_eq!(b1_count.load(Ordering::Relaxed), 1);
        assert_eq!(b2_count.load(Ordering::Relaxed), 1); // Both called
    }

    #[tokio::test]
    async fn test_fanout_partial_failure_ok() {
        let b1 = MockBackend::new("kafka", true); // Fails
        let b2 = MockBackend::new("file", false); // Succeeds
        let b2_count = Arc::clone(&b2.send_count);

        let dlq = Dlq {
            backends: vec![Box::new(b1), Box::new(b2)],
            mode: DlqMode::FanOut,
            enabled: true,
        };

        // Should succeed because at least one backend worked
        dlq.send(test_entry()).await.expect("partial fanout");
        assert_eq!(b2_count.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_fanout_all_fail() {
        let b1 = MockBackend::new("kafka", true);
        let b2 = MockBackend::new("file", true);

        let dlq = Dlq {
            backends: vec![Box::new(b1), Box::new(b2)],
            mode: DlqMode::FanOut,
            enabled: true,
        };

        let err = dlq.send(test_entry()).await.unwrap_err();
        assert!(matches!(err, DlqError::AllBackendsFailed(_)));
    }

    #[tokio::test]
    async fn test_batch_empty_is_noop() {
        let dlq = Dlq {
            backends: vec![Box::new(MockBackend::new("file", false))],
            mode: DlqMode::Cascade,
            enabled: true,
        };
        dlq.send_batch(&[]).await.expect("empty batch");
    }

    #[tokio::test]
    async fn test_add_custom_backend() {
        let mut dlq = Dlq {
            backends: vec![],
            mode: DlqMode::Cascade,
            enabled: true,
        };

        let custom = MockBackend::new("custom", false);
        let count = Arc::clone(&custom.send_count);
        dlq.add_backend(Box::new(custom));

        dlq.send(test_entry()).await.expect("custom backend");
        assert_eq!(count.load(Ordering::Relaxed), 1);
    }
}
