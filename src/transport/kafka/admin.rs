// Project:   hyperi-rustlib
// File:      src/transport/kafka/admin.rs
// Purpose:   Kafka administrative operations
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Kafka administrative operations.
//!
//! `KafkaAdmin` provides programmatic access to managing consumer group offsets,
//! topic configuration, and partition management. Matches the Python
//! `hs_pylib.kafka.KafkaAdmin` API.
//!
//! ## Example
//!
//! ```rust,ignore
//! use hyperi_rustlib::transport::kafka::{KafkaAdmin, KafkaConfig};
//!
//! let config = KafkaConfig::default();
//! let admin = KafkaAdmin::new(&config)?;
//!
//! // Reset consumer group to earliest
//! admin.reset_offsets_to_earliest("my-group", "events", None).await?;
//!
//! // Get consumer lag
//! let lag = admin.get_consumer_lag("my-group", "events").await?;
//! for (partition, lag) in lag {
//!     println!("Partition {}: lag {}", partition, lag);
//! }
//! ```

use super::config::KafkaConfig;
use crate::transport::error::{TransportError, TransportResult};
use rdkafka::admin::{
    AdminClient, AdminOptions, AlterConfig, NewPartitions, NewTopic, ResourceSpecifier,
    TopicReplication,
};
use rdkafka::client::DefaultClientContext;
use rdkafka::config::ClientConfig;
use rdkafka::consumer::{BaseConsumer, Consumer};
use rdkafka::metadata::MetadataTopic;
use rdkafka::topic_partition_list::{Offset, TopicPartitionList};
use std::collections::HashMap;
use std::time::Duration;

/// Information about a topic.
#[derive(Debug, Clone)]
pub struct TopicInfo {
    /// Topic name.
    pub name: String,
    /// Number of partitions.
    pub partition_count: i32,
    /// Replication factor (from first partition's ISR).
    pub replication_factor: i32,
}

/// Kafka administrative client.
///
/// Provides operations for managing consumer group offsets, topic configuration,
/// and partition scaling. Designed to match the Python `KafkaAdmin` API.
pub struct KafkaAdmin {
    admin: AdminClient<DefaultClientContext>,
    consumer: BaseConsumer,
    config: ClientConfig,
}

impl KafkaAdmin {
    /// Create a new Kafka admin client.
    ///
    /// # Errors
    ///
    /// Returns error if admin client creation fails.
    pub fn new(config: &KafkaConfig) -> TransportResult<Self> {
        let mut client_config = ClientConfig::new();

        // Required settings
        client_config.set("bootstrap.servers", config.brokers.join(","));
        client_config.set("client.id", &config.client_id);

        // Security settings
        client_config.set("security.protocol", &config.security_protocol);
        if let Some(ref mechanism) = config.sasl_mechanism {
            client_config.set("sasl.mechanism", mechanism);
        }
        if let Some(ref username) = config.sasl_username {
            client_config.set("sasl.username", username);
        }
        if let Some(ref password) = config.sasl_password {
            client_config.set("sasl.password", password);
        }

        // TLS settings
        if let Some(ref ca) = config.ssl_ca_location {
            client_config.set("ssl.ca.location", ca);
        }
        if let Some(ref cert) = config.ssl_certificate_location {
            client_config.set("ssl.certificate.location", cert);
        }
        if let Some(ref key) = config.ssl_key_location {
            client_config.set("ssl.key.location", key);
        }
        if config.ssl_skip_verify {
            client_config.set("enable.ssl.certificate.verification", "false");
        }

        // Apply profile defaults and user overrides
        let rdkafka_config = config.build_librdkafka_config();
        for (key, value) in &rdkafka_config {
            client_config.set(key, value);
        }

        // Create admin client
        let admin: AdminClient<DefaultClientContext> = client_config.create().map_err(|e| {
            TransportError::Connection(format!("Failed to create admin client: {e}"))
        })?;

        // Create consumer for offset queries (group.id required but can be arbitrary)
        let mut consumer_config = client_config.clone();
        consumer_config.set("group.id", "__hs_admin_internal");
        let consumer: BaseConsumer = consumer_config
            .create()
            .map_err(|e| TransportError::Connection(format!("Failed to create consumer: {e}")))?;

        Ok(Self {
            admin,
            consumer,
            config: client_config,
        })
    }

    // --- Consumer Group Offset Management ---

