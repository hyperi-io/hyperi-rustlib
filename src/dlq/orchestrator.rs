// Project:   hyperi-rustlib
// File:      src/dlq/orchestrator.rs
// Purpose:   Dlq orchestrator over BackgroundSink<DlqEntry>
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Dlq orchestrator.
//!
//! Wraps a [`BackgroundSink<DlqEntry>`] whose drain (`DlqDrain`)
//! dispatches batches across one or more [`super::DlqBackend`]
//! variants using the configured [`DlqMode`].
//!
//! ## Hot path
//!
//! `try_send` / `send` queue an entry onto the in-memory mpsc and
//! return. The drain task -- the only place that touches backends --
//! coalesces queued entries into batches and writes to backends. The
//! caller never blocks on disk, Kafka, HTTP, or Redis I/O.
//!
//! ## Modes
//!
//! - `Cascade` / `FileOnly` / `KafkaOnly` -- try backends in order,
//!   stop on first success.
//! - `FanOut` -- send to all backends, succeed if any succeed.
//!
//! ## Shutdown
//!
//! On `CancellationToken::cancel()` the drain finishes its in-flight
//! batch, drains the queue, then exits. Use [`Dlq::shutdown`] for
//! graceful join. Dropping all `Dlq` handles also triggers a clean
//! exit (channel closes, drain drains, then exits).

use std::sync::Arc;

use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::concurrency::{
    BackgroundSink, BackgroundSinkConfig, BackgroundSinkHandle, DrainError, Overflow, SinkDrain,
    SinkError,
};

use super::backend::DlqBackend;
use super::config::{DlqConfig, DlqMode};
use super::entry::DlqEntry;
use super::error::DlqError;
use super::file::FileDlqInner;

/// Unified DLQ. Caller queues entries from any task; the orchestrator
/// drains them off-runtime via the configured backends.
///
/// Clone is cheap (`mpsc::Sender` clone). The single-owner shutdown
/// handle stays inside `Arc<AsyncMutex<Option<...>>>` so `Dlq` itself
/// is `Clone`.
#[derive(Clone)]
pub struct Dlq {
    sink: Option<BackgroundSink<DlqEntry>>,
    join: Arc<AsyncMutex<Option<BackgroundSinkHandle>>>,
    enabled: bool,
    mode: DlqMode,
    /// Child of the user-supplied shutdown token. The drain task runs
    /// on this child, so [`Dlq::shutdown`] can cancel only the DLQ
    /// without affecting the caller's broader shutdown plan. When the
    /// caller cancels their own token, the child fires too (normal
    /// child-token semantics), so the drain still exits on global
    /// shutdown.
    cancel: CancellationToken,
}

impl std::fmt::Debug for Dlq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dlq")
            .field("enabled", &self.enabled)
            .field("mode", &self.mode)
            .field(
                "pending",
                &self.sink.as_ref().map_or(0, BackgroundSink::pending),
            )
            .field(
                "dropped",
                &self.sink.as_ref().map_or(0, BackgroundSink::dropped),
            )
            .finish_non_exhaustive()
    }
}

