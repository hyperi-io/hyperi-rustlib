// Project:   hyperi-rustlib
// File:      src/transport/grpc/batch.rs
// Purpose:   Native batch transport -- WorkBatch <-> proto Batch wire mapper
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # Native batch transport wire mapper (Task 0.6)
//!
//! Serde-less rustlib<->rustlib (DFE<->DFE) transfer of a whole
//! [`WorkBatch`](crate::transport::WorkBatch) over the existing gRPC mesh. One
//! `WorkBatch` maps to one proto [`Batch`](super::proto::Batch) and travels in a
//! single `RouteBatch` RPC -- batch-at-a-time, NOT record-by-record streaming.
//!
//! ## Payloads are OPAQUE in transit
//!
//! Each [`Record`] payload is mapped straight onto the
//! proto `Record.payload` `bytes` field and back. prost is configured (in
//! `build.rs`, `.bytes(".")`) to decode that field ZERO-COPY into
//! [`bytes::Bytes`], so on the receive side the payload is a refcounted view of
//! the decode buffer, never copied. The JSON / MsgPack codec
//! ([`crate::transport::codec`]) is NEVER invoked here -- the bytes pass through
//! untouched, so a payload that is not valid JSON or MsgPack survives a
//! round-trip byte-identical.
//!
//! ## Swappable wire
//!
//! The wire is protobuf BY DEFAULT but the mapping is isolated to this module:
//! a future hand-rolled frame could replace [`records_to_proto`] /
//! [`proto_batch_to_records`] without touching the `WorkBatch` types or the
//! transport seam.
//!
//! ## What does NOT cross the wire
//!
//! `commit_tokens` and `dlq_entries` are deliberately left off the proto `Batch`.
//! Commit tokens are the SENDER's source acks -- fired locally after the block
//! is sent (at-least-once), they have no meaning on the receiver. Inline-DLQ
//! entries are a local no-silent-drop concern. So the mapper takes / returns the
//! records (`&[Record]` / `Vec<Record>`), not the whole `WorkBatch<T>`; this
//! also avoids dragging the `CommitToken` generic onto the wire path.

use super::proto;
use crate::transport::types::PayloadFormat;
use crate::transport::work_batch::{Record, RecordMeta};
use bytes::Bytes;
use std::sync::Arc;

/// Map a rustlib [`PayloadFormat`] onto the proto [`Format`](proto::Format).
fn format_to_proto(format: PayloadFormat) -> proto::Format {
    match format {
        PayloadFormat::Auto => proto::Format::Auto,
        PayloadFormat::Json => proto::Format::Json,
        PayloadFormat::MsgPack => proto::Format::Msgpack,
    }
}

/// Map a proto [`Format`](proto::Format) back onto a rustlib [`PayloadFormat`].
///
/// `FORMAT_ARROW_IPC` has no rustlib equivalent yet, so it collapses to
/// [`PayloadFormat::Auto`] (the safe "detect later" default). An out-of-range
/// enum value (forward-compat from a newer peer) likewise maps to `Auto`.
fn format_from_proto(format: i32) -> PayloadFormat {
    match proto::Format::try_from(format) {
        Ok(proto::Format::Json) => PayloadFormat::Json,
        Ok(proto::Format::Msgpack) => PayloadFormat::MsgPack,
        // FORMAT_AUTO, FORMAT_ARROW_IPC, or an unknown future value.
        _ => PayloadFormat::Auto,
    }
}

/// Map one rustlib [`Record`] onto a proto [`Record`](proto::Record).
///
/// The payload `Bytes` handle is MOVED onto the proto field (no copy). `key`
/// `None` becomes the empty string; `Some(k)` carries the key text.
fn record_to_proto(record: Record) -> proto::Record {
    let (timestamp_ms, has_timestamp_ms) = match record.metadata.timestamp_ms {
        Some(ts) => (ts, true),
        None => (0, false),
    };

    proto::Record {
        payload: record.payload,
        key: record.key.as_deref().unwrap_or("").to_string(),
        headers: record
            .headers
            .into_iter()
            .map(|(key, value)| proto::Header {
                key,
                value: Bytes::from(value),
            })
            .collect(),
        timestamp_ms,
        has_timestamp_ms,
        format: format_to_proto(record.metadata.format).into(),
    }
}