    /// Reset consumer group offsets to earliest (reprocess all messages).
    ///
    /// The consumer group must be stopped (no active consumers) before resetting.
    ///
    /// # Arguments
    ///
    /// * `group_id` - Consumer group ID
    /// * `topic` - Topic name
    /// * `partitions` - Specific partitions to reset, or None for all
    ///
    /// # Errors
    ///
    /// Returns error if offset reset fails.
    pub async fn reset_offsets_to_earliest(
        &self,
        group_id: &str,
        topic: &str,
        partitions: Option<&[i32]>,
    ) -> TransportResult<()> {
        let partition_list: TopicPartitionList = self.get_partition_list(topic, partitions).await?;

        // Set all offsets to Beginning
        let mut tpl = TopicPartitionList::new();
        for elem in partition_list.elements() {
            tpl.add_partition_offset(elem.topic(), elem.partition(), Offset::Beginning)
                .map_err(|e| TransportError::Admin(format!("Failed to build TPL: {e}")))?;
        }

        self.commit_offsets_for_group(group_id, &tpl).await
    }

    /// Reset consumer group offsets to latest (skip to end).
    ///
    /// # Arguments
    ///
    /// * `group_id` - Consumer group ID
    /// * `topic` - Topic name
    /// * `partitions` - Specific partitions to reset, or None for all
    ///
    /// # Errors
    ///
    /// Returns error if offset reset fails.
    pub async fn reset_offsets_to_latest(
        &self,
        group_id: &str,
        topic: &str,
        partitions: Option<&[i32]>,
    ) -> TransportResult<()> {
        let partition_list: TopicPartitionList = self.get_partition_list(topic, partitions).await?;

        // Get high watermarks for each partition
        let mut tpl = TopicPartitionList::new();
        for elem in partition_list.elements() {
            let (_, high) = self
                .consumer
                .fetch_watermarks(topic, elem.partition(), Duration::from_secs(10))
                .map_err(|e| {
                    TransportError::Admin(format!(
                        "Failed to fetch watermarks for partition {}: {e}",
                        elem.partition()
                    ))
                })?;

            tpl.add_partition_offset(elem.topic(), elem.partition(), Offset::Offset(high))
                .map_err(|e| TransportError::Admin(format!("Failed to build TPL: {e}")))?;
        }

        self.commit_offsets_for_group(group_id, &tpl).await
    }

    /// Reset consumer group offsets to a specific timestamp.
    ///
    /// Offsets are set to the first message at or after the specified timestamp.
    ///
    /// # Arguments
    ///
    /// * `group_id` - Consumer group ID
    /// * `topic` - Topic name
    /// * `timestamp_ms` - Unix timestamp in milliseconds
    /// * `partitions` - Specific partitions to reset, or None for all
    ///
    /// # Errors
    ///
    /// Returns error if offset reset fails.
    pub async fn reset_offsets_to_timestamp(
        &self,
        group_id: &str,
        topic: &str,
        timestamp_ms: i64,
        partitions: Option<&[i32]>,
    ) -> TransportResult<()> {
        let partition_list: TopicPartitionList = self.get_partition_list(topic, partitions).await?;

        // Build TPL with timestamps
        let mut tpl = TopicPartitionList::new();
        for elem in partition_list.elements() {
            tpl.add_partition_offset(elem.topic(), elem.partition(), Offset::Offset(timestamp_ms))
                .map_err(|e| TransportError::Admin(format!("Failed to build TPL: {e}")))?;
        }

        // Get offsets for timestamps
        let offsets = self
            .consumer
            .offsets_for_times(tpl, Duration::from_secs(30))
            .map_err(|e| TransportError::Admin(format!("Failed to get offsets for times: {e}")))?;

        self.commit_offsets_for_group(group_id, &offsets).await
    }

    /// Get consumer lag per partition.
    ///
    /// Lag is the difference between the high watermark and the committed offset.
    ///
    /// # Returns
    ///
    /// Map of partition ID to lag (messages behind).
    ///
    /// # Errors
    ///
    /// Returns error if lag calculation fails.
    pub async fn get_consumer_lag(
        &self,
        group_id: &str,
        topic: &str,
    ) -> TransportResult<HashMap<i32, i64>> {
        // Create a consumer with the target group to query committed offsets
        let mut group_config = self.config.clone();
        group_config.set("group.id", group_id);
        let group_consumer: BaseConsumer = group_config
            .create()
            .map_err(|e| TransportError::Connection(format!("Failed to create consumer: {e}")))?;

        // Get topic metadata to find partitions
        let metadata = self
            .consumer
            .fetch_metadata(Some(topic), Duration::from_secs(10))
            .map_err(|e| TransportError::Admin(format!("Failed to fetch metadata: {e}")))?;

        let topic_meta: &MetadataTopic = metadata
            .topics()
            .iter()
            .find(|t| t.name() == topic)
            .ok_or_else(|| TransportError::Admin(format!("Topic {topic} not found")))?;

        // Build TPL for committed offset query
        let mut tpl = TopicPartitionList::new();
        for partition in topic_meta.partitions() {
            tpl.add_partition(topic, partition.id());
        }

        // Get committed offsets
        let committed = group_consumer
            .committed_offsets(tpl, Duration::from_secs(10))
            .map_err(|e| TransportError::Admin(format!("Failed to get committed offsets: {e}")))?;

        // Calculate lag for each partition
        let mut lag_map = HashMap::new();
        for elem in committed.elements() {
            let (_, high) = self
                .consumer
                .fetch_watermarks(topic, elem.partition(), Duration::from_secs(10))
                .map_err(|e| {
                    TransportError::Admin(format!(
                        "Failed to fetch watermarks for partition {}: {e}",
                        elem.partition()
                    ))
                })?;

            let committed_offset = if let Offset::Offset(o) = elem.offset() {
                o
            } else {
                0
            };

            let lag = high - committed_offset;
            lag_map.insert(elem.partition(), lag.max(0));
        }

        Ok(lag_map)
    }

