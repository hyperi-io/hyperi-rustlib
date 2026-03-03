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
//! ## File Layout
//!
//! ```text
//! /var/spool/dfe/output/loader/
//! ├── events.ndjson              # Current file
//! ├── events.ndjson.20260302T14  # Rotated (hourly)
//! └── events.ndjson.20260302T13.gz  # Compressed
//! ```

use tracing::debug;

use crate::io::NdjsonWriter;

use super::config::FileOutputConfig;
use super::error::OutputError;

/// File output sink for raw NDJSON events.
///
/// Wraps [`NdjsonWriter`] with output-specific configuration and logging.
pub struct FileOutput {
    writer: NdjsonWriter,
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

        Ok(Self { writer })
    }

    /// Write a single raw JSON bytes line.
    ///
    /// The data should be a complete JSON object. A trailing newline is
    /// appended automatically if not present.
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

    /// Write a batch of raw JSON bytes lines.
    ///
    /// Each entry should be a complete JSON object. Newlines are appended
    /// automatically between entries.
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

    /// Number of lines successfully written.
    pub fn lines_written(&self) -> u64 {
        self.writer.lines_written()
    }

    /// Number of write errors encountered.
    pub fn write_errors(&self) -> u64 {
        self.writer.write_errors()
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
}
