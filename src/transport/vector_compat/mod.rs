// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED

//! # Vector Wire Protocol Compatibility
//!
//! Provides source (server) and sink (client) implementations compatible
//! with Vector's v2 gRPC protocol (`vector.Vector/PushEvents`).
//!
//! This enables seamless migration from Vector-based pipelines to DFE native:
//!
//! ```text
//! Phase 1: vector-receiver → vector-sink → [DFE loader + vector-compat]
//! Phase 2: [DFE receiver] → DFE gRPC → [DFE loader, native proto]
//! Phase 3: Disable vector-compat (pure DFE pipeline)
//! ```
//!
//! ## Proto files
//!
//! Vendored from <https://github.com/vectordotdev/vector> (pinned 2026-03-02):
//! - `proto/vector/vector.proto` — PushEvents service definition
//! - `proto/vector/event.proto` — EventWrapper, Log, Value, Metric, Trace
//!
//! ## Feature flag
//!
//! Requires `transport-grpc-vector-compat` (implies `transport-grpc`).

pub mod convert;
pub mod proto;
pub mod sink;
pub mod source;

pub use convert::{event_wrapper_to_json, json_to_event_wrapper};
pub use sink::VectorCompatClient;
pub use source::VectorCompatService;
