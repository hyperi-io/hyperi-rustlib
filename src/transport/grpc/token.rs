// Project:   hyperi-rustlib
// File:      src/transport/grpc/token.rs
// Purpose:   gRPC commit token abstraction
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use super::super::traits::CommitToken;
use std::fmt;
use std::sync::Arc;

/// Commit token for gRPC transport.
///
/// gRPC has no broker-side persistence, so commit is a no-op.
/// The token provides sequence tracking for application-level ordering.
#[derive(Debug, Clone)]
pub struct GrpcToken {
    /// Local sequence number (monotonically increasing per transport instance).
    pub seq: u64,

    /// Remote peer identifier (if available from gRPC metadata).
    pub source: Option<Arc<str>>,
}

impl GrpcToken {
    /// Create a new token with sequence number.
    #[must_use]
    pub fn new(seq: u64) -> Self {
        Self { seq, source: None }
    }

    /// Create a new token with sequence number and source.
    #[must_use]
    pub fn with_source(seq: u64, source: Arc<str>) -> Self {
        Self {
            seq,
            source: Some(source),
        }
    }
}

impl CommitToken for GrpcToken {}

impl fmt::Display for GrpcToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.source {
            Some(src) => write!(f, "grpc:{}:{}", src, self.seq),
            None => write!(f, "grpc:{}", self.seq),
        }
    }
}
