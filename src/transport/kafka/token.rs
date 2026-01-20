// Project:   hs-rustlib
// File:      src/transport/kafka/token.rs
// Purpose:   Kafka transport commit token
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

use crate::transport::traits::CommitToken;
use std::sync::Arc;

/// Commit token for Kafka transport.
///
/// Contains topic, partition, and offset for consumer group commits.
#[derive(Debug, Clone)]
pub struct KafkaToken {
    /// Topic name (shared Arc for efficiency).
    pub topic: Arc<str>,
    /// Partition number.
    pub partition: i32,
    /// Message offset.
    pub offset: i64,
}

impl KafkaToken {
    /// Create a new Kafka token.
    #[must_use]
    pub fn new(topic: Arc<str>, partition: i32, offset: i64) -> Self {
        Self {
            topic,
            partition,
            offset,
        }
    }
}

impl CommitToken for KafkaToken {}

impl std::fmt::Display for KafkaToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "kafka:{}:{}:{}", self.topic, self.partition, self.offset)
    }
}

impl PartialEq for KafkaToken {
    fn eq(&self, other: &Self) -> bool {
        self.topic == other.topic
            && self.partition == other.partition
            && self.offset == other.offset
    }
}

impl Eq for KafkaToken {}

impl std::hash::Hash for KafkaToken {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.topic.hash(state);
        self.partition.hash(state);
        self.offset.hash(state);
    }
}
