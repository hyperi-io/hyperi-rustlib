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
//! ## Thread Safety
//!
//! Uses `parking_lot::Mutex<FileRotate>` for safe concurrent writes.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use file_rotate::suffix::AppendTimestamp;
use file_rotate::suffix::FileLimit;
use file_rotate::{compression::Compression, ContentLimit, FileRotate};
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
}
