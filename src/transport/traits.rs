// Project:   hyperi-rustlib
// File:      src/transport/traits.rs
// Purpose:   Transport trait definitions (sender + receiver split)
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

use super::error::TransportResult;
use super::filter::FilteredDlqEntry;
use super::types::{Message, SendResult};
use super::work_batch::{Record, WorkBatch};
use std::fmt::{Debug, Display};
use std::future::Future;

/// Transport-specific token for commit/acknowledgment.
///
/// Each transport provides its own token type capturing what it needs to
/// acknowledge message processing.
pub trait CommitToken: Clone + Send + Sync + Debug + Display + 'static {
    /// Get a string representation for logging/debugging.
    fn as_str(&self) -> String {
        format!("{self}")
    }
}

/// Result of a [`TransportReceiver::recv`] call.
///
/// Carries passing messages AND any filter-routed DLQ entries in one struct, so
/// a caller cannot lose dead-letters by forgetting a separate drain step. The
/// caller routes `dlq_entries` onward via its own DLQ handle.
#[derive(Debug)]
pub struct RecvBatch<T: CommitToken> {
    /// Messages that passed all inbound filters (or had no filter match).
    pub messages: Vec<Message<T>>,
    /// Entries matched by `action: dlq` inbound filters. Caller routes to DLQ.
    pub dlq_entries: Vec<FilteredDlqEntry>,
    /// Commit tokens of messages removed by inbound `drop`/`dlq` filters.
    ///
    /// Handled records that produced no passing message. Carried into
    /// `WorkBatch.commit_tokens` so the block commit advances the source past
    /// them -- otherwise an all-filtered stretch stalls the Kafka offset /
    /// leaks the Redis PEL.
    pub filtered_tokens: Vec<T>,
}

impl<T: CommitToken> RecvBatch<T> {
    /// An empty batch (no messages, no DLQ entries).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            messages: Vec::new(),
            dlq_entries: Vec::new(),
            filtered_tokens: Vec::new(),
        }
    }

    /// A batch of messages with no DLQ entries (e.g. filters disabled).
    #[must_use]
    pub fn from_messages(messages: Vec<Message<T>>) -> Self {
        Self {
            messages,
            dlq_entries: Vec::new(),
            filtered_tokens: Vec::new(),
        }
    }

    /// Whether the batch has no messages AND no filtered-only acks to commit.
    /// (DLQ entries may still be present alongside passing messages.)
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty() && self.filtered_tokens.is_empty()
    }

    /// Number of passing messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }
}

/// Lifecycle and introspection methods shared by senders and receivers.
pub trait TransportBase: Send + Sync {
    /// Shutdown the transport gracefully.
    fn close(&self) -> impl Future<Output = TransportResult<()>> + Send;

    /// Check if the transport is healthy and connected.
    fn is_healthy(&self) -> bool;

    /// Get transport name for logging/metrics.
    fn name(&self) -> &'static str;
}

/// Send-side transport.
///
/// The factory returns `AnySender` (enum dispatch) for runtime selection. All
/// implementations auto-emit `dfe_transport_*` metrics when a `MetricsManager`
/// recorder is installed.
pub trait TransportSender: TransportBase {
    /// Send raw bytes to a key/destination.
    ///
    /// The `key` semantics depend on the transport:
    /// - Kafka: topic name
    /// - gRPC: metadata routing key
    /// - HTTP: URL path suffix or ignored
    /// - File: filename suffix or ignored
    /// - Redis: stream name
    /// - Pipe: ignored (single stdout)
    fn send(&self, key: &str, payload: bytes::Bytes) -> impl Future<Output = SendResult> + Send;

    /// Send a whole block of [`Record`]s in one shot.
    ///
    /// The default sends each record individually via [`send`](Self::send),
    /// using the record's own `key` (empty when `None`) and payload (a refcount
    /// bump, not a copy). Transports with a native batch RPC (e.g. gRPC's
    /// `RouteBatch`) override this. Commit tokens and inline-DLQ entries are NOT
    /// sent -- they are the SENDER's local concern; fire the commit tokens
    /// locally after this returns [`SendResult::Ok`].
    ///
    /// ## At-least-once caveat -- per-record fallback can partially send
    ///
    /// Not atomic. If record `k` of `n` returns a transient non-`Ok`
    /// (`Backpressured`/`Fatal`), records `0..k` are already on the wire and
    /// this returns without unsending them. The caller retries the whole block,
    /// re-delivering the sent prefix (at-least-once -- duplicates, never loss).
    /// A native batch override (gRPC) sends the whole block as one RPC, avoiding
    /// the partial-send window.
    ///
    /// ## Outbound-filter dispositions do NOT abort the batch
    ///
    /// A per-record `FilteredDlq` is the record being HANDLED, not a send
    /// failure: skip it and continue. Returning it would make the caller retry
    /// the whole block forever -- the deterministic filter re-matches the same
    /// record every time (a livelock that stalls the source). `Drop` records
    /// likewise never reach the wire. Only `Backpressured`/`Fatal`
    /// short-circuit.
    fn send_batch(&self, records: &[Record]) -> impl Future<Output = SendResult> + Send {
        async move {
            for record in records {
                let key = record.key.as_deref().unwrap_or("");
                match self.send(key, record.payload.clone()).await {
                    // Sent, dropped (Ok), or suppressed by an outbound dlq
                    // filter -- all handled; keep going, do NOT abort the block.
                    SendResult::Ok | SendResult::FilteredDlq => {}
                    // Transient/fatal transport failure: stop so the caller
                    // retries the unconfirmed remainder of the block.
                    other @ (SendResult::Backpressured | SendResult::Fatal(_)) => return other,
                }
            }
            SendResult::Ok
        }
    }
}

