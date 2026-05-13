// Project:   hyperi-rustlib
// File:      src/io/ndjson_writer.rs
// Purpose:   Core NDJSON file writer with rotation and metrics
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Core NDJSON file writer with automatic rotation.
//!
//! Writes `&[u8]` lines to a rotating file using the `file-rotate` crate.
//! This writer knows nothing about DLQ or output semantics — callers
//! serialise their own types and hand raw bytes to the writer.
//!
//! ## Two APIs
//!
//! - [`NdjsonWriter`] — synchronous. Acquires a `parking_lot::Mutex` and
//!   calls `std::io::Write::write_all` directly. Cheap (~µs) but blocks
//!   the calling thread. Safe to call from non-async code and tests.
//! - [`AsyncNdjsonWriter`] — async wrapper over `Arc<NdjsonWriter>`. Each
//!   call runs the sync work on a `tokio::task::spawn_blocking` thread,
//!   so the tokio runtime is never stalled. Use this from `async fn`
//!   bodies.
//!
//! ## Thread Safety
//!
//! Both wrappers are `Send + Sync`. `NdjsonWriter` uses
//! `parking_lot::Mutex<FileRotate>` internally so multiple callers can
//! share one writer instance.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use file_rotate::suffix::AppendTimestamp;
use file_rotate::suffix::FileLimit;
use file_rotate::{ContentLimit, FileRotate, compression::Compression};
use parking_lot::Mutex;
use tracing::debug;

use super::config::{FileWriterConfig, RotationPeriod};

/// NDJSON file writer with automatic rotation and metrics.
///
/// Each line written is expected to be a complete JSON object (NDJSON format).
/// The writer handles file rotation, optional compression, and age-based cleanup.
pub struct NdjsonWriter {
    writer: Mutex<FileRotate<AppendTimestamp>>,
    label: String,
    output_path: PathBuf,
    lines_written: AtomicU64,
    write_errors: AtomicU64,
}

