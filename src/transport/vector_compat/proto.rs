// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED

//! Generated protobuf types for Vector wire protocol compatibility.
//!
//! Vendored from <https://github.com/vectordotdev/vector> (v2 gRPC protocol).
//! - `event` module: EventWrapper, Log, Value, Metric, Trace, etc.
//! - `vector` module: PushEvents service, PushEventsRequest/Response.

/// Event types from Vector's `event.proto` (package `event`).
#[allow(clippy::all, clippy::pedantic, deprecated)]
pub mod event {
    include!(concat!(env!("OUT_DIR"), "/event.rs"));
}

/// Vector gRPC service from `vector.proto` (package `vector`).
/// References event types via extern_path mapping in build.rs.
#[allow(clippy::all, clippy::pedantic)]
pub mod vector {
    include!(concat!(env!("OUT_DIR"), "/vector.rs"));
}