    // --- Topic Management ---

    /// Create one or more topics.
    ///
    /// Ignores "topic already exists" errors — safe to call repeatedly.
    ///
    /// # Arguments
    ///
    /// * `topics` - Slice of `(name, num_partitions, replication_factor)` tuples
    ///
    /// # Errors
    ///
    /// Returns error if topic creation fails for reasons other than already existing.
    pub async fn create_topics(&self, topics: &[(&str, i32, i32)]) -> TransportResult<()> {
        let new_topics: Vec<NewTopic<'_>> = topics
            .iter()
            .map(|(name, partitions, replication)| {
                NewTopic::new(name, *partitions, TopicReplication::Fixed(*replication))
            })
            .collect();

        let opts = AdminOptions::new().operation_timeout(Some(Duration::from_secs(30)));

        let results = self
            .admin
            .create_topics(&new_topics, &opts)
            .await
            .map_err(|e| TransportError::Admin(format!("Failed to create topics: {e}")))?;

        for result in results {
            if let Err((topic_name, err_code)) = result {
                let err_str = format!("{err_code:?}");
                if err_str.contains("TopicAlreadyExists") {
                    continue;
                }
                return Err(TransportError::Admin(format!(
                    "Failed to create topic {topic_name}: {err_code:?}"
                )));
            }
        }

        Ok(())
    }

    /// Delete one or more topics.
    ///
    /// # Errors
    ///
    /// Returns error if topic deletion fails.
    pub async fn delete_topics(&self, topics: &[&str]) -> TransportResult<()> {
        let opts = AdminOptions::new().operation_timeout(Some(Duration::from_secs(30)));

        let results = self
            .admin
            .delete_topics(topics, &opts)
            .await
            .map_err(|e| TransportError::Admin(format!("Failed to delete topics: {e}")))?;

        for result in results {
            if let Err((topic_name, err_code)) = result {
                return Err(TransportError::Admin(format!(
                    "Failed to delete topic {topic_name}: {err_code:?}"
                )));
            }
        }

        Ok(())
    }

    /// Increase the partition count for a topic.
    ///
    /// Partition count can only be increased, not decreased.
    ///
    /// # Errors
    ///
    /// Returns error if partition increase fails.
    #[allow(clippy::cast_sign_loss)] // new_total is validated to be positive
    pub async fn increase_partitions(&self, topic: &str, new_total: i32) -> TransportResult<()> {
        let new_partitions = NewPartitions::new(topic, new_total.max(0) as usize);
        let opts = AdminOptions::new().request_timeout(Some(Duration::from_secs(30)));

        let results = self
            .admin
            .create_partitions(&[new_partitions], &opts)
            .await
            .map_err(|e| TransportError::Admin(format!("Failed to create partitions: {e}")))?;

        for result in results {
            if let Err((topic_name, err_code)) = result {
                return Err(TransportError::Admin(format!(
                    "Failed to increase partitions for {topic_name}: {err_code:?}"
                )));
            }
        }

        Ok(())
    }

    /// Set the retention period for a topic.
    ///
    /// # Arguments
    ///
    /// * `topic` - Topic name
    /// * `retention_ms` - Retention period in milliseconds
    ///
    /// # Errors
    ///
    /// Returns error if configuration change fails.
    pub async fn set_retention(&self, topic: &str, retention_ms: i64) -> TransportResult<()> {
        let retention_str = retention_ms.to_string();
        let alter_config =
            AlterConfig::new(ResourceSpecifier::Topic(topic)).set("retention.ms", &retention_str);
        let opts = AdminOptions::new().request_timeout(Some(Duration::from_secs(30)));

        let results = self
            .admin
            .alter_configs(&[alter_config], &opts)
            .await
            .map_err(|e| TransportError::Admin(format!("Failed to alter config: {e}")))?;

        for result in results {
            if let Err((_, e)) = result {
                return Err(TransportError::Admin(format!(
                    "Failed to set retention: {e}"
                )));
            }
        }

        Ok(())
    }