/// Map a proto [`Record`](proto::Record) back onto a rustlib [`Record`].
///
/// The payload is the prost-decoded [`Bytes`] handle MOVED across (zero-copy --
/// a refcounted view of the decode buffer). An empty `key` string maps back to
/// `None`.
fn record_from_proto(record: proto::Record) -> Record {
    let key = if record.key.is_empty() {
        None
    } else {
        Some(Arc::from(record.key.as_str()))
    };

    Record {
        payload: record.payload,
        key,
        headers: record
            .headers
            .into_iter()
            .map(|h| (h.key, h.value.to_vec()))
            .collect(),
        metadata: RecordMeta {
            timestamp_ms: if record.has_timestamp_ms {
                Some(record.timestamp_ms)
            } else {
                None
            },
            format: format_from_proto(record.format),
        },
    }
}

/// Map a slice of rustlib [`Record`]s onto a proto [`Batch`](proto::Batch).
///
/// This is the SEND-side mapper. It takes the records (NOT the whole
/// `WorkBatch<T>`) because commit tokens and DLQ entries do not cross the wire
/// (see the module docs). Payloads are moved, not copied.
#[must_use]
pub fn records_to_proto(records: Vec<Record>) -> proto::Batch {
    proto::Batch {
        records: records.into_iter().map(record_to_proto).collect(),
    }
}