impl Dlq {
    /// Build a disabled DLQ. All `send` / `try_send` calls succeed as
    /// no-ops.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            sink: None,
            join: Arc::new(AsyncMutex::new(None)),
            enabled: false,
            mode: DlqMode::default(),
            cancel: CancellationToken::new(),
        }
    }

    /// Spawn the DLQ with whatever backends the config enables.
    ///
    /// `kafka_config` is required if the config has `kafka.enabled =
    /// true` (or the mode demands Kafka). Pass `None` if the service
    /// has no Kafka transport -- Kafka mode/enabled flags are honoured
    /// where possible and a clear `Err(DlqError::NotConfigured)` is
    /// returned if Kafka is required but unavailable.
    ///
    /// # Errors
    ///
    /// Returns `Err` if any enabled backend fails to initialise.
    pub fn spawn(
        config: &DlqConfig,
        service_name: &str,
        #[cfg(feature = "dlq-kafka")] kafka_config: Option<&crate::transport::KafkaConfig>,
        #[cfg(not(feature = "dlq-kafka"))] _kafka_config: Option<&()>,
        shutdown: CancellationToken,
    ) -> Result<Self, DlqError> {
        if !config.enabled {
            return Ok(Self::disabled());
        }

        let backends = build_backends(
            config,
            service_name,
            #[cfg(feature = "dlq-kafka")]
            kafka_config,
        )?;

        if backends.is_empty() {
            warn!("DLQ enabled but no backends configured -- entries will be dropped");
            return Ok(Self::disabled());
        }

        let names: Vec<&'static str> = backends.iter().map(DlqBackend::name).collect();
        debug!(mode = ?config.mode, backends = ?names, "DLQ initialised");

        let drain = DlqDrain {
            mode: config.mode,
            backends,
        };

        let sink_config = BackgroundSinkConfig {
            queue_capacity: config.queue_capacity,
            batch_size: config.batch_size,
            flush_interval: std::time::Duration::from_millis(config.flush_interval_ms),
            overflow: Overflow::Drop,
            metric_prefix: Some("dfe_dlq"),
        };

        // Derive a child token so `Dlq::shutdown` can stop the drain
        // without forcing the caller to cancel their broader shutdown
        // plan. The child fires automatically when the parent fires,
        // so global shutdown still drains the DLQ.
        let cancel = shutdown.child_token();
        let (sink, handle) = BackgroundSink::spawn(drain, sink_config, cancel.clone());

        Ok(Self {
            sink: Some(sink),
            join: Arc::new(AsyncMutex::new(Some(handle))),
            enabled: true,
            mode: config.mode,
            cancel,
        })
    }

    /// Whether the DLQ is accepting entries.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Configured routing mode (informational).
    #[must_use]
    pub fn mode(&self) -> DlqMode {
        self.mode
    }

    /// Approximate queue depth (drain may be mid-recv).
    #[must_use]
    pub fn pending(&self) -> usize {
        self.sink.as_ref().map_or(0, BackgroundSink::pending)
    }

    /// Total entries dropped due to overflow since spawn.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.sink.as_ref().map_or(0, BackgroundSink::dropped)
    }

    /// Sync-shaped queue submission. Returns immediately. On a full
    /// queue, returns `Err(DlqError::QueueFull)` and increments the
    /// drop counter -- caller decides whether to log, escalate, or
    /// proceed.
    ///
    /// # Errors
    ///
    /// `QueueFull` if the in-memory queue is full. `Closed` if the
    /// drain has exited.
    pub fn try_send(&self, entry: DlqEntry) -> Result<(), DlqError> {
        let Some(sink) = self.sink.as_ref() else {
            return Ok(());
        };
        sink.try_push(entry).map_err(map_sink_err)
    }

    /// Async submission that awaits queue space.
    ///
    /// Successful return means the entry is queued, NOT that it is
    /// durably written. Use [`Self::flush`] for that.
    ///
    /// # Errors
    ///
    /// `Closed` if the drain has exited.
    pub async fn send(&self, entry: DlqEntry) -> Result<(), DlqError> {
        let Some(sink) = self.sink.as_ref() else {
            return Ok(());
        };
        sink.push_blocking(entry).await.map_err(map_sink_err)
    }

    /// Async batch submission. Each entry is queued individually; the
    /// drain decides how to coalesce.
    ///
    /// # Errors
    ///
    /// `Closed` if the drain has exited mid-batch.
    pub async fn send_batch(&self, entries: Vec<DlqEntry>) -> Result<(), DlqError> {
        let Some(sink) = self.sink.as_ref() else {
            return Ok(());
        };
        for entry in entries {
            sink.push_blocking(entry).await.map_err(map_sink_err)?;
        }
        Ok(())
    }

    /// Block until every entry queued before this call is durably
    /// written by the drain.
    ///
    /// # Errors
    ///
    /// `Closed` if the drain has exited before this barrier was
    /// processed.
    pub async fn flush(&self) -> Result<(), DlqError> {
        let Some(sink) = self.sink.as_ref() else {
            return Ok(());
        };
        sink.flush().await.map_err(map_sink_err)
    }

    /// Initiate shutdown and await graceful drain exit.
    ///
    /// Cancels the internal child token (drain observes the cancellation
    /// in its next `select!`, flushes its remaining batch, and exits),
    /// then awaits the drain task. This is the canonical "stop the DLQ
    /// and wait for it" call -- the previous version only awaited the
    /// join and would hang forever unless the caller had separately
    /// cancelled the token passed to `spawn`.
    ///
    /// Idempotent: safe to call from many clones; the join happens
    /// once. Subsequent calls observe an empty join slot and return Ok.
    ///
    /// # Errors
    ///
    /// Returns `Err(DlqError::Closed)` if the drain task panicked.
    pub async fn shutdown(&self) -> Result<(), DlqError> {
        // Trip the child token so the drain notices on its next
        // select!. Idempotent -- CancellationToken::cancel handles
        // re-cancellation.
        self.cancel.cancel();
        let mut guard = self.join.lock().await;
        let Some(handle) = guard.take() else {
            return Ok(());
        };
        handle
            .join()
            .await
            .map_err(|e| DlqError::File(format!("DLQ drain join failed: {e}")))?;
        Ok(())
    }
}

