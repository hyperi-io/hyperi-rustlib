// Project:   hyperi-rustlib
// File:      src/transport/file.rs
// Purpose:   NDJSON file transport
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # File Transport
//!
//! NDJSON (newline-delimited JSON) file transport for debugging, audit
//! trails, and replay. Wraps async file I/O behind the Transport traits.
//!
//! ## Send
//!
//! Appends one NDJSON line per `send()` call to the configured file path.
//!
//! ## Receive
//!
//! Reads NDJSON lines from the file, tracking byte offset for commit.
//! Position is persisted to a `.pos` sidecar file so reads survive restarts.
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::transport::file::{FileTransport, FileTransportConfig};
//!
//! let config = FileTransportConfig { path: "/tmp/events.ndjson".into(), append: true };
//! let transport = FileTransport::new(&config).await?;
//! transport.send("events", b"{\"msg\":\"hello\"}").await;
//! ```

use super::error::{TransportError, TransportResult};
use super::traits::{CommitToken, TransportBase, TransportReceiver, TransportSender};
use super::types::{Message, PayloadFormat, SendResult};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

/// Commit token for file transport.
///
/// Contains the byte offset in the file after reading the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileToken {
    /// Byte offset after the line was read.
    pub offset: u64,
}

impl CommitToken for FileToken {}

impl std::fmt::Display for FileToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "file:{}", self.offset)
    }
}

/// Configuration for file transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTransportConfig {
    /// File path for read/write.
    pub path: String,

    /// Append mode (default true for send).
    #[serde(default = "default_append")]
    pub append: bool,
}

fn default_append() -> bool {
    true
}

impl Default for FileTransportConfig {
    fn default() -> Self {
        Self {
            path: String::new(),
            append: true,
        }
    }
}

impl FileTransportConfig {
    /// Load from the config cascade under the `transport.file` key.
    #[must_use]
    pub fn from_cascade() -> Self {
        #[cfg(feature = "config")]
        {
            if let Some(cfg) = crate::config::try_get()
                && let Ok(tc) = cfg.unmarshal_key_registered::<Self>("transport.file")
            {
                return tc;
            }
        }
        Self::default()
    }
}

/// Internal state for the write side.
struct WriteState {
    file: tokio::fs::File,
}

/// Internal state for the read side.
struct ReadState {
    reader: BufReader<tokio::fs::File>,
    offset: u64,
    line_buf: String,
}

/// NDJSON file transport.
///
/// Supports both send (append) and receive (sequential read with
/// position tracking). Position is persisted to a `.pos` sidecar
/// file so reads survive process restarts.
pub struct FileTransport {
    config: FileTransportConfig,
    writer: Mutex<Option<WriteState>>,
    reader: Mutex<Option<ReadState>>,
    closed: Arc<AtomicBool>,
}

impl FileTransport {
    /// Create a new file transport.
    ///
    /// # Errors
    ///
    /// Returns error if the file path is empty.
    pub async fn new(config: &FileTransportConfig) -> TransportResult<Self> {
        if config.path.is_empty() {
            return Err(TransportError::Config("file path is empty".into()));
        }

        #[cfg(feature = "logger")]
        tracing::info!(path = %config.path, append = config.append, "File transport opened");

        let closed = Arc::new(AtomicBool::new(false));

        #[cfg(feature = "health")]
        {
            let h = Arc::clone(&closed);
            crate::health::HealthRegistry::register("transport:file", move || {
                if h.load(Ordering::Relaxed) {
                    crate::health::HealthStatus::Unhealthy
                } else {
                    crate::health::HealthStatus::Healthy
                }
            });
        }

        Ok(Self {
            config: config.clone(),
            writer: Mutex::new(None),
            reader: Mutex::new(None),
            closed,
        })
    }

    /// Path to the `.pos` sidecar file that tracks read position.
    fn pos_path(data_path: &Path) -> PathBuf {
        let mut pos_path = data_path.as_os_str().to_owned();
        pos_path.push(".pos");
        PathBuf::from(pos_path)
    }

    /// Load committed read position from the sidecar file.
    async fn load_position(data_path: &Path) -> u64 {
        let pos_path = Self::pos_path(data_path);
        match tokio::fs::read_to_string(&pos_path).await {
            Ok(content) => content.trim().parse::<u64>().unwrap_or(0),
            Err(_) => 0,
        }
    }

    /// Save read position to the sidecar file.
    async fn save_position(data_path: &Path, offset: u64) -> TransportResult<()> {
        let pos_path = Self::pos_path(data_path);
        tokio::fs::write(&pos_path, offset.to_string())
            .await
            .map_err(|e| TransportError::Commit(format!("failed to write position file: {e}")))
    }