/// Map a proto [`Batch`](proto::Batch) back onto a `Vec<Record>`.
///
/// This is the RECEIVE-side mapper. The caller wraps the returned records in a
/// fresh [`WorkBatch`](crate::transport::WorkBatch) (attaching its own commit
/// tokens). Payloads are zero-copy [`Bytes`] views of the decode buffer.
#[must_use]
pub fn proto_batch_to_records(batch: proto::Batch) -> Vec<Record> {
    batch.records.into_iter().map(record_from_proto).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a record with all fields populated (incl. non-UTF8 payload).
    fn full_record() -> Record {
        Record {
            // Deliberately NOT valid JSON or MsgPack, and includes non-UTF8
            // bytes -- proves no codec runs on the payload in transit.
            payload: Bytes::from_static(&[0x00, 0xff, 0xfe, b'{', 0x80, b'a']),
            key: Some(Arc::from("events.land")),
            headers: vec![
                ("trace".to_string(), vec![0x01, 0x02, 0x03]),
                ("source".to_string(), b"loader".to_vec()),
            ],
            metadata: RecordMeta {
                timestamp_ms: Some(1_717_000_000_123),
                format: PayloadFormat::Json,
            },
        }
    }

    /// Round-trip ONE record through proto and back, asserting byte-equality on
    /// every field. `proto::Record::payload` must be `Bytes` (typed below).
    #[test]
    fn record_round_trips_byte_identical() {
        let original = full_record();
        let p = record_to_proto(original.clone());

        // The proto payload field is `bytes::Bytes` (zero-copy config). This
        // line would not compile if prost emitted `Vec<u8>` here.
        let _: &Bytes = &p.payload;

        let back = record_from_proto(p);
        assert_eq!(back, original);
    }

    /// Whole-batch round-trip with several records, including binary payloads
    /// with non-UTF8 bytes, keys, headers, and timestamps. Asserts every field
    /// is byte-identical after a real prost encode/decode cycle.
    #[test]
    fn batch_round_trips_through_prost_encode_decode() {
        use prost::Message as _;

        let records = vec![
            full_record(),
            Record {
                payload: Bytes::from_static(b"plain text not json"),
                key: None,
                headers: Vec::new(),
                metadata: RecordMeta {
                    timestamp_ms: None,
                    format: PayloadFormat::Auto,
                },
            },
            Record {
                // MsgPack fixmap byte lead.
                payload: Bytes::from_static(&[0x81, 0xa1, b'a', 0x01]),
                key: Some(Arc::from("metrics")),
                headers: vec![("k".to_string(), Vec::new())],
                metadata: RecordMeta {
                    timestamp_ms: Some(0), // genuine zero, must survive as Some(0)
                    format: PayloadFormat::MsgPack,
                },
            },
        ];

        // Map -> encode (prost) -> decode (prost) -> map back.
        let proto_batch = records_to_proto(records.clone());
        let encoded = proto_batch.encode_to_vec();
        let decoded = proto::Batch::decode(Bytes::from(encoded)).expect("prost decode");
        let back = proto_batch_to_records(decoded);

        assert_eq!(back.len(), records.len());
        for (a, b) in back.iter().zip(records.iter()) {
            assert_eq!(a, b, "record mismatch after wire round-trip");
        }
        // The genuine zero timestamp survived as Some(0), not None.
        assert_eq!(back[2].metadata.timestamp_ms, Some(0));
    }

    /// A payload that is NEITHER valid JSON NOR valid MsgPack survives the wire
    /// round-trip byte-for-byte. If any codec ran on the payload in transit it
    /// would fail to parse or mutate the bytes -- proving the payload is opaque.
    #[test]
    fn non_codec_payload_survives_round_trip_opaque() {
        use prost::Message as _;

        // Random binary: not parseable as JSON or MsgPack.
        let raw: Vec<u8> = (0u8..=255).cycle().take(777).collect();
        let payload = Bytes::from(raw.clone());

        let records = vec![Record {
            payload,
            key: Some(Arc::from("k")),
            headers: Vec::new(),
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Auto,
            },
        }];

        let encoded = records_to_proto(records).encode_to_vec();
        let decoded = proto::Batch::decode(Bytes::from(encoded)).expect("prost decode");
        let back = proto_batch_to_records(decoded);

        assert_eq!(back.len(), 1);
        assert_eq!(back[0].payload.as_ref(), raw.as_slice());
    }

    /// An empty batch round-trips to an empty batch.
    #[test]
    fn empty_batch_round_trips() {
        use prost::Message as _;

        let encoded = records_to_proto(Vec::new()).encode_to_vec();
        let decoded = proto::Batch::decode(Bytes::from(encoded)).expect("prost decode");
        let back = proto_batch_to_records(decoded);
        assert!(back.is_empty());
    }

    /// A large-ish batch (1000 small records) round-trips intact.
    #[test]
    fn large_batch_round_trips() {
        use prost::Message as _;

        let records: Vec<Record> = (0..1000u32)
            .map(|i| Record {
                payload: Bytes::from(format!("{{\"i\":{i}}}").into_bytes()),
                key: Some(Arc::from("bulk")),
                headers: vec![("seq".to_string(), i.to_le_bytes().to_vec())],
                metadata: RecordMeta {
                    timestamp_ms: Some(i64::from(i)),
                    format: PayloadFormat::Json,
                },
            })
            .collect();

        let encoded = records_to_proto(records.clone()).encode_to_vec();
        let decoded = proto::Batch::decode(Bytes::from(encoded)).expect("prost decode");
        let back = proto_batch_to_records(decoded);

        assert_eq!(back.len(), 1000);
        assert_eq!(back, records);
    }

    /// The proto `Record.payload` field is `bytes::Bytes` (not `Vec<u8>`).
    /// This is the build.rs `.bytes(".")` config taking effect; a regression to
    /// the default `Vec<u8>` would fail to compile this assignment.
    #[test]
    fn proto_payload_field_is_bytes_type() {
        let r = proto::Record {
            payload: Bytes::from_static(b"x"),
            ..Default::default()
        };
        let p: Bytes = r.payload;
        assert_eq!(p.as_ref(), b"x");
    }

    /// Empty key string maps to `None`, and `None` maps back to empty string.
    #[test]
    fn empty_key_maps_to_none() {
        let r = Record {
            payload: Bytes::from_static(b"x"),
            key: None,
            headers: Vec::new(),
            metadata: RecordMeta {
                timestamp_ms: None,
                format: PayloadFormat::Auto,
            },
        };
        let p = record_to_proto(r);
        assert_eq!(p.key, "");
        let back = record_from_proto(p);
        assert_eq!(back.key, None);
    }

    /// FORMAT_ARROW_IPC (no rustlib equivalent) maps back to `Auto`.
    #[test]
    fn arrow_format_collapses_to_auto() {
        let p = proto::Record {
            payload: Bytes::from_static(b"x"),
            format: proto::Format::ArrowIpc.into(),
            ..Default::default()
        };
        let back = record_from_proto(p);
        assert_eq!(back.metadata.format, PayloadFormat::Auto);
    }
}
