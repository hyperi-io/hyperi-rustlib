// Project:   hyperi-rustlib
// File:      src/dlq/backend.rs
// Purpose:   DLQ backend trait for pluggable destinations
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Trait for DLQ backends.
//!
//! Implement [`DlqBackend`] to add new DLQ destinations (file, Kafka, S3, etc.).

use async_trait::async_trait;

use super::entry::DlqEntry;
use super::error::DlqError;

/// A pluggable DLQ destination.
///
/// Implementations handle writing failed messages to a specific backend
/// (file, Kafka, S3, etc.). The [`super::Dlq`] orchestrator routes entries
/// to one or more backends based on the configured mode.
#[async_trait]
pub trait DlqBackend: Send + Sync {
    /// Write a single entry to this backend.
    async fn send(&self, entry: &DlqEntry) -> Result<(), DlqError>;

    /// Write a batch of entries to this backend.
    ///
    /// Default implementation iterates and calls [`send`](Self::send) for each entry.
    /// Backends may override for more efficient batch operations.
    async fn send_batch(&self, entries: &[DlqEntry]) -> Result<(), DlqError> {
        for entry in entries {
            self.send(entry).await?;
        }
        Ok(())
    }

    /// Backend name for metrics and logging (e.g. "file", "kafka").
    fn name(&self) -> &'static str;
}
