// Project:   hyperi-rustlib
// File:      src/spool/queue.rs
// Purpose:   Disk-backed async FIFO queue implementation using yaque
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Disk-backed async FIFO queue implementation.

use crate::spool::{Result, SpoolConfig, SpoolError};
use std::path::Path;
use yaque::{Receiver, Sender};

/// Disk-backed async FIFO queue with optional compression.
///
/// Crash-safe writes, survives restarts. Built on
/// [yaque](https://crates.io/crates/yaque) (transactional persistent queue).
pub struct Spool {
    sender: Sender,
    receiver: Receiver,
    config: SpoolConfig,
    len: usize,
}

impl Spool {
    /// Open the queue at the configured path, creating it if absent.
    ///
    /// # Errors
    ///
    /// Returns an error if the queue cannot be opened or created.
    pub async fn open(config: SpoolConfig) -> Result<Self> {
        let (sender, receiver) = yaque::channel(&config.path).map_err(|e| SpoolError::Open {
            path: config.path.display().to_string(),
            message: e.to_string(),
        })?;

        // yaque exposes no count API -- parse segment files to count items
        // between the receiver position and the end.
        let len = count_existing_items(&config.path).unwrap_or(0);

        Ok(Self {
            sender,
            receiver,
            config,
            len,
        })
    }

    /// Create a new spool at the given path with default settings.
    ///
    /// # Errors
    ///
    /// Returns an error if the queue cannot be created.
    pub async fn create(path: impl AsRef<Path>) -> Result<Self> {
        Self::open(SpoolConfig::new(path.as_ref())).await
    }

    /// Create a new spool with compression enabled.
    ///
    /// # Errors
    ///
    /// Returns an error if the queue cannot be created.
    pub async fn create_compressed(path: impl AsRef<Path>) -> Result<Self> {
        Self::open(SpoolConfig::with_compression(path.as_ref())).await
    }

    /// Push data onto the queue, compressing first if enabled.
    ///
    /// # Errors
    ///
    /// Errors on size/item-count cap, compression failure, or I/O.
    pub async fn push(&mut self, data: &[u8]) -> Result<()> {
        if let Some(max) = self.config.max_items
            && self.len >= max
        {
            return Err(SpoolError::MaxItemsReached { max });
        }

        // Approximate -- checked before write.
        if let Some(max_bytes) = self.config.max_size_bytes
            && self.file_size()? >= max_bytes
        {
            return Err(SpoolError::MaxSizeReached { max_bytes });
        }

        let to_write = if self.config.compress {
            self.compress(data)?
        } else {
            data.to_vec()
        };

        self.sender
            .send(to_write)
            .await
            .map_err(|e| SpoolError::Queue(e.to_string()))?;

        self.len += 1;
        #[cfg(feature = "metrics")]
        {
            ::metrics::gauge!("dfe_spool_queue_depth").set(self.len as f64);
            // New default (metrics audit): spill RATE. drain < enqueue => backlog.
            ::metrics::counter!("dfe_spool_enqueue_total").increment(1);
        }
        Ok(())
    }

    /// Peek at the first item without removing it.
    ///
    /// yaque has no direct peek -- `try_recv` then let the guard roll back
    /// on drop to leave the item in the queue.
    ///
    /// # Errors
    ///
    /// Returns an error if decompression fails or an I/O error occurs.
    pub async fn peek(&mut self) -> Result<Option<Vec<u8>>> {
        match self.receiver.try_recv() {
            Ok(guard) => {
                let raw_data = guard.to_vec();
                let data = if self.config.compress {
                    zstd::decode_all(raw_data.as_slice())
                        .map_err(|e| SpoolError::Decompression(e.to_string()))?
                } else {
                    raw_data
                };
                // No commit -- guard rollback on drop keeps the item.
                drop(guard);
                Ok(Some(data))
            }
            Err(yaque::TryRecvError::Io(e)) => Err(SpoolError::Io(e)),
            Err(yaque::TryRecvError::QueueEmpty) => Ok(None),
        }
    }

    /// Remove the first item from the queue.
    ///
    /// # Errors
    ///
    /// Returns an error if an I/O error occurs.
    pub async fn pop(&mut self) -> Result<()> {
        match self.receiver.try_recv() {
            Ok(guard) => {
                guard
                    .commit()
                    .map_err(|e| SpoolError::Queue(e.to_string()))?;
                self.len = self.len.saturating_sub(1);
                Ok(())
            }
            Err(yaque::TryRecvError::Io(e)) => Err(SpoolError::Io(e)),
            Err(yaque::TryRecvError::QueueEmpty) => Ok(()), // Nothing to pop
        }
    }