    /// Lazily open the write file handle.
    async fn ensure_writer(&self) -> TransportResult<()> {
        let mut guard = self.writer.lock().await;
        if guard.is_none() {
            let file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(self.config.append)
                .write(true)
                .open(&self.config.path)
                .await
                .map_err(|e| {
                    TransportError::Connection(format!(
                        "failed to open '{}' for writing: {e}",
                        self.config.path
                    ))
                })?;
            *guard = Some(WriteState { file });
        }
        Ok(())
    }

    /// Lazily open the read file handle and seek to committed position.
    async fn ensure_reader(&self) -> TransportResult<()> {
        let mut guard = self.reader.lock().await;
        if guard.is_none() {
            let path = Path::new(&self.config.path);

            // If the file does not exist yet, there is nothing to read
            if !path.exists() {
                return Err(TransportError::Recv(format!(
                    "file '{}' does not exist",
                    self.config.path
                )));
            }

            let offset = Self::load_position(path).await;
            let mut file = tokio::fs::File::open(&self.config.path)
                .await
                .map_err(|e| {
                    TransportError::Connection(format!(
                        "failed to open '{}' for reading: {e}",
                        self.config.path
                    ))
                })?;

            // Seek to committed position
            file.seek(std::io::SeekFrom::Start(offset))
                .await
                .map_err(|e| {
                    TransportError::Recv(format!("failed to seek to offset {offset}: {e}"))
                })?;

            *guard = Some(ReadState {
                reader: BufReader::new(file),
                offset,
                line_buf: String::with_capacity(4096),
            });
        }
        Ok(())
    }
}

impl TransportBase for FileTransport {
    async fn close(&self) -> TransportResult<()> {
        self.closed.store(true, Ordering::Relaxed);

        // Flush and drop writer
        if let Some(mut state) = self.writer.lock().await.take() {
            let _ = state.file.flush().await;
        }

        // Drop reader
        let _ = self.reader.lock().await.take();

        Ok(())
    }

    fn is_healthy(&self) -> bool {
        !self.closed.load(Ordering::Relaxed)
    }

    fn name(&self) -> &'static str {
        "file"
    }
}

impl TransportSender for FileTransport {
    async fn send(&self, _key: &str, payload: &[u8]) -> SendResult {
        if self.closed.load(Ordering::Relaxed) {
            return SendResult::Fatal(TransportError::Closed);
        }

        if let Err(e) = self.ensure_writer().await {
            return SendResult::Fatal(e);
        }

        let mut guard = self.writer.lock().await;
        let Some(state) = guard.as_mut() else {
            return SendResult::Fatal(TransportError::Internal("writer not initialised".into()));
        };

        // Write payload + newline as a single operation
        if let Err(e) = state.file.write_all(payload).await {
            #[cfg(feature = "logger")]
            tracing::warn!(error = %e, "File transport: write error");
            return SendResult::Fatal(TransportError::Send(format!("write failed: {e}")));
        }
        if let Err(e) = state.file.write_all(b"\n").await {
            #[cfg(feature = "logger")]
            tracing::warn!(error = %e, "File transport: newline write error");
            return SendResult::Fatal(TransportError::Send(format!("write newline failed: {e}")));
        }
        if let Err(e) = state.file.flush().await {
            #[cfg(feature = "logger")]
            tracing::warn!(error = %e, "File transport: flush error");
            return SendResult::Fatal(TransportError::Send(format!("flush failed: {e}")));
        }

        #[cfg(feature = "logger")]
        tracing::debug!(bytes = payload.len(), "File transport: message sent");

        #[cfg(feature = "metrics")]
        metrics::counter!("dfe_transport_sent_total", "transport" => "file").increment(1);

        SendResult::Ok
    }
}

impl TransportReceiver for FileTransport {
    type Token = FileToken;