/// Limits for a single byte-aware [`TransportReceiver::recv_limited`] poll.
///
/// A bare [`recv`](TransportReceiver::recv) takes only a RECORD cap, so one poll
/// can build a [`WorkBatch`] arbitrarily larger than any memory budget.
/// `RecvLimits` adds the BYTE bound: the governed driver passes its
/// self-regulation byte budget here so a single recv never retains more than
/// `max_bytes` (plus the one-oversized-record floor) before the sub-block split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecvLimits {
    /// Hard cap on records per poll (`>= 1`). Bounds a tiny-record flood that
    /// stays within the byte budget.
    pub max_records: usize,
    /// Soft cap on the SUM of `payload.len()` per poll. A transport that has
    /// accumulated at least one record stops draining once payload bytes reach
    /// this value (so it retains at most `max_bytes + one oversized record`).
    /// The default impl has no byte-aware drain and bounds by `max_records`
    /// only.
    pub max_bytes: u64,
}

/// Receive-side transport -- generic over commit token type.
///
/// Input stages (receiver, fetcher) use concrete implementations directly for
/// type-safe token handling.
pub trait TransportReceiver: TransportBase {
    /// The token type for this transport.
    type Token: CommitToken;

    /// Receive up to `max` records as one [`WorkBatch`].
    ///
    /// Returns immediately with available records (may be fewer than `max`).
    /// Returns an empty batch if no records are available. The source acks for
    /// the whole block live on [`WorkBatch::commit_tokens`] -- they are decoupled
    /// from `records.len()` so a downstream fan-out cannot disturb them.
    ///
    /// **Filter behaviour:** if the transport has inbound filters configured,
    /// `recv()` removes records matching `action: drop` filters and carries
    /// records matching `action: dlq` filters in [`WorkBatch`]`.dlq_entries`
    /// alongside the passing [`WorkBatch`]`.records`. Route the DLQ entries via
    /// your own DLQ handle -- they cannot be silently lost.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let batch = transport.recv(100).await?;
    /// for entry in batch.dlq_entries {
    ///     dlq.send(DlqEntry::new("filter", entry.reason, entry.payload)).await?;
    /// }
    /// for record in batch.records { /* process */ }
    /// ```
    ///
    /// # Cancel-safety (REQUIRED of implementors)
    ///
    /// The governed run loop polls `recv()` inside a `tokio::select!` and DROPS
    /// the future when shutdown or a ticker wins, so it must not leave records
    /// half-consumed at an `.await` -- either gather records synchronously (no
    /// `.await` between taking a record off the wire and returning it) or buffer
    /// internally. The in-tree Kafka (synchronous poll) and memory (awaits only
    /// on an empty buffer) impls satisfy this; a custom impl that holds records
    /// across an `.await` will drop data on cancellation.
    fn recv(
        &self,
        max: usize,
    ) -> impl Future<Output = TransportResult<WorkBatch<Self::Token>>> + Send;

    /// Byte-aware receive: bound a single poll by BOTH a record cap and a
    /// payload-byte cap (see [`RecvLimits`]).
    ///
    /// The governed driver uses this so the self-regulation byte budget bounds
    /// RECEIVE memory, not just the post-recv sub-block lease: a poll retains at
    /// most `limits.max_bytes` of payload (plus one oversized record), so the
    /// inbound footprint is bounded BEFORE the sub-block split, never after.
    ///
    /// **Default impl:** falls back to [`recv`](Self::recv)`(limits.max_records)`
    /// -- record-bounded only, byte cap ignored. Only transports that buffer a
    /// whole poll's bytes in one allocation (Kafka's recv-arena) override it.
    /// Channel/stream transports (Memory, gRPC, ...) already retain only one
    /// record's bytes at a time, so the fallback is correct for them.
    ///
    /// Filter behaviour and the `commit_tokens` contract match
    /// [`recv`](Self::recv).
    fn recv_limited(
        &self,
        limits: RecvLimits,
    ) -> impl Future<Output = TransportResult<WorkBatch<Self::Token>>> + Send {
        self.recv(limits.max_records)
    }

    /// Commit/acknowledge processed messages.
    ///
    /// - Kafka: commits consumer offsets
    /// - gRPC: no-op (no persistence)
    /// - Redis: XACK
    /// - File: advances read position
    /// - Memory: advances internal sequence
    fn commit(&self, tokens: &[Self::Token]) -> impl Future<Output = TransportResult<()>> + Send;
}

/// Combined transport -- implements both send and receive.
///
/// Most concrete impls (Kafka, gRPC, Memory, Redis, File, Pipe) qualify;
/// auto-implemented via blanket impl.
pub trait Transport: TransportSender + TransportReceiver {}

/// Blanket impl: anything that implements both traits is a Transport.
impl<T: TransportSender + TransportReceiver> Transport for T {}

/// Load a transport config from the cascade under a fixed key.
///
/// Consolidates the byte-identical `from_cascade()` bodies each transport config
/// used to repeat. Implementors only name their key. Without the `config`
/// feature the default method returns `Default::default()`.
pub trait FromCascade: Default + serde::Serialize + serde::de::DeserializeOwned + 'static {
    /// Load `Self` from the config cascade under `key`, registering the section
    /// in the global registry; falls back to `Default` if the cascade is
    /// unavailable or the key is absent/invalid.
    #[must_use]
    fn from_cascade_key(key: &str) -> Self {
        #[cfg(feature = "config")]
        {
            if let Some(cfg) = crate::config::try_get()
                && let Ok(value) = cfg.unmarshal_key_registered::<Self>(key)
            {
                return value;
            }
        }
        // Without `config`, or on any cascade miss, use defaults.
        #[cfg(not(feature = "config"))]
        let _ = key;
        Self::default()
    }
}