    /// Get topic configuration.
    ///
    /// # Returns
    ///
    /// Map of configuration key to value.
    ///
    /// # Errors
    ///
    /// Returns error if configuration fetch fails.
    pub async fn get_topic_config(&self, topic: &str) -> TransportResult<HashMap<String, String>> {
        let resource = ResourceSpecifier::Topic(topic);
        let opts = AdminOptions::new().request_timeout(Some(Duration::from_secs(30)));

        let results = self
            .admin
            .describe_configs(&[resource], &opts)
            .await
            .map_err(|e| TransportError::Admin(format!("Failed to describe configs: {e}")))?;

        let mut config_map = HashMap::new();
        for result in results {
            match result {
                Ok(config_resource) => {
                    for entry in config_resource.entries {
                        if let Some(value) = entry.value {
                            config_map.insert(entry.name, value);
                        }
                    }
                }
                Err(e) => {
                    return Err(TransportError::Admin(format!(
                        "Failed to get topic config: {e}"
                    )));
                }
            }
        }

        Ok(config_map)
    }

    /// List all topics.
    ///
    /// # Returns
    ///
    /// List of topic names.
    ///
    /// # Errors
    ///
    /// Returns error if metadata fetch fails.
    pub fn list_topics(&self) -> TransportResult<Vec<String>> {
        let metadata = self
            .consumer
            .fetch_metadata(None, Duration::from_secs(10))
            .map_err(|e| TransportError::Admin(format!("Failed to fetch metadata: {e}")))?;

        Ok(metadata
            .topics()
            .iter()
            .map(|t| t.name().to_string())
            .collect())
    }

    /// Get topic metadata including partition count and replication factor.
    ///
    /// # Errors
    ///
    /// Returns error if metadata fetch fails.
    pub fn describe_topic(&self, topic: &str) -> TransportResult<TopicInfo> {
        let metadata = self
            .consumer
            .fetch_metadata(Some(topic), Duration::from_secs(10))
            .map_err(|e| TransportError::Admin(format!("Failed to fetch metadata: {e}")))?;

        let topic_meta: &MetadataTopic = metadata
            .topics()
            .iter()
            .find(|t| t.name() == topic)
            .ok_or_else(|| TransportError::Admin(format!("Topic {topic} not found")))?;

        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let replication_factor = topic_meta
            .partitions()
            .first()
            .map_or(0, |p| p.replicas().len() as i32);

        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let partition_count = topic_meta.partitions().len() as i32;

        Ok(TopicInfo {
            name: topic.to_string(),
            partition_count,
            replication_factor,
        })
    }

    // --- Internal helpers ---

    /// Get partition list for a topic.
    async fn get_partition_list(
        &self,
        topic: &str,
        partitions: Option<&[i32]>,
    ) -> TransportResult<TopicPartitionList> {
        let mut tpl = TopicPartitionList::new();

        if let Some(parts) = partitions {
            for &partition in parts {
                tpl.add_partition(topic, partition);
            }
        } else {
            // Get all partitions from metadata
            let metadata = self
                .consumer
                .fetch_metadata(Some(topic), Duration::from_secs(10))
                .map_err(|e| TransportError::Admin(format!("Failed to fetch metadata: {e}")))?;

            let topic_meta: &MetadataTopic =
                metadata
                    .topics()
                    .iter()
                    .find(|t| t.name() == topic)
                    .ok_or_else(|| TransportError::Admin(format!("Topic {topic} not found")))?;

            for partition in topic_meta.partitions() {
                tpl.add_partition(topic, partition.id());
            }
        }

        Ok(tpl)
    }

    /// Commit offsets for a consumer group.
    ///
    /// Note: This requires stopping all consumers in the group first.
    /// The Kafka protocol requires using a consumer from the group to commit offsets,
    /// so we create a temporary consumer with the target group ID.
    async fn commit_offsets_for_group(
        &self,
        group_id: &str,
        offsets: &TopicPartitionList,
    ) -> TransportResult<()> {
        // Create a consumer with the target group to commit offsets
        let mut group_config = self.config.clone();
        group_config.set("group.id", group_id);
        group_config.set("enable.auto.commit", "false");

        let group_consumer: BaseConsumer = group_config.create().map_err(|e| {
            TransportError::Connection(format!("Failed to create group consumer: {e}"))
        })?;

        // Commit the offsets
        group_consumer
            .commit(offsets, rdkafka::consumer::CommitMode::Sync)
            .map_err(|e| TransportError::Commit(format!("Failed to commit offsets: {e}")))?;

        Ok(())
    }
}

impl std::fmt::Debug for KafkaAdmin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KafkaAdmin").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topic_info_debug() {
        let info = TopicInfo {
            name: "test".to_string(),
            partition_count: 3,
            replication_factor: 2,
        };
        assert!(format!("{info:?}").contains("test"));
    }
}
