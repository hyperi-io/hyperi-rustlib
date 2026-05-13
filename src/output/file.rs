// Project:   hyperi-rustlib
// File:      src/output/file.rs
// Purpose:   File output sink using NdjsonWriter
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! File output sink for raw NDJSON events.
//!
//! Writes raw JSON bytes to rotating files using the shared [`NdjsonWriter`].
//! Used for testing and bare-metal deployments where Kafka is not available.
//!
//! ## Sync vs async API
//!
//! - [`FileOutput::write`] / [`FileOutput::write_batch`] are SYNC — they
//!   call into the parking_lot-protected `NdjsonWriter` directly. Cheap
//!   (~µs) but block the calling thread. Safe from sync code, tests, and
//!   pre-runtime startup.
//! - [`FileOutput::write_async`] / [`FileOutput::write_batch_async`] are
//!   ASYNC — they hand the sync work to `tokio::task::spawn_blocking` so
//!   the tokio runtime is never stalled. Use these from `async fn`
//!   bodies.
//!
//! Both APIs share the same underlying `Arc<NdjsonWriter>`, so counters
//! and rotation state are consistent regardless of which path is taken.
//!
//! ## File Layout
//!
//! ```text
//! /var/spool/dfe/output/loader/
//! ├── events.ndjson              # Current file
//! ├── events.ndjson.20260302T14  # Rotated (hourly)
//! └── events.ndjson.20260302T13.gz  # Compressed
//! ```

use std::sync::Arc;

use tracing::debug;

use crate::io::NdjsonWriter;

use super::config::FileOutputConfig;
use super::error::OutputError;

/// File output sink for raw NDJSON events.
///
/// Wraps [`NdjsonWriter`] with output-specific configuration and logging.
/// Cheap to `Clone` — the inner writer is shared via `Arc`.
#[derive(Clone)]
pub struct FileOutput {
    writer: Arc<NdjsonWriter>,
}

impl std::fmt::Debug for FileOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileOutput")
            .field("lines_written", &self.writer.lines_written())
            .field("write_errors", &self.writer.write_errors())
            .field("output_path", self.writer.output_path())
            .finish_non_exhaustive()
    }
}

impl FileOutput {
    /// Create a new file output sink.
    ///
    /// Creates the output directory if it doesn't exist.
    ///
    /// # Arguments
    ///
    /// * `config` — File output configuration
    /// * `service_name` — Used as subdirectory name (e.g. "loader", "receiver")
    ///
    /// # Errors
    ///
    /// Returns an error if the output directory cannot be created or the sink
    /// is disabled.
    pub fn new(config: &FileOutputConfig, service_name: &str) -> Result<Self, OutputError> {
        if !config.enabled {
            return Err(OutputError::Disabled);
        }

        let writer_config = config.to_writer_config();
        let writer = NdjsonWriter::new(&writer_config, service_name, &config.filename, "output")?;

        debug!(
            service = service_name,
            filename = %config.filename,
            path = %config.path.display(),
            "File output sink initialised"
        );

        Ok(Self {
            writer: Arc::new(writer),
        })
    }

    /// Write a single raw JSON bytes line (sync). Blocks the calling
    /// thread on disk I/O. Safe from sync code; **never call from an
    /// `async fn` body** — use [`Self::write_async`] instead.
    pub fn write(&self, data: &[u8]) -> Result<(), OutputError> {
        if data.last() == Some(&b'\n') {
            self.writer.write_line(data)?;
        } else {
            let mut line = Vec::with_capacity(data.len() + 1);
            line.extend_from_slice(data);
            line.push(b'\n');
            self.writer.write_line(&line)?;
        }
        Ok(())
    }

    /// Write a batch of raw JSON bytes lines (sync). Same caveats as
    /// [`Self::write`] — use [`Self::write_batch_async`] from `async fn`.
    pub fn write_batch(&self, data: &[&[u8]]) -> Result<(), OutputError> {
        if data.is_empty() {
            return Ok(());
        }

        let total_len: usize = data.iter().map(|d| d.len() + 1).sum();
        let mut buf = Vec::with_capacity(total_len);
        for entry in data {
            buf.extend_from_slice(entry);
            if entry.last() != Some(&b'\n') {
                buf.push(b'\n');
            }
        }

        let count = data.len() as u64;
        self.writer.write_buf(&buf, count)?;
        Ok(())
    }

