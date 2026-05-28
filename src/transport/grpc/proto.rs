// Project:   hyperi-rustlib
// File:      src/transport/grpc/proto.rs
// Purpose:   gRPC protobuf bindings
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Generated protobuf types for DFE native gRPC transport.

#[allow(clippy::all, clippy::pedantic)]
mod inner {
    tonic::include_proto!("dfe.transport.v1");
}

pub use inner::*;