fn map_sink_err(e: SinkError) -> DlqError {
    match e {
        SinkError::Overflow => DlqError::QueueFull,
        SinkError::Closed => DlqError::Closed,
        SinkError::Drain(d) => DlqError::File(d.to_string()),
    }
}

fn build_backends(
    config: &DlqConfig,
    service_name: &str,
    #[cfg(feature = "dlq-kafka")] kafka_config: Option<&crate::transport::KafkaConfig>,
) -> Result<Vec<DlqBackend>, DlqError> {
    let mut backends: Vec<DlqBackend> = Vec::new();
    let mode = config.mode;

    // Kafka first (primary in cascade) -- feature-gated.
    #[cfg(feature = "dlq-kafka")]
    {
        let want_kafka = matches!(
            mode,
            DlqMode::Cascade | DlqMode::FanOut | DlqMode::KafkaOnly
        );
        if want_kafka && config.kafka.enabled {
            let kc = kafka_config.ok_or_else(|| {
                DlqError::Kafka(
                    "DLQ Kafka backend enabled but no KafkaConfig provided to Dlq::spawn".into(),
                )
            })?;
            backends.push(DlqBackend::Kafka(super::kafka::KafkaDlqInner::new(
                kc,
                &config.kafka,
            )?));
        }
    }

    // File second (fallback in cascade) -- always available.
    let want_file = matches!(mode, DlqMode::Cascade | DlqMode::FanOut | DlqMode::FileOnly);
    if want_file && config.file.enabled {
        backends.push(DlqBackend::File(FileDlqInner::new(
            &config.file,
            service_name,
        )?));
    }

    // HTTP -- feature-gated, added when explicitly enabled.
    #[cfg(feature = "dlq-http")]
    {
        if config.http.enabled {
            backends.push(DlqBackend::Http(super::http::HttpDlqInner::new(
                &config.http,
            )?));
        }
    }

    // Redis -- feature-gated. Requires async constructor; we build a
    // tokio runtime handle inline. Spawn() must run inside a tokio
    // runtime (true for every HyperI service).
    #[cfg(feature = "dlq-redis")]
    {
        if config.redis.enabled {
            let cfg = config.redis.clone();
            let inner = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(super::redis_dlq::RedisDlqInner::new(&cfg))
            })?;
            backends.push(DlqBackend::Redis(inner));
        }
    }

    Ok(backends)
}

/// Drain task -- owns the backends and implements cascade / fan-out
/// dispatch. Lives inside the actor task spawned by `BackgroundSink`.
struct DlqDrain {
    mode: DlqMode,
    backends: Vec<DlqBackend>,
}