    async fn recv(&self, max: usize) -> TransportResult<Vec<Message<Self::Token>>> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(TransportError::Closed);
        }

        self.ensure_reader().await?;

        let mut guard = self.reader.lock().await;
        let state = guard
            .as_mut()
            .ok_or_else(|| TransportError::Internal("reader not initialised".into()))?;

        let mut messages = Vec::with_capacity(max.min(100));

        for _ in 0..max {
            state.line_buf.clear();
            let bytes_read = state
                .reader
                .read_line(&mut state.line_buf)
                .await
                .map_err(|e| TransportError::Recv(format!("read failed: {e}")))?;

            if bytes_read == 0 {
                // EOF
                break;
            }

            state.offset += bytes_read as u64;

            // Strip trailing newline
            let line = state.line_buf.trim_end_matches('\n').trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }

            let payload = line.as_bytes().to_vec();
            let format = PayloadFormat::detect(&payload);
            let timestamp_ms = chrono::Utc::now().timestamp_millis();

            messages.push(Message {
                key: None,
                payload,
                token: FileToken {
                    offset: state.offset,
                },
                timestamp_ms: Some(timestamp_ms),
                format,
            });
        }

        #[cfg(feature = "logger")]
        if !messages.is_empty() {
            tracing::debug!(lines = messages.len(), "File transport: batch received");
        }

        #[cfg(feature = "metrics")]
        if !messages.is_empty() {
            metrics::counter!("dfe_transport_received_total", "transport" => "file")
                .increment(messages.len() as u64);
        }

        Ok(messages)
    }

    async fn commit(&self, tokens: &[Self::Token]) -> TransportResult<()> {
        if let Some(max_token) = tokens.iter().max_by_key(|t| t.offset) {
            let path = Path::new(&self.config.path);
            Self::save_position(path, max_token.offset).await?;

            #[cfg(feature = "logger")]
            tracing::debug!(
                offset = max_token.offset,
                "File transport: position committed"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn make_transport(dir: &TempDir, filename: &str) -> FileTransport {
        let path = dir.path().join(filename);
        let config = FileTransportConfig {
            path: path.to_str().unwrap().to_string(),
            append: true,
        };
        FileTransport::new(&config).await.unwrap()
    }

    #[tokio::test]
    async fn send_and_receive() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.ndjson");
        let path_str = path.to_str().unwrap().to_string();

        // Write messages
        let config = FileTransportConfig {
            path: path_str.clone(),
            append: true,
        };
        let sender = FileTransport::new(&config).await.unwrap();

        let r1 = sender.send("key", b"{\"msg\":\"hello\"}").await;
        assert!(r1.is_ok());
        let r2 = sender.send("key", b"{\"msg\":\"world\"}").await;
        assert!(r2.is_ok());
        sender.close().await.unwrap();

        // Read messages back
        let reader_config = FileTransportConfig {
            path: path_str,
            append: true,
        };
        let reader = FileTransport::new(&reader_config).await.unwrap();
        let messages = reader.recv(10).await.unwrap();

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].payload, b"{\"msg\":\"hello\"}");
        assert_eq!(messages[1].payload, b"{\"msg\":\"world\"}");

        // Tokens should have increasing offsets
        assert!(messages[1].token.offset > messages[0].token.offset);
    }

    #[tokio::test]
    async fn commit_persists_position() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("commit_test.ndjson");
        let path_str = path.to_str().unwrap().to_string();

        // Write 3 messages
        let config = FileTransportConfig {
            path: path_str.clone(),
            append: true,
        };
        let sender = FileTransport::new(&config).await.unwrap();
        sender.send("k", b"line1").await;
        sender.send("k", b"line2").await;
        sender.send("k", b"line3").await;
        sender.close().await.unwrap();

        // Read first 2 messages and commit
        let r1 = FileTransport::new(&FileTransportConfig {
            path: path_str.clone(),
            append: true,
        })
        .await
        .unwrap();
        let msgs = r1.recv(2).await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].payload, b"line1");
        assert_eq!(msgs[1].payload, b"line2");

        // Commit up to message 2
        let tokens: Vec<_> = msgs.iter().map(|m| m.token).collect();
        r1.commit(&tokens).await.unwrap();
        r1.close().await.unwrap();

        // Open a new transport — should resume from committed position
        let r2 = FileTransport::new(&FileTransportConfig {
            path: path_str,
            append: true,
        })
        .await
        .unwrap();
        let remaining = r2.recv(10).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].payload, b"line3");
    }

    #[tokio::test]
    async fn close_prevents_operations() {
        let dir = TempDir::new().unwrap();
        let transport = make_transport(&dir, "close_test.ndjson").await;

        transport.close().await.unwrap();
        assert!(!transport.is_healthy());

        let result = transport.send("k", b"data").await;
        assert!(result.is_fatal());

        let result = transport.recv(1).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn file_token_display() {
        let token = FileToken { offset: 42 };
        assert_eq!(format!("{token}"), "file:42");
    }

    #[tokio::test]
    async fn recv_returns_empty_at_eof() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("eof_test.ndjson");
        let path_str = path.to_str().unwrap().to_string();

        // Write one line
        let config = FileTransportConfig {
            path: path_str.clone(),
            append: true,
        };
        let transport = FileTransport::new(&config).await.unwrap();
        transport.send("k", b"only_line").await;
        transport.close().await.unwrap();

        // Read all, then read again — should get empty
        let reader = FileTransport::new(&FileTransportConfig {
            path: path_str,
            append: true,
        })
        .await
        .unwrap();
        let msgs = reader.recv(10).await.unwrap();
        assert_eq!(msgs.len(), 1);

        let more = reader.recv(10).await.unwrap();
        assert!(more.is_empty());
    }

    #[tokio::test]
    async fn empty_path_is_config_error() {
        let result = FileTransport::new(&FileTransportConfig::default()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn transport_name() {
        let dir = TempDir::new().unwrap();
        let transport = make_transport(&dir, "name_test.ndjson").await;
        assert_eq!(transport.name(), "file");
    }
}
