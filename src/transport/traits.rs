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
/// Each transport implementation provides its own token type that
/// captures the information needed to acknowledge message processing.
pub trait CommitToken: Clone + Send + Sync + Debug + Display + 'static {
    /// Get a string representation for logging/debugging.
    fn as_str(&self) -> String {
        format!("{self}")
    }
}

/// Result of a [`TransportReceiver::recv`] call.
///
/// Carries the messages that passed inbound filtering AND any entries those
/// filters routed to DLQ, so a caller cannot accidentally lose dead-letters by
/// forgetting a separate drain step (the previous two-call
/// `recv()` + `take_filtered_dlq_entries()` contract). The caller routes
/// `dlq_entries` onward via its own DLQ handle.
#[derive(Debug)]
pub struct RecvBatch<T: CommitToken> {
    /// Messages that passed all inbound filters (or had no filter match).
    pub messages: Vec<Message<T>>,
    /// Entries matched by `action: dlq` inbound filters. Caller routes to DLQ.
    pub dlq_entries: Vec<FilteredDlqEntry>,
}

impl<T: CommitToken> RecvBatch<T> {
    /// An empty batch (no messages, no DLQ entries).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            messages: Vec::new(),
            dlq_entries: Vec::new(),
        }
    }

    /// A batch of messages with no DLQ entries (e.g. filters disabled).
    #[must_use]
    pub fn from_messages(messages: Vec<Message<T>>) -> Self {
        Self {
            messages,
            dlq_entries: Vec::new(),
        }
    }

    /// Whether the batch has no messages (DLQ entries may still be present).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Number of passing messages.
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }
}

/// Common transport operations shared by senders and receivers.
///
/// Every transport implementation provides these lifecycle and
/// introspection methods regardless of direction.
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
/// Extends `TransportBase` with send capability. The factory returns
/// `AnySender` (enum dispatch) for runtime transport selection.
///
/// All implementations auto-emit `dfe_transport_*` Prometheus metrics
/// when a `MetricsManager` recorder is installed.
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
    /// using the record's own `key` (empty string when `None`) and its payload
    /// `Bytes` (a refcount bump, not a copy). Transports with a native batch RPC
    /// (e.g. gRPC's `RouteBatch`) override this to send the whole block in a
    /// single call -- serde-less and round-trip-cheaper.
    ///
    /// Commit tokens and inline-DLQ entries are NOT sent: they are the SENDER's
    /// local concern. Pass the records (e.g. `&workbatch.records`) and fire the
    /// commit tokens locally after this returns [`SendResult::Ok`].
    ///
    /// ## At-least-once caveat -- per-record fallback can partially send
    ///
    /// The default is NOT atomic. If record `k` of `n` returns a non-`Ok`
    /// result, records `0..k` are already on the wire and this returns that
    /// first non-`Ok` result WITHOUT unsending them. The caller treats the whole
    /// block as not-yet-committed and retries it; the already-sent prefix is
    /// re-delivered (at-least-once -- duplicates, never loss). A `Backpressured`
    /// or `Fatal` short-circuits (no further records are attempted) so the caller
    /// retries the remainder rather than skipping past a transient failure. A
    /// native batch override (gRPC) avoids the partial-send window entirely: the
    /// whole block is one RPC, accepted or not.
    fn send_batch(&self, records: &[Record]) -> impl Future<Output = SendResult> + Send {
        async move {
            for record in records {
                let key = record.key.as_deref().unwrap_or("");
                let result = self.send(key, record.payload.clone()).await;
                if !result.is_ok() {
                    // Backpressured / Fatal / FilteredDlq -- stop here so the
                    // caller retries the (unconfirmed) remainder of the block.
                    return result;
                }
            }
            SendResult::Ok
        }
    }
}

/// Limits for a single byte-aware [`TransportReceiver::recv_limited`] poll.
///
/// A bare [`recv`](TransportReceiver::recv) takes only a RECORD cap, so a single
/// poll can build a [`WorkBatch`] whose total bytes are arbitrarily larger than
/// any memory budget (and, for the Kafka recv-arena, allocate one arena for the
/// whole poll). `RecvLimits` adds the missing BYTE bound: the governed driver
/// passes the self-regulation byte budget here so a single governed recv never
/// retains more than `max_bytes` (plus the one-oversized-record floor) of
/// inbound payload before the sub-block split runs.
///
/// `max_records` is the existing poll-safety record cap (a tiny-record flood
/// cannot blow the count even within the byte budget). `max_bytes` is the new
/// payload-bytes ceiling for the poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecvLimits {
    /// Hard cap on the number of records returned by one poll (`>= 1`).
    pub max_records: usize,
    /// Soft cap on the SUM of `payload.len()` returned by one poll. A transport
    /// that has accumulated at least one record stops draining once the
    /// accumulated payload bytes reach this value, so the poll never retains
    /// more than `max_bytes + one oversized record`. A transport with no
    /// byte-aware drain (the default impl) ignores this and bounds by
    /// `max_records` only.
    pub max_bytes: u64,
}

/// Receive-side transport -- generic over commit token type.
///
/// Extends `TransportBase` with receive and commit capability.
/// Input stages (receiver, fetcher) use concrete implementations
/// directly for type-safe token handling.
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
    fn recv(
        &self,
        max: usize,
    ) -> impl Future<Output = TransportResult<WorkBatch<Self::Token>>> + Send;

    /// Byte-aware receive: bound a single poll by BOTH a record cap and a
    /// payload-byte cap (see [`RecvLimits`]).
    ///
    /// This is the receive limit the governed driver uses so the
    /// self-regulation byte budget actually constrains RECEIVE memory -- not
    /// just the post-recv sub-block lease. A single poll retains at most
    /// `limits.max_bytes` of payload (plus one oversized record under the
    /// floor), so the in-flight inbound footprint is bounded BEFORE the
    /// sub-block split, never after.
    ///
    /// **Default impl:** falls back to [`recv`](Self::recv)`(limits.max_records)`
    /// -- record-bounded only, byte cap ignored. This keeps every transport
    /// green with no churn; only transports that buffer a whole poll's bytes in
    /// one allocation (Kafka's recv-arena) override it to honour `max_bytes`.
    /// Channel/stream transports (Memory, gRPC, ...) already retain only one
    /// record's bytes at a time, so the record-bounded fallback is correct for
    /// them.
    ///
    /// Filter behaviour and the `commit_tokens` contract are identical to
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
/// Convenience trait for transports that support bidirectional communication.
/// Most concrete implementations (Kafka, gRPC, Memory, Redis, File, Pipe)
/// implement this. Automatically implemented via blanket impl.
pub trait Transport: TransportSender + TransportReceiver {}

/// Blanket impl: anything that implements both traits is a Transport.
impl<T: TransportSender + TransportReceiver> Transport for T {}

/// Load a transport config from the cascade under a fixed key.
///
/// Consolidates the byte-identical `from_cascade()` bodies that each transport
/// config previously repeated (try the cascade under a key, register the
/// section, fall back to `Default`). Implementors only name their key; the
/// loading logic lives here once. Without the `config` feature the default
/// method just returns `Default::default()`.
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