impl SinkDrain<DlqEntry> for DlqDrain {
    async fn write_batch(&mut self, batch: Vec<DlqEntry>) -> Result<(), DrainError> {
        if batch.is_empty() {
            return Ok(());
        }

        match self.mode {
            DlqMode::Cascade | DlqMode::FileOnly | DlqMode::KafkaOnly => {
                let mut last_err: Option<DlqError> = None;
                for backend in &mut self.backends {
                    match backend.send_batch(&batch).await {
                        Ok(()) => return Ok(()),
                        Err(e) => {
                            warn!(
                                backend = backend.name(),
                                error = %e,
                                count = batch.len(),
                                "DLQ backend failed in cascade, trying next"
                            );
                            last_err = Some(e);
                        }
                    }
                }
                let msg = last_err
                    .map_or_else(|| "no backends configured".to_string(), |e| e.to_string());
                Err(DrainError::Backend(Box::new(DlqError::AllBackendsFailed(
                    msg,
                ))))
            }
            DlqMode::FanOut => {
                let mut any_ok = false;
                let mut errs: Vec<String> = Vec::new();
                for backend in &mut self.backends {
                    match backend.send_batch(&batch).await {
                        Ok(()) => any_ok = true,
                        Err(e) => {
                            warn!(
                                backend = backend.name(),
                                error = %e,
                                count = batch.len(),
                                "DLQ backend failed in fan-out"
                            );
                            errs.push(format!("{}:{}", backend.name(), e));
                        }
                    }
                }
                if any_ok {
                    Ok(())
                } else {
                    Err(DrainError::Backend(Box::new(DlqError::AllBackendsFailed(
                        errs.join("; "),
                    ))))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dlq::config::{FileDlqConfig, RotationPeriod};
    use crate::dlq::entry::DlqSource;

    fn tmp_config(dir: &std::path::Path) -> DlqConfig {
        DlqConfig {
            file: FileDlqConfig {
                enabled: true,
                path: dir.to_path_buf(),
                rotation: RotationPeriod::Daily,
                max_age_days: 1,
                compress_rotated: false,
            },
            mode: DlqMode::FileOnly,
            queue_capacity: 1024,
            batch_size: 16,
            flush_interval_ms: 20,
            ..DlqConfig::default()
        }
    }

    fn test_entry(reason: &str) -> DlqEntry {
        DlqEntry::new("test", reason, b"payload".to_vec())
            .with_destination("acme.auth")
            .with_source(DlqSource::kafka("events", 1, 42))
    }

    #[tokio::test]
    async fn disabled_dlq_accepts_silently() {
        let dlq = Dlq::disabled();
        dlq.send(test_entry("err")).await.expect("noop");
        dlq.send_batch(vec![test_entry("err")]).await.expect("noop");
        dlq.flush().await.expect("noop flush");
        dlq.shutdown().await.expect("noop shutdown");
    }

    #[tokio::test]
    async fn file_only_writes_and_flushes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shutdown = CancellationToken::new();
        let dlq = Dlq::spawn(
            &tmp_config(dir.path()),
            "svc",
            #[cfg(feature = "dlq-kafka")]
            None,
            #[cfg(not(feature = "dlq-kafka"))]
            None,
            shutdown.clone(),
        )
        .expect("spawn");

        for i in 0..5 {
            dlq.send(test_entry(&format!("err_{i}")))
                .await
                .expect("send");
        }
        dlq.flush().await.expect("flush");

        let path = dir.path().join("svc/dlq.ndjson");
        let body = std::fs::read_to_string(&path).expect("read");
        let lines: Vec<&str> = body.trim().lines().collect();
        assert_eq!(lines.len(), 5);

        shutdown.cancel();
        dlq.shutdown().await.expect("clean shutdown");
    }

    #[tokio::test]
    async fn try_send_returns_queue_full_when_saturated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cfg = tmp_config(dir.path());
        cfg.queue_capacity = 2;
        cfg.batch_size = 1024;
        cfg.flush_interval_ms = 60_000; // drain rarely fires
        let shutdown = CancellationToken::new();
        let dlq = Dlq::spawn(
            &cfg,
            "svc",
            #[cfg(feature = "dlq-kafka")]
            None,
            #[cfg(not(feature = "dlq-kafka"))]
            None,
            shutdown.clone(),
        )
        .expect("spawn");

        let mut full_count = 0;
        for i in 0..50 {
            if let Err(DlqError::QueueFull) = dlq.try_send(test_entry(&format!("err_{i}"))) {
                full_count += 1;
            }
        }
        assert!(full_count > 0, "expected at least one QueueFull");
        shutdown.cancel();
    }

    #[tokio::test]
    async fn dlq_clone_shares_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shutdown = CancellationToken::new();
        let dlq = Dlq::spawn(
            &tmp_config(dir.path()),
            "svc",
            #[cfg(feature = "dlq-kafka")]
            None,
            #[cfg(not(feature = "dlq-kafka"))]
            None,
            shutdown.clone(),
        )
        .expect("spawn");

        let dlq2 = dlq.clone();
        dlq.send(test_entry("a")).await.expect("send a");
        dlq2.send(test_entry("b")).await.expect("send b");
        dlq.flush().await.expect("flush");

        let path = dir.path().join("svc/dlq.ndjson");
        let body = std::fs::read_to_string(&path).expect("read");
        assert_eq!(body.trim().lines().count(), 2);

        shutdown.cancel();
        dlq.shutdown().await.expect("shutdown");
    }
}