    /// Pop and return the first item, atomically receiving and removing it.
    ///
    /// # Errors
    ///
    /// Returns an error if decompression fails or an I/O error occurs.
    pub async fn pop_front(&mut self) -> Result<Option<Vec<u8>>> {
        match self.receiver.try_recv() {
            Ok(guard) => {
                let raw_data = guard.to_vec();
                let data = if self.config.compress {
                    zstd::decode_all(raw_data.as_slice())
                        .map_err(|e| SpoolError::Decompression(e.to_string()))?
                } else {
                    raw_data
                };
                guard
                    .commit()
                    .map_err(|e| SpoolError::Queue(e.to_string()))?;
                self.len = self.len.saturating_sub(1);
                #[cfg(feature = "metrics")]
                {
                    ::metrics::gauge!("dfe_spool_queue_depth").set(self.len as f64);
                    // New default (metrics audit): drain RATE.
                    ::metrics::counter!("dfe_spool_dequeue_total").increment(1);
                }
                Ok(Some(data))
            }
            Err(yaque::TryRecvError::Io(e)) => Err(SpoolError::Io(e)),
            Err(yaque::TryRecvError::QueueEmpty) => Ok(None),
        }
    }

    /// Receive an item, awaiting if the queue is empty. Preferred consumer API.
    ///
    /// # Errors
    ///
    /// Returns an error if decompression fails or an I/O error occurs.
    pub async fn recv(&mut self) -> Result<Vec<u8>> {
        let guard = self
            .receiver
            .recv()
            .await
            .map_err(|e| SpoolError::Queue(e.to_string()))?;

        let raw_data = guard.to_vec();
        let data = if self.config.compress {
            zstd::decode_all(raw_data.as_slice())
                .map_err(|e| SpoolError::Decompression(e.to_string()))?
        } else {
            raw_data
        };

        guard
            .commit()
            .map_err(|e| SpoolError::Queue(e.to_string()))?;
        self.len = self.len.saturating_sub(1);
        #[cfg(feature = "metrics")]
        {
            ::metrics::gauge!("dfe_spool_queue_depth").set(self.len as f64);
            // New default (metrics audit): drain RATE.
            ::metrics::counter!("dfe_spool_dequeue_total").increment(1);
        }
        Ok(data)
    }

    /// Approximate item count, tracked internally.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Check if the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Drain all items from the queue.
    ///
    /// # Errors
    ///
    /// Returns an error if an I/O error occurs.
    pub fn clear(&mut self) -> Result<()> {
        // yaque has no built-in clear -- drain by committing every item.
        loop {
            match self.receiver.try_recv() {
                Ok(guard) => {
                    guard
                        .commit()
                        .map_err(|e| SpoolError::Queue(e.to_string()))?;
                }
                Err(yaque::TryRecvError::QueueEmpty) => break,
                Err(yaque::TryRecvError::Io(e)) => return Err(SpoolError::Io(e)),
            }
        }
        self.len = 0;
        #[cfg(feature = "metrics")]
        ::metrics::gauge!("dfe_spool_queue_depth").set(0.0);
        Ok(())
    }

    /// Get the configuration for this spool.
    #[must_use]
    pub fn config(&self) -> &SpoolConfig {
        &self.config
    }

    /// Get the approximate directory size in bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory cannot be read.
    pub fn file_size(&self) -> Result<u64> {
        let mut total = 0u64;
        if self.config.path.is_dir() {
            for entry in std::fs::read_dir(&self.config.path)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    total += entry.metadata()?.len();
                }
            }
        }
        Ok(total)
    }

    /// Compress data using zstd.
    fn compress(&self, data: &[u8]) -> Result<Vec<u8>> {
        zstd::encode_all(data, self.config.compression_level)
            .map_err(|e| SpoolError::Compression(e.to_string()))
    }
}

/// Count items in a yaque queue dir by walking segment files.
///
/// yaque stores messages as `[4-byte Hamming header][payload]` in `<n>.q`
/// segments; receiver position lives in `recv-metadata`. Count from the
/// receiver position to the end of the highest segment.
fn count_existing_items(path: &std::path::Path) -> std::io::Result<usize> {
    if !path.is_dir() {
        return Ok(0);
    }

    // Read receiver state from recv-metadata (two big-endian u64: segment, position)
    let recv_metadata_path = path.join("recv-metadata");
    let (recv_segment, recv_position) = if recv_metadata_path.exists() {
        let data = std::fs::read(&recv_metadata_path)?;
        if data.len() >= 16 {
            let segment = u64::from_be_bytes(data[0..8].try_into().unwrap_or([0; 8]));
            let position = u64::from_be_bytes(data[8..16].try_into().unwrap_or([0; 8]));
            (segment, position)
        } else {
            (0, 0)
        }
    } else {
        (0, 0)
    };

    // Collect all segment numbers
    let mut segments: Vec<u64> = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let file_path = entry.path();
        if file_path.extension().and_then(|e| e.to_str()) == Some("q")
            && let Some(stem) = file_path.file_stem().and_then(|s| s.to_str())
            && let Ok(seg_num) = stem.parse::<u64>()
            && seg_num >= recv_segment
        {
            segments.push(seg_num);
        }
    }
    segments.sort_unstable();

    let mut count = 0usize;
    // Header EOF marker in yaque
    let header_eof: [u8; 4] = [255, 255, 255, 255];

    for &seg_num in &segments {
        let seg_path = path.join(format!("{seg_num}.q"));
        let file_data = std::fs::read(&seg_path)?;

        // Start position: if this is the receiver's segment, skip to receiver position
        #[allow(clippy::cast_possible_truncation)]
        let start = if seg_num == recv_segment {
            recv_position as usize
        } else {
            0
        };

        let mut pos = start;
        while pos + 4 <= file_data.len() {
            let header_bytes: [u8; 4] = file_data[pos..pos + 4].try_into().unwrap_or([0; 4]);

            // Check for EOF marker
            if header_bytes == header_eof {
                break; // End of segment, move to next
            }

            // Decode length from Hamming-encoded header (lower 26 bits)
            let encoded = u32::from_be_bytes(header_bytes);
            let payload_len = (encoded & 0x03_FF_FF_FF) as usize;

            pos += 4 + payload_len;
            if pos <= file_data.len() {
                count += 1;
            }
        }
    }

    Ok(count)
}

