// Project:   hs-rustlib
// File:      src/transport/memory/token.rs
// Purpose:   Memory transport commit token
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

use crate::transport::traits::CommitToken;

/// Commit token for memory transport.
///
/// Contains a sequence number that can be used to track
/// which messages have been processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MemoryToken {
    /// Message sequence number.
    pub seq: u64,
}

impl CommitToken for MemoryToken {}

impl std::fmt::Display for MemoryToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "memory:{}", self.seq)
    }
}