    /// Async write — runs the rotate-and-write on a blocking thread via
    /// `tokio::task::spawn_blocking`. Hot-path safe for async callers.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`OutputError`] from the sync writer, or
    /// an `OutputError::Io` if the blocking thread panicked.
    pub async fn write_async(&self, data: Vec<u8>) -> Result<(), OutputError> {
        let writer = Arc::clone(&self.writer);
        tokio::task::spawn_blocking(move || -> Result<(), OutputError> {
            let line: &[u8] = if data.last() == Some(&b'\n') {
                &data
            } else {
                // Borrow check: build the owned buffer in the same scope.
                return {
                    let mut line = Vec::with_capacity(data.len() + 1);
                    line.extend_from_slice(&data);
                    line.push(b'\n');
                    writer.write_line(&line).map_err(OutputError::from)
                };
            };
            writer.write_line(line).map_err(OutputError::from)
        })
        .await
        .map_err(|e| OutputError::Io(std::io::Error::other(e)))?
    }

    /// Async batch write — coalesces lines into a single buffer and runs
    /// the rotate-and-write on a blocking thread.
    ///
    /// # Errors
    ///
    /// As [`Self::write_async`].
    pub async fn write_batch_async(&self, data: Vec<Vec<u8>>) -> Result<(), OutputError> {
        if data.is_empty() {
            return Ok(());
        }
        let writer = Arc::clone(&self.writer);
        tokio::task::spawn_blocking(move || -> Result<(), OutputError> {
            let total_len: usize = data.iter().map(|d| d.len() + 1).sum();
            let mut buf = Vec::with_capacity(total_len);
            for entry in &data {
                buf.extend_from_slice(entry);
                if entry.last() != Some(&b'\n') {
                    buf.push(b'\n');
                }
            }
            let count = data.len() as u64;
            writer.write_buf(&buf, count).map_err(OutputError::from)
        })
        .await
        .map_err(|e| OutputError::Io(std::io::Error::other(e)))?
    }

    /// Number of lines successfully written.
    #[must_use]
    pub fn lines_written(&self) -> u64 {
        self.writer.lines_written()
    }

    /// Number of write errors encountered.
    #[must_use]
    pub fn write_errors(&self) -> u64 {
        self.writer.write_errors()
    }

