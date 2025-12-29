// Project:   hs-rustlib
// File:      src/transport/zenoh/token.rs
// Purpose:   Zenoh transport commit token
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

use crate::transport::traits::CommitToken;
use std::sync::Arc;

/// Commit token for Zenoh transport.
///
/// Contains key expression and optional timestamp for tracking.
/// Note: Zenoh has no persistence, so "commit" is a no-op for durability.
#[derive(Debug, Clone)]
pub struct ZenohToken {
    /// Key expression the message was published on.
    pub key_expr: Arc<str>,
    /// Zenoh HLC timestamp (if available).
    pub timestamp: Option<u64>,
    /// Local sequence number for ordering.
    pub seq: u64,
}

impl ZenohToken {
    /// Create a new Zenoh token.
    #[must_use]
    pub fn new(key_expr: Arc<str>, timestamp: Option<u64>, seq: u64) -> Self {
        Self {
            key_expr,
            timestamp,
            seq,
        }
    }
}

impl CommitToken for ZenohToken {}

impl std::fmt::Display for ZenohToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.timestamp {
            Some(ts) => write!(f, "zenoh:{}:{}:{}", self.key_expr, ts, self.seq),
            None => write!(f, "zenoh:{}::{}", self.key_expr, self.seq),
        }
    }
}

impl PartialEq for ZenohToken {
    fn eq(&self, other: &Self) -> bool {
        self.key_expr == other.key_expr && self.seq == other.seq
    }
}

impl Eq for ZenohToken {}

impl std::hash::Hash for ZenohToken {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.key_expr.hash(state);
        self.seq.hash(state);
    }
}
