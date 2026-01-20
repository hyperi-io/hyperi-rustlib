// Project:   hs-rustlib
// File:      src/spool/queue.rs
// Purpose:   Disk-backed async FIFO queue implementation using yaque
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Disk-backed async FIFO queue implementation.

use crate::spool::{Result, SpoolConfig, SpoolError};
use std::path::Path;
use yaque::{Receiver, Sender};

/// A disk-backed async FIFO queue with optional compression.
///
/// Provides persistent storage for binary data with crash-safe writes.
/// Items are stored in FIFO order and survive application restarts.
///
/// Built on [yaque](https://crates.io/crates/yaque), a fast, async,
/// persistent queue with transactional semantics.
pub struct Spool {
    sender: Sender,
    receiver: Receiver,
    config: SpoolConfig,
    len: usize,
}

impl Spool {
    /// Open or create a spool at the configured path.
    ///
    /// If the directory exists, opens the existing queue.
    /// If the directory doesn't exist, creates a new queue.
    ///
    /// # Errors
    ///
    /// Returns an error if the queue cannot be opened or created.
    pub async fn open(config: SpoolConfig) -> Result<Self> {
        let (sender, receiver) = yaque::channel(&config.path).map_err(|e| SpoolError::Open {
            path: config.path.display().to_string(),
            message: e.to_string(),
        })?;

        // Get initial count by iterating (yaque doesn't expose count directly)
        // We track this ourselves for efficiency
        let len = 0; // Will be tracked as items are pushed/popped

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

    /// Push data onto the queue.
    ///
    /// If compression is enabled, the data is compressed before storage.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The queue has reached its maximum item count
    /// - Compression fails
    /// - I/O error occurs
    pub async fn push(&mut self, data: &[u8]) -> Result<()> {
        // Check item limit
        if let Some(max) = self.config.max_items {
            if self.len >= max {
                return Err(SpoolError::MaxItemsReached { max });
            }
        }

        // Check size limit (approximate - check before write)
        if let Some(max_bytes) = self.config.max_size_bytes {
            if self.file_size()? >= max_bytes {
                return Err(SpoolError::MaxSizeReached { max_bytes });
            }
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
        Ok(())
    }

    /// Peek at the first item in the queue without removing it.
    ///
    /// Note: In yaque, there's no direct peek. This uses try_recv and
    /// lets the guard rollback on drop.
    ///
    /// # Errors
    ///
    /// Returns an error if decompression fails or an I/O error occurs.
    pub async fn peek(&mut self) -> Result<Option<Vec<u8>>> {
        match self.receiver.try_recv() {
            Ok(guard) => {
                // Copy data before any operations
                let raw_data = guard.to_vec();
                let data = if self.config.compress {
                    zstd::decode_all(raw_data.as_slice())
                        .map_err(|e| SpoolError::Decompression(e.to_string()))?
                } else {
                    raw_data
                };
                // Don't commit - guard drops and rolls back, keeping item in queue
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

    /// Pop and return the first item from the queue.
    ///
    /// This atomically receives and removes the item.
    ///
    /// # Errors
    ///
    /// Returns an error if decompression fails or an I/O error occurs.
    pub async fn pop_front(&mut self) -> Result<Option<Vec<u8>>> {
        match self.receiver.try_recv() {
            Ok(guard) => {
                // Copy data before any operations
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
                Ok(Some(data))
            }
            Err(yaque::TryRecvError::Io(e)) => Err(SpoolError::Io(e)),
            Err(yaque::TryRecvError::QueueEmpty) => Ok(None),
        }
    }

    /// Receive an item asynchronously, waiting if the queue is empty.
    ///
    /// This is the preferred async method for consuming items.
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

        // Copy data before any operations
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
        Ok(data)
    }

    /// Get the approximate number of items in the queue.
    ///
    /// Note: This is tracked internally and may not be accurate
    /// if the queue was opened with existing data.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Check if the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Clear all items from the queue.
    ///
    /// This removes all files in the queue directory and recreates it.
    ///
    /// # Errors
    ///
    /// Returns an error if an I/O error occurs.
    pub fn clear(&mut self) -> Result<()> {
        // yaque doesn't have a built-in clear, so we manually clear by
        // removing all items
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
    async fn test_debug_format() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test-queue");

        let spool = Spool::create(&path).await.unwrap();
        let debug = format!("{spool:?}");
        assert!(debug.contains("Spool"));
        assert!(debug.contains("test-queue"));
    }
}