    /// Shared `Arc<NdjsonWriter>` for callers that need both sync and
    /// async access to the same underlying writer (e.g. building an
    /// [`crate::io::AsyncNdjsonWriter`] view).
    #[must_use]
    pub fn shared_writer(&self) -> Arc<NdjsonWriter> {
        Arc::clone(&self.writer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::RotationPeriod;

    fn test_config(dir: &std::path::Path) -> FileOutputConfig {
        FileOutputConfig {
            enabled: true,
            path: dir.to_path_buf(),
            filename: "events.ndjson".into(),
            rotation: RotationPeriod::Daily,
            max_age_days: 1,
            compress_rotated: false,
        }
    }

    #[test]
    fn test_disabled_returns_error() {
        let config = FileOutputConfig::default(); // enabled: false
        let result = FileOutput::new(&config, "test");
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), OutputError::Disabled),
            "expected Disabled error"
        );
    }

    #[test]
    fn test_write_single() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path());
        let output = FileOutput::new(&config, "test-svc").expect("create");

        output.write(b"{\"event\":\"login\"}").expect("write");
        assert_eq!(output.lines_written(), 1);

        let content =
            std::fs::read_to_string(dir.path().join("test-svc/events.ndjson")).expect("read");
        assert_eq!(content.trim(), r#"{"event":"login"}"#);
    }

    #[test]
    fn test_write_with_trailing_newline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path());
        let output = FileOutput::new(&config, "nl-svc").expect("create");

        output.write(b"{\"event\":\"test\"}\n").expect("write");
        assert_eq!(output.lines_written(), 1);

        let content =
            std::fs::read_to_string(dir.path().join("nl-svc/events.ndjson")).expect("read");
        assert_eq!(content.trim(), r#"{"event":"test"}"#);
    }

    #[test]
    fn test_write_batch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path());
        let output = FileOutput::new(&config, "batch-svc").expect("create");

        let events: Vec<&[u8]> = vec![b"{\"n\":0}", b"{\"n\":1}", b"{\"n\":2}"];
        output.write_batch(&events).expect("batch write");
        assert_eq!(output.lines_written(), 3);

        let content =
            std::fs::read_to_string(dir.path().join("batch-svc/events.ndjson")).expect("read");
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], r#"{"n":0}"#);
        assert_eq!(lines[2], r#"{"n":2}"#);
    }

    #[test]
    fn test_write_batch_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path());
        let output = FileOutput::new(&config, "empty-svc").expect("create");

        output.write_batch(&[]).expect("empty batch");
        assert_eq!(output.lines_written(), 0);
    }

    #[test]
    fn test_debug_format() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path());
        let output = FileOutput::new(&config, "dbg-svc").expect("create");

        let debug = format!("{output:?}");
        assert!(debug.contains("FileOutput"));
        assert!(debug.contains("lines_written"));
    }

    #[tokio::test]
    async fn write_async_writes_to_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = test_config(dir.path());
        let output = FileOutput::new(&cfg, "async-svc").expect("create");

        output
            .write_async(b"{\"k\":\"v\"}".to_vec())
            .await
            .expect("write_async");
        assert_eq!(output.lines_written(), 1);

        let body =
            std::fs::read_to_string(dir.path().join("async-svc/events.ndjson")).expect("read");
        assert_eq!(body.trim(), r#"{"k":"v"}"#);
    }

    #[tokio::test]
    async fn write_batch_async_writes_to_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = test_config(dir.path());
        let output = FileOutput::new(&cfg, "ab-svc").expect("create");

        let batch: Vec<Vec<u8>> = (0..3)
            .map(|i| format!("{{\"n\":{i}}}").into_bytes())
            .collect();
        output.write_batch_async(batch).await.expect("batch async");
        assert_eq!(output.lines_written(), 3);

        let body = std::fs::read_to_string(dir.path().join("ab-svc/events.ndjson")).expect("read");
        assert_eq!(body.trim().lines().count(), 3);
    }

    #[tokio::test]
    async fn write_batch_async_empty_is_noop() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = test_config(dir.path());
        let output = FileOutput::new(&cfg, "empty-async").expect("create");
        output.write_batch_async(vec![]).await.expect("empty async");
        assert_eq!(output.lines_written(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn write_async_does_not_block_runtime() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = test_config(dir.path());
        let output = FileOutput::new(&cfg, "nb-svc").expect("create");

        let ticks = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let tc = ticks.clone();
        let ticker = tokio::spawn(async move {
            let mut t = tokio::time::interval(std::time::Duration::from_millis(2));
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            t.tick().await;
            for _ in 0..15 {
                t.tick().await;
                tc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });

        let mut writers = Vec::new();
        for _ in 0..4 {
            let o = output.clone();
            writers.push(tokio::spawn(async move {
                for i in 0..50_u32 {
                    o.write_async(format!("{{\"n\":{i}}}").into_bytes())
                        .await
                        .expect("write");
                }
            }));
        }
        for h in writers {
            h.await.expect("writer task");
        }
        ticker.await.expect("ticker");

        assert_eq!(output.lines_written(), 200);
        let t = ticks.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            t >= 8,
            "ticker fired only {t} times — FileOutput starved the runtime",
        );
    }

    #[tokio::test]
    async fn clone_shares_writer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = test_config(dir.path());
        let a = FileOutput::new(&cfg, "share").expect("create");
        let b = a.clone();

        a.write_async(b"{\"a\":1}".to_vec()).await.expect("a");
        b.write_async(b"{\"b\":2}".to_vec()).await.expect("b");

        assert_eq!(a.lines_written(), 2);
        assert_eq!(b.lines_written(), 2);
        assert!(std::sync::Arc::ptr_eq(
            &a.shared_writer(),
            &b.shared_writer()
        ));
    }
}
