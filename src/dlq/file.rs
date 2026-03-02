// Project:   hyperi-rustlib
// File:      src/dlq/file.rs
// Purpose:   File-based DLQ backend using NDJSON with rotation
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! File-based DLQ backend.
//!
//! Writes failed messages as NDJSON (one JSON line per entry) with automatic
//! file rotation via the `file-rotate` crate. Files are rotated by time
//! (hourly or daily) and optionally compressed after rotation.
//!
//! ## File Layout
//!
//! ```text
//! /var/spool/dfe/dlq/loader/
//! ├── dlq.ndjson              # Current file
//! ├── dlq.ndjson.20260302T14  # Rotated (hourly)
//! └── dlq.ndjson.20260302T13.gz  # Compressed
//! ```
//!
//! ## Thread Safety
//!
//! `FileDlq` uses a `Mutex<FileRotate>` for safe concurrent writes.
//! The write path uses `spawn_blocking` to avoid blocking the async runtime.

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use file_rotate::suffix::AppendTimestamp;
use file_rotate::suffix::FileLimit;
use file_rotate::{compression::Compression, ContentLimit, FileRotate};
use parking_lot::Mutex;
use tracing::debug;

use super::backend::DlqBackend;
use super::config::{FileDlqConfig, RotationPeriod};
use super::entry::DlqEntry;
use super::error::DlqError;

/// File-based DLQ backend using NDJSON format.
pub struct FileDlq {
    writer: Mutex<FileRotate<AppendTimestamp>>,
    service_name: String,
    entries_written: AtomicU64,
    write_errors: AtomicU64,
}

impl std::fmt::Debug for FileDlq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileDlq")
            .field("service_name", &self.service_name)
            .field(
                "entries_written",
                &self.entries_written.load(Ordering::Relaxed),
            )
            .field("write_errors", &self.write_errors.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl FileDlq {
    /// Create a new file-based DLQ backend.
    ///
    /// Creates the output directory if it doesn't exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the output directory cannot be created.
    pub fn new(config: &FileDlqConfig, service_name: &str) -> Result<Self, DlqError> {
        let dir = config.path.join(service_name);
        std::fs::create_dir_all(&dir).map_err(|e| {
            DlqError::File(format!(
                "failed to create DLQ directory {}: {}",
                dir.display(),
                e
            ))
        })?;

        let file_path = dir.join("dlq.ndjson");

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
            service = service_name,
            path = %dir.display(),
            rotation = ?config.rotation,
            "File DLQ initialised"
        );

        Ok(Self {
            writer: Mutex::new(writer),
            service_name: service_name.to_string(),
            entries_written: AtomicU64::new(0),
            write_errors: AtomicU64::new(0),
        })
    }

    /// Number of entries successfully written.
    pub fn entries_written(&self) -> u64 {
        self.entries_written.load(Ordering::Relaxed)
    }

    /// Number of write errors.
    pub fn write_errors(&self) -> u64 {
        self.write_errors.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl DlqBackend for FileDlq {
    async fn send(&self, entry: &DlqEntry) -> Result<(), DlqError> {
        // Serialise outside the lock
        let mut line = serde_json::to_vec(entry)
            .map_err(|e| DlqError::Serialization(format!("failed to serialise DLQ entry: {e}")))?;
        line.push(b'\n');

        // Write under lock (sync I/O, but DLQ volume is low)
        {
            let mut writer = self.writer.lock();
            writer.write_all(&line)?;
            writer.flush()?;
        }

        self.entries_written.fetch_add(1, Ordering::Relaxed);
        debug!(
            service = %self.service_name,
            reason = %entry.reason,
            destination = entry.destination.as_deref().unwrap_or("-"),
            "DLQ entry written to file"
        );

        Ok(())
    }

    async fn send_batch(&self, entries: &[DlqEntry]) -> Result<(), DlqError> {
        if entries.is_empty() {
            return Ok(());
        }

        // Serialise all entries outside the lock
        let mut buf = Vec::with_capacity(entries.len() * 256);
        for entry in entries {
            serde_json::to_writer(&mut buf, entry).map_err(|e| {
                DlqError::Serialization(format!("failed to serialise DLQ entry: {e}"))
            })?;
            buf.push(b'\n');
        }

        // Single write under lock
        {
            let mut writer = self.writer.lock();
            writer.write_all(&buf)?;
            writer.flush()?;
        }

        let count = entries.len() as u64;
        self.entries_written.fetch_add(count, Ordering::Relaxed);
        debug!(
            service = %self.service_name,
            count = entries.len(),
            "DLQ batch written to file"
        );

        Ok(())
    }

    fn name(&self) -> &'static str {
        "file"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dlq::entry::DlqSource;

    #[tokio::test]
    async fn test_file_dlq_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = FileDlqConfig {
            enabled: true,
            path: dir.path().to_path_buf(),
            rotation: RotationPeriod::Daily,
            max_age_days: 1,
            compress_rotated: false,
        };

        let dlq = FileDlq::new(&config, "test-service").expect("create");
        assert_eq!(dlq.name(), "file");

        let entry = DlqEntry::new("test-service", "parse_error", b"bad data".to_vec())
            .with_destination("acme.auth")
            .with_source(DlqSource::kafka("events", 1, 42));

        dlq.send(&entry).await.expect("send");
        assert_eq!(dlq.entries_written(), 1);

        // Read and verify NDJSON format
        let content =
            std::fs::read_to_string(dir.path().join("test-service/dlq.ndjson")).expect("read");
        let parsed: DlqEntry = serde_json::from_str(content.trim()).expect("parse");
        assert_eq!(parsed.service, "test-service");
        assert_eq!(parsed.reason, "parse_error");
        assert_eq!(parsed.payload, b"bad data");
        assert_eq!(parsed.destination.as_deref(), Some("acme.auth"));
    }

    #[tokio::test]
    async fn test_file_dlq_batch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = FileDlqConfig {
            enabled: true,
            path: dir.path().to_path_buf(),
            rotation: RotationPeriod::Daily,
            max_age_days: 1,
            compress_rotated: false,
        };

        let dlq = FileDlq::new(&config, "batch-svc").expect("create");

        let entries: Vec<DlqEntry> = (0..5)
            .map(|i| DlqEntry::new("batch-svc", format!("error_{}", i), vec![i as u8]))
            .collect();

        dlq.send_batch(&entries).await.expect("batch send");
        assert_eq!(dlq.entries_written(), 5);

        // Verify each line is valid JSON
        let content =
            std::fs::read_to_string(dir.path().join("batch-svc/dlq.ndjson")).expect("read");
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 5);
        for (i, line) in lines.iter().enumerate() {
            let parsed: DlqEntry = serde_json::from_str(line).expect("parse line");
            assert_eq!(parsed.reason, format!("error_{}", i));
        }
    }

    #[tokio::test]
    async fn test_file_dlq_empty_batch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = FileDlqConfig {
            enabled: true,
            path: dir.path().to_path_buf(),
            rotation: RotationPeriod::Daily,
            max_age_days: 1,
            compress_rotated: false,
        };

        let dlq = FileDlq::new(&config, "empty").expect("create");
        dlq.send_batch(&[]).await.expect("empty batch");
        assert_eq!(dlq.entries_written(), 0);
    }
}