impl std::fmt::Debug for Spool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spool")
            .field("path", &self.config.path)
            .field("len", &self.len)
            .field("compress", &self.config.compress)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_create_and_push_pop() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test-queue");

        let mut spool = Spool::create(&path).await.unwrap();
        assert!(spool.is_empty());

        spool.push(b"hello").await.unwrap();
        spool.push(b"world").await.unwrap();

        assert_eq!(spool.len(), 2);
        assert!(!spool.is_empty());

        assert_eq!(spool.pop_front().await.unwrap(), Some(b"hello".to_vec()));
        assert_eq!(spool.pop_front().await.unwrap(), Some(b"world".to_vec()));

        assert!(spool.is_empty());
    }

    #[tokio::test]
    async fn test_pop_front_empty() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test-queue");

        let mut spool = Spool::create(&path).await.unwrap();
        assert_eq!(spool.pop_front().await.unwrap(), None);
    }

    #[tokio::test]
    async fn test_compression() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test-queue");

        let mut spool = Spool::create_compressed(&path).await.unwrap();

        let data = b"hello world ".repeat(100);
        spool.push(&data).await.unwrap();

        // Verify decompression works - data comes back correctly
        let retrieved = spool.pop_front().await.unwrap().unwrap();
        assert_eq!(retrieved, data);
    }

    #[tokio::test]
    async fn test_max_items_limit() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test-queue");

        let config = SpoolConfig::new(&path).max_items(2);
        let mut spool = Spool::open(config).await.unwrap();

        spool.push(b"one").await.unwrap();
        spool.push(b"two").await.unwrap();

        let result = spool.push(b"three").await;
        assert!(matches!(
            result,
            Err(SpoolError::MaxItemsReached { max: 2 })
        ));
    }

    #[tokio::test]
    async fn test_clear() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test-queue");

        let mut spool = Spool::create(&path).await.unwrap();
        spool.push(b"one").await.unwrap();
        spool.push(b"two").await.unwrap();

        assert_eq!(spool.len(), 2);
        spool.clear().unwrap();
        assert!(spool.is_empty());
    }

    #[tokio::test]
    async fn test_len_survives_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test-reopen-queue");

        // Open, push items, then drop
        {
            let mut spool = Spool::create(&path).await.unwrap();
            spool.push(b"one").await.unwrap();
            spool.push(b"two").await.unwrap();
            spool.push(b"three").await.unwrap();
            assert_eq!(spool.len(), 3);
        }

        // Reopen -- len should reflect existing items
        {
            let spool = Spool::create(&path).await.unwrap();
            assert_eq!(spool.len(), 3);
        }
    }

    #[tokio::test]
    async fn test_len_survives_partial_consume_and_reopen() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test-partial-queue");

        // Open, push 5, consume 2
        {
            let mut spool = Spool::create(&path).await.unwrap();
            for i in 0..5 {
                spool.push(format!("item-{i}").as_bytes()).await.unwrap();
            }
            assert_eq!(spool.len(), 5);
            spool.pop_front().await.unwrap(); // consume 1
            spool.pop_front().await.unwrap(); // consume 2
            assert_eq!(spool.len(), 3);
        }

        // Reopen -- should show 3 remaining
        {
            let spool = Spool::create(&path).await.unwrap();
            assert_eq!(spool.len(), 3);
        }
    }

    #[tokio::test]
    async fn test_debug_format() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test-queue");

        let spool = Spool::create(&path).await.unwrap();
        let debug = format!("{spool:?}");
        assert!(debug.contains("Spool"));
        assert!(debug.contains("test-queue"));
    }
}