impl std::fmt::Debug for NdjsonWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NdjsonWriter")
            .field("label", &self.label)
            .field("output_path", &self.output_path)
            .field("lines_written", &self.lines_written.load(Ordering::Relaxed))
            .field("write_errors", &self.write_errors.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl NdjsonWriter {
    /// Create a new NDJSON writer.
    ///
    /// Creates `{config.path}/{subdir}/` and writes to `{filename}` within it.
    /// Files are rotated according to the config's rotation period.
    ///
    /// # Arguments
    ///
    /// * `config` — Shared file writer settings (path, rotation, compression)
    /// * `subdir` — Subdirectory under `config.path` (e.g. service name)
    /// * `filename` — Output filename (e.g. "dlq.ndjson", "events.ndjson")
    /// * `label` — Human label for log messages (e.g. "dlq", "output")
    ///
    /// # Errors
    ///
    /// Returns `std::io::Error` if the output directory cannot be created.
    pub fn new(
        config: &FileWriterConfig,
        subdir: &str,
        filename: &str,
        label: &str,
    ) -> Result<Self, std::io::Error> {
        let dir = config.path.join(subdir);
        std::fs::create_dir_all(&dir)?;

        let file_path = dir.join(filename);

        let content_limit = match config.rotation {
            RotationPeriod::Hourly => ContentLimit::Time(file_rotate::TimeFrequency::Hourly),
            RotationPeriod::Daily => ContentLimit::Time(file_rotate::TimeFrequency::Daily),
        };

        let max_age = chrono::Duration::days(i64::from(config.max_age_days));
        let suffix_scheme = AppendTimestamp::default(FileLimit::Age(max_age));

        let compression = if config.compress_rotated {
            Compression::OnRotate(6)
        } else {
            Compression::None
        };

        let writer = FileRotate::new(file_path, suffix_scheme, content_limit, compression, None);

        debug!(
            label = label,
            path = %dir.display(),
            rotation = ?config.rotation,
            "{} writer initialised",
            label,
        );

        Ok(Self {
            writer: Mutex::new(writer),
            label: label.to_string(),
            output_path: dir,
            lines_written: AtomicU64::new(0),
            write_errors: AtomicU64::new(0),
        })
    }

    /// Write a single line (must include trailing newline or caller appends it).
    ///
    /// The data is written as-is — caller is responsible for serialisation
    /// and newline termination.
    pub fn write_line(&self, line: &[u8]) -> Result<(), std::io::Error> {
        let mut writer = self.writer.lock();
        if let Err(e) = writer.write_all(line).and_then(|()| writer.flush()) {
            self.write_errors.fetch_add(1, Ordering::Relaxed);
            return Err(e);
        }
        self.lines_written.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Write a pre-serialised buffer containing multiple newline-delimited lines.
    ///
    /// The buffer should already have newlines between entries. The `count`
    /// parameter is used for metrics tracking.
    pub fn write_buf(&self, buf: &[u8], count: u64) -> Result<(), std::io::Error> {
        let mut writer = self.writer.lock();
        if let Err(e) = writer.write_all(buf).and_then(|()| writer.flush()) {
            self.write_errors.fetch_add(1, Ordering::Relaxed);
            return Err(e);
        }
        self.lines_written.fetch_add(count, Ordering::Relaxed);
        Ok(())
    }

    /// Number of lines successfully written.
    pub fn lines_written(&self) -> u64 {
        self.lines_written.load(Ordering::Relaxed)
    }

    /// Number of write errors encountered.
    pub fn write_errors(&self) -> u64 {
        self.write_errors.load(Ordering::Relaxed)
    }

    /// Human label for this writer (e.g. "dlq", "output").
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Output directory path.
    pub fn output_path(&self) -> &PathBuf {
        &self.output_path
    }
}

/// Async wrapper around [`NdjsonWriter`] that runs the sync rotate-and-write
/// on `tokio::task::spawn_blocking` to keep the tokio runtime unblocked.
///
/// Use this from `async fn` bodies. For sync code paths, call
/// [`NdjsonWriter`] directly.
///
/// Holds an `Arc<NdjsonWriter>` so multiple async tasks can share one
/// writer without cloning the underlying `parking_lot::Mutex<FileRotate>`.
#[derive(Debug, Clone)]
pub struct AsyncNdjsonWriter {
    inner: Arc<NdjsonWriter>,
}

impl AsyncNdjsonWriter {
    /// Wrap an `NdjsonWriter`. Use this when no other task needs the
    /// underlying writer.
    #[must_use]
    pub fn new(writer: NdjsonWriter) -> Self {
        Self {
            inner: Arc::new(writer),
        }
    }

    /// Wrap a shared `Arc<NdjsonWriter>`. Use this when sync code paths
    /// also need access to the same writer.
    #[must_use]
    pub fn from_arc(writer: Arc<NdjsonWriter>) -> Self {
        Self { inner: writer }
    }

    /// Write a single line off-runtime. The closure runs on a blocking
    /// thread; the tokio runtime is free to schedule other tasks.
    ///
    /// # Errors
    ///
    /// Returns the underlying `std::io::Error` from the sync writer, or
    /// an `io::Error::other(JoinError)` if the blocking thread panicked.
    pub async fn write_line(&self, line: Vec<u8>) -> Result<(), std::io::Error> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.write_line(&line))
            .await
            .map_err(std::io::Error::other)?
    }

    /// Write a pre-coalesced buffer of `count` lines off-runtime.
    ///
    /// # Errors
    ///
    /// As [`Self::write_line`].
    pub async fn write_buf(&self, buf: Vec<u8>, count: u64) -> Result<(), std::io::Error> {
        let inner = Arc::clone(&self.inner);
        tokio::task::spawn_blocking(move || inner.write_buf(&buf, count))
            .await
            .map_err(std::io::Error::other)?
    }

    /// Number of lines successfully written.
    #[must_use]
    pub fn lines_written(&self) -> u64 {
        self.inner.lines_written()
    }

    /// Number of write errors.
    #[must_use]
    pub fn write_errors(&self) -> u64 {
        self.inner.write_errors()
    }

    /// Human label.
    #[must_use]
    pub fn label(&self) -> &str {
        self.inner.label()
    }

    /// Output directory path.
    #[must_use]
    pub fn output_path(&self) -> &Path {
        self.inner.output_path().as_path()
    }

    /// Shared `Arc<NdjsonWriter>` for code paths that need sync access.
    #[must_use]
    pub fn shared(&self) -> Arc<NdjsonWriter> {
        Arc::clone(&self.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(dir: &std::path::Path) -> FileWriterConfig {
        FileWriterConfig {
            path: dir.to_path_buf(),
            rotation: RotationPeriod::Daily,
            max_age_days: 1,
            compress_rotated: false,
        }
    }

    #[test]
    fn test_write_single_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path());

        let writer = NdjsonWriter::new(&config, "test-svc", "out.ndjson", "test").expect("create");
        assert_eq!(writer.lines_written(), 0);
        assert_eq!(writer.write_errors(), 0);

        writer.write_line(b"{\"msg\":\"hello\"}\n").expect("write");
        assert_eq!(writer.lines_written(), 1);

        let content =
            std::fs::read_to_string(dir.path().join("test-svc/out.ndjson")).expect("read");
        assert_eq!(content.trim(), r#"{"msg":"hello"}"#);
    }

    #[test]
    fn test_write_multiple_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path());

        let writer =
            NdjsonWriter::new(&config, "multi", "events.ndjson", "output").expect("create");

        for i in 0..3 {
            let line = format!("{{\"n\":{i}}}\n");
            writer.write_line(line.as_bytes()).expect("write");
        }
        assert_eq!(writer.lines_written(), 3);

        let content =
            std::fs::read_to_string(dir.path().join("multi/events.ndjson")).expect("read");
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_write_buf_batch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path());

        let writer = NdjsonWriter::new(&config, "batch", "out.ndjson", "test").expect("create");

        let mut buf = Vec::new();
        for i in 0..5 {
            buf.extend_from_slice(format!("{{\"n\":{i}}}\n").as_bytes());
        }
        writer.write_buf(&buf, 5).expect("write batch");
        assert_eq!(writer.lines_written(), 5);

        let content = std::fs::read_to_string(dir.path().join("batch/out.ndjson")).expect("read");
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn test_debug_format() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path());

        let writer = NdjsonWriter::new(&config, "dbg", "out.ndjson", "dlq").expect("create");
        let debug = format!("{writer:?}");
        assert!(debug.contains("NdjsonWriter"));
        assert!(debug.contains("dlq"));
    }

    #[test]
    fn test_label_and_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path());

        let writer = NdjsonWriter::new(&config, "svc", "data.ndjson", "output").expect("create");
        assert_eq!(writer.label(), "output");
        assert_eq!(writer.output_path(), &dir.path().join("svc"));
    }

    // -----------------------------------------------------------------
    // AsyncNdjsonWriter tests — these prove the async wrapper actually
    // moves the sync work off the runtime thread.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn async_write_line_writes_to_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = test_config(dir.path());
        let writer = NdjsonWriter::new(&cfg, "async-svc", "out.ndjson", "test").expect("create");
        let async_w = AsyncNdjsonWriter::new(writer);

        async_w
            .write_line(b"{\"k\":\"v\"}\n".to_vec())
            .await
            .expect("write_line");
        assert_eq!(async_w.lines_written(), 1);
        assert_eq!(async_w.write_errors(), 0);
        assert_eq!(async_w.label(), "test");
        assert_eq!(async_w.output_path(), dir.path().join("async-svc"));

        let body = std::fs::read_to_string(dir.path().join("async-svc/out.ndjson")).expect("read");
        assert_eq!(body.trim(), r#"{"k":"v"}"#);
    }

    #[tokio::test]
    async fn async_writer_from_arc_shares_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = test_config(dir.path());
        let writer = NdjsonWriter::new(&cfg, "share", "out.ndjson", "test").expect("create");
        let shared = Arc::new(writer);
        let a = AsyncNdjsonWriter::from_arc(Arc::clone(&shared));
        let b = AsyncNdjsonWriter::from_arc(Arc::clone(&shared));

        a.write_line(b"{\"a\":1}\n".to_vec()).await.expect("a");
        b.write_line(b"{\"b\":2}\n".to_vec()).await.expect("b");

        // Both views see the shared counter.
        assert_eq!(a.lines_written(), 2);
        assert_eq!(b.lines_written(), 2);
        assert!(Arc::ptr_eq(&a.shared(), &b.shared()));
    }

    #[tokio::test]
    async fn async_write_buf_writes_batch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = test_config(dir.path());
        let writer = NdjsonWriter::new(&cfg, "batch", "out.ndjson", "test").expect("create");
        let async_w = AsyncNdjsonWriter::new(writer);

        let mut buf = Vec::new();
        for i in 0..5 {
            buf.extend_from_slice(format!("{{\"n\":{i}}}\n").as_bytes());
        }
        async_w.write_buf(buf, 5).await.expect("write_buf");
        assert_eq!(async_w.lines_written(), 5);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn async_writer_does_not_block_runtime() {
        // Prove that concurrent writers + ticker on the same runtime
        // make progress concurrently — i.e. write_line releases the
        // runtime thread.
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = test_config(dir.path());
        let writer = NdjsonWriter::new(&cfg, "concurrent", "out.ndjson", "test").expect("create");
        let async_w = AsyncNdjsonWriter::new(writer);

        let ticker_fired = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let tf = ticker_fired.clone();
        let ticker = tokio::spawn(async move {
            let mut t = tokio::time::interval(std::time::Duration::from_millis(2));
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            t.tick().await; // burn the t=0 tick
            for _ in 0..20 {
                t.tick().await;
                tf.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });

        let mut writers = Vec::new();
        for _ in 0..4 {
            let w = async_w.clone();
            writers.push(tokio::spawn(async move {
                for i in 0..50_u32 {
                    w.write_line(format!("{{\"n\":{i}}}\n").into_bytes())
                        .await
                        .expect("write");
                }
            }));
        }
        for h in writers {
            h.await.expect("writer task");
        }
        ticker.await.expect("ticker task");

        assert_eq!(async_w.lines_written(), 200);
        let ticks = ticker_fired.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            ticks >= 10,
            "ticker fired only {ticks} times — writers starved the runtime",
        );
    }
}
