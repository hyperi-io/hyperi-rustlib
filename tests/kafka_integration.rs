// Project:   hyperi-rustlib
// File:      tests/kafka_integration.rs
// Purpose:   Kafka transport integration tests
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Integration tests for Kafka transport.
//!
//! These tests require a running Kafka broker. They are ignored by default.
//! Run with: `TEST_KAFKA_BROKERS=localhost:9092 cargo test --features transport-kafka -- --ignored`
//!
//! Or set up via environment variables:
//! - `TEST_KAFKA_BROKERS`: Kafka broker addresses (default: localhost:9092)
//! - `TEST_KAFKA_TOPIC`: Test topic name (default: hyperi-rustlib-test)
//! - `TEST_KAFKA_GROUP`: Consumer group ID (default: hyperi-rustlib-test-group)

#![cfg(feature = "transport-kafka")]

use hyperi_rustlib::transport::kafka::{
    healthy_broker_count, total_consumer_lag, BrokerMetrics, KafkaAdmin, KafkaConfig, KafkaMetrics,
    KafkaProfile, KafkaToken, StatsContext, TopicInfo, DEVTEST_PROFILE, PRODUCTION_PROFILE,
};
use std::sync::Arc;

mod common;
use common::EnvGuard;

// --- Unit Tests (no Kafka required) ---

// --- Profile Tests ---

#[test]
fn test_kafka_profile_default_is_production() {
    let config = KafkaConfig::default();
    assert_eq!(config.profile, KafkaProfile::Production);
}

#[test]
fn test_kafka_profile_production() {
    let config = KafkaConfig::production();
    assert_eq!(config.profile, KafkaProfile::Production);
    assert!(!config.ssl_skip_verify);
}

#[test]
fn test_kafka_profile_devtest() {
    let config = KafkaConfig::devtest();
    assert_eq!(config.profile, KafkaProfile::DevTest);
    assert!(config.ssl_skip_verify); // Auto-enabled for devtest
}

#[test]
fn test_kafka_profile_with_profile() {
    let config = KafkaConfig::default().with_profile(KafkaProfile::DevTest);
    assert_eq!(config.profile, KafkaProfile::DevTest);
    assert!(config.ssl_skip_verify);
}

#[test]
fn test_kafka_profile_defaults_production() {
    let config = KafkaConfig::production();
    let defaults = config.profile_defaults();
    assert_eq!(defaults, PRODUCTION_PROFILE);
}

#[test]
fn test_kafka_profile_defaults_devtest() {
    let config = KafkaConfig::devtest();
    let defaults = config.profile_defaults();
    assert_eq!(defaults, DEVTEST_PROFILE);
}

#[test]
fn test_kafka_profile_from_str() {
    assert_eq!(
        "production".parse::<KafkaProfile>().unwrap(),
        KafkaProfile::Production
    );
    assert_eq!(
        "prod".parse::<KafkaProfile>().unwrap(),
        KafkaProfile::Production
    );
    assert_eq!(
        "devtest".parse::<KafkaProfile>().unwrap(),
        KafkaProfile::DevTest
    );
    assert_eq!(
        "dev".parse::<KafkaProfile>().unwrap(),
        KafkaProfile::DevTest
    );
    assert_eq!(
        "test".parse::<KafkaProfile>().unwrap(),
        KafkaProfile::DevTest
    );
    assert!("invalid".parse::<KafkaProfile>().is_err());
}

#[test]
fn test_kafka_profile_display() {
    assert_eq!(KafkaProfile::Production.to_string(), "production");
    assert_eq!(KafkaProfile::DevTest.to_string(), "devtest");
}

// --- Override Tests ---

#[test]
fn test_kafka_config_with_override() {
    let config = KafkaConfig::production().with_override("fetch.min.bytes", "2097152");

    assert_eq!(
        config.librdkafka_overrides.get("fetch.min.bytes"),
        Some(&"2097152".to_string())
    );
}

#[test]
fn test_kafka_config_with_overrides() {
    let config = KafkaConfig::production().with_overrides(&[
        ("fetch.min.bytes", "2097152"),
        ("statistics.interval.ms", "5000"),
    ]);

    assert_eq!(
        config.librdkafka_overrides.get("fetch.min.bytes"),
        Some(&"2097152".to_string())
    );
    assert_eq!(
        config.librdkafka_overrides.get("statistics.interval.ms"),
        Some(&"5000".to_string())
    );
}

#[test]
fn test_kafka_build_librdkafka_config_priority() {
    let mut config = KafkaConfig::production();
    // Profile sets fetch.min.bytes = 1048576 (1MB)
    // Override should win
    config
        .librdkafka_overrides
        .insert("fetch.min.bytes".to_string(), "2097152".to_string());

    let built = config.build_librdkafka_config();
    assert_eq!(built.get("fetch.min.bytes"), Some(&"2097152".to_string()));
}

// --- Basic Config Tests ---

#[test]
fn test_kafka_config_defaults() {
    let config = KafkaConfig::default();

    assert_eq!(config.brokers, vec!["localhost:9092"]);
    assert_eq!(config.group, "hyperi-rustlib-consumer");
    assert_eq!(config.client_id, "hyperi-rustlib");
    assert!(!config.enable_auto_commit);
    assert_eq!(config.auto_offset_reset, "earliest");
    assert_eq!(config.fetch_max_bytes, 52_428_800); // 50MB
    assert_eq!(config.session_timeout_ms, 45000);
    assert_eq!(config.heartbeat_interval_ms, 3000);
    assert_eq!(config.max_poll_interval_ms, 300_000);
}

#[test]
fn test_kafka_config_for_testing() {
    let config = KafkaConfig::for_testing("kafka:9092", "test-group", vec!["events".to_string()]);

    assert_eq!(config.brokers, vec!["kafka:9092"]);
    assert_eq!(config.group, "test-group");
    assert_eq!(config.topics, vec!["events"]);
}

#[test]
fn test_kafka_config_with_scram() {
    let config = KafkaConfig::default().with_scram("SCRAM-SHA-256", "user", "pass");

    assert_eq!(config.security_protocol, "sasl_plaintext");
    assert_eq!(config.sasl_mechanism, Some("SCRAM-SHA-256".to_string()));
    assert_eq!(config.sasl_username, Some("user".to_string()));
    assert_eq!(config.sasl_password, Some("pass".to_string()));
}

#[test]
fn test_kafka_config_with_scram_ssl() {
    let config = KafkaConfig::default().with_scram_ssl("SCRAM-SHA-512", "user", "pass");

    assert_eq!(config.security_protocol, "sasl_ssl");
    assert_eq!(config.sasl_mechanism, Some("SCRAM-SHA-512".to_string()));
}

#[test]
fn test_kafka_config_with_tls() {
    let config = KafkaConfig::default().with_tls(Some("/path/to/ca.crt"));

    assert_eq!(config.security_protocol, "ssl");
    assert_eq!(config.ssl_ca_location, Some("/path/to/ca.crt".to_string()));
}

#[test]
fn test_kafka_config_with_tls_upgrades_sasl() {
    let config = KafkaConfig::default()
        .with_scram("PLAIN", "user", "pass")
        .with_tls(None);

    assert_eq!(config.security_protocol, "sasl_ssl");
}

#[test]
fn test_kafka_config_with_client_cert() {
    let config = KafkaConfig::default().with_client_cert("/path/cert.pem", "/path/key.pem");

    assert_eq!(
        config.ssl_certificate_location,
        Some("/path/cert.pem".to_string())
    );
    assert_eq!(config.ssl_key_location, Some("/path/key.pem".to_string()));
}

#[test]
fn test_kafka_config_with_ssl_skip_verify() {
    let config = KafkaConfig::default().with_ssl_skip_verify();

    assert!(config.ssl_skip_verify);
}

#[test]
fn test_kafka_config_with_ssl_insecure() {
    let config = KafkaConfig::default().with_ssl_insecure();

    assert_eq!(config.security_protocol, "ssl");
    assert!(config.ssl_skip_verify);
}

#[test]
fn test_kafka_config_with_ssl_insecure_upgrades_sasl() {
    let config = KafkaConfig::default()
        .with_scram("PLAIN", "user", "pass")
        .with_ssl_insecure();

    assert_eq!(config.security_protocol, "sasl_ssl");
    assert!(config.ssl_skip_verify);
}

#[test]
fn test_kafka_config_with_producer_defaults() {
    let config = KafkaConfig::default().with_producer_defaults();
    let built = config.build_librdkafka_config();

    assert_eq!(built.get("acks"), Some(&"all".to_string()));
    assert_eq!(built.get("retries"), Some(&"5".to_string()));
    assert_eq!(built.get("compression.type"), Some(&"lz4".to_string()));
    assert_eq!(built.get("linger.ms"), Some(&"50".to_string()));
}

#[test]
fn test_kafka_production_profile_settings() {
    // Production profile is the default
    let config = KafkaConfig::production();
    let built = config.build_librdkafka_config();

    // Verify production profile settings
    assert_eq!(
        built.get("queued.min.messages"),
        Some(&"100000".to_string())
    );
    assert_eq!(
        built.get("queued.max.messages.kbytes"),
        Some(&"1048576".to_string())
    );
    assert_eq!(
        built.get("partition.assignment.strategy"),
        Some(&"cooperative-sticky".to_string())
    );
    assert_eq!(built.get("check.crcs"), Some(&"false".to_string()));
    assert_eq!(built.get("socket.nagle.disable"), Some(&"true".to_string()));
}

#[test]
fn test_kafka_devtest_profile_settings() {
    let config = KafkaConfig::devtest();
    let built = config.build_librdkafka_config();

    // Verify devtest profile settings
    assert_eq!(built.get("queued.min.messages"), Some(&"1000".to_string()));
    assert_eq!(
        built.get("queued.max.messages.kbytes"),
        Some(&"65536".to_string())
    );
    assert_eq!(built.get("check.crcs"), Some(&"true".to_string())); // CRC enabled in devtest
    assert_eq!(built.get("reconnect.backoff.ms"), Some(&"10".to_string()));
    assert_eq!(built.get("log.connection.close"), Some(&"true".to_string()));
}

#[test]
fn test_kafka_config_with_low_latency() {
    let config = KafkaConfig::default().with_low_latency();
    let built = config.build_librdkafka_config();

    assert_eq!(built.get("fetch.wait.max.ms"), Some(&"10".to_string()));
    assert_eq!(built.get("fetch.min.bytes"), Some(&"1".to_string()));
    assert_eq!(built.get("reconnect.backoff.ms"), Some(&"10".to_string()));
    assert_eq!(
        built.get("reconnect.backoff.max.ms"),
        Some(&"100".to_string())
    );
}

#[test]
fn test_kafka_config_with_statistics() {
    let config = KafkaConfig::default().with_statistics(5000);

    // with_statistics uses librdkafka_overrides
    assert_eq!(
        config.librdkafka_overrides.get("statistics.interval.ms"),
        Some(&"5000".to_string())
    );

    // Also verify it appears in built config
    let built = config.build_librdkafka_config();
    assert_eq!(
        built.get("statistics.interval.ms"),
        Some(&"5000".to_string())
    );
}

#[test]
fn test_kafka_config_with_cloud_connection_tuning() {
    let config = KafkaConfig::default().with_cloud_connection_tuning();
    let built = config.build_librdkafka_config();

    // Cloud tuning is in librdkafka_overrides
    assert_eq!(
        built.get("socket.keepalive.enable"),
        Some(&"true".to_string())
    );
    assert_eq!(
        built.get("metadata.max.age.ms"),
        Some(&"180000".to_string())
    );
    assert_eq!(
        built.get("socket.connection.setup.timeout.ms"),
        Some(&"30000".to_string())
    );
}

#[test]
fn test_kafka_config_chained_builders() {
    // Test that all builder methods can be chained together
    let config = KafkaConfig::production()
        .with_scram_ssl("SCRAM-SHA-512", "user", "pass")
        .with_statistics(1000)
        .with_cloud_connection_tuning()
        .with_override("fetch.min.bytes", "2097152");

    let built = config.build_librdkafka_config();

    // Verify SASL
    assert_eq!(config.security_protocol, "sasl_ssl");
    assert_eq!(config.sasl_mechanism, Some("SCRAM-SHA-512".to_string()));

    // Verify production profile defaults are present
    assert_eq!(
        built.get("queued.min.messages"),
        Some(&"100000".to_string())
    );

    // Verify statistics override
    assert_eq!(
        built.get("statistics.interval.ms"),
        Some(&"1000".to_string())
    );

    // Verify cloud tuning
    assert_eq!(
        built.get("socket.keepalive.enable"),
        Some(&"true".to_string())
    );

    // Verify explicit override wins
    assert_eq!(built.get("fetch.min.bytes"), Some(&"2097152".to_string()));
}

#[test]
fn test_kafka_config_from_env() {
    let _guard = EnvGuard::new(&[
        ("TESTAPP_BOOTSTRAP_SERVERS", "kafka1:9092,kafka2:9092"),
        ("TESTAPP_GROUP_ID", "test-consumer"),
        ("TESTAPP_CLIENT_ID", "test-client"),
        ("TESTAPP_SECURITY_PROTOCOL", "sasl_ssl"),
        ("TESTAPP_SASL_MECHANISM", "SCRAM-SHA-256"),
        ("TESTAPP_SASL_USERNAME", "testuser"),
        ("TESTAPP_SASL_PASSWORD", "testpass"),
        ("TESTAPP_SSL_SKIP_VERIFY", "true"),
        ("TESTAPP_TOPICS", "topic1,topic2,topic3"),
    ]);

    let config = KafkaConfig::from_env("TESTAPP");

    assert_eq!(config.brokers, vec!["kafka1:9092", "kafka2:9092"]);
    assert_eq!(config.group, "test-consumer");
    assert_eq!(config.client_id, "test-client");
    assert_eq!(config.security_protocol, "sasl_ssl");
    assert_eq!(config.sasl_mechanism, Some("SCRAM-SHA-256".to_string()));
    assert_eq!(config.sasl_username, Some("testuser".to_string()));
    assert_eq!(config.sasl_password, Some("testpass".to_string()));
    assert!(config.ssl_skip_verify);
    assert_eq!(config.topics, vec!["topic1", "topic2", "topic3"]);
}

#[test]
fn test_kafka_librdkafka_overrides_win() {
    // User overrides should win over profile defaults
    let config = KafkaConfig::production().with_override("queued.min.messages", "50000"); // Override the profile's 100000

    let built = config.build_librdkafka_config();

    // User override should win
    assert_eq!(built.get("queued.min.messages"), Some(&"50000".to_string()));
}

#[test]
fn test_kafka_config_from_env_with_profile() {
    let _guard = EnvGuard::new(&[
        ("TESTAPP2_PROFILE", "devtest"),
        ("TESTAPP2_BOOTSTRAP_SERVERS", "kafka:9092"),
    ]);

    let config = KafkaConfig::from_env("TESTAPP2");

    assert_eq!(config.profile, KafkaProfile::DevTest);
    assert!(config.ssl_skip_verify); // Auto-enabled for devtest
    assert_eq!(config.brokers, vec!["kafka:9092"]);
}

// --- Token Tests ---

#[test]
fn test_kafka_token_display() {
    let token = KafkaToken::new(Arc::from("events"), 0, 12345);
    assert_eq!(token.to_string(), "kafka:events:0:12345");
}

#[test]
fn test_kafka_token_equality() {
    let token1 = KafkaToken::new(Arc::from("events"), 0, 100);
    let token2 = KafkaToken::new(Arc::from("events"), 0, 100);
    let token3 = KafkaToken::new(Arc::from("events"), 1, 100);

    assert_eq!(token1, token2);
    assert_ne!(token1, token3);
}

#[test]
fn test_kafka_token_hash() {
    use std::collections::HashSet;

    let mut set = HashSet::new();
    set.insert(KafkaToken::new(Arc::from("events"), 0, 100));
    set.insert(KafkaToken::new(Arc::from("events"), 0, 100)); // Duplicate
    set.insert(KafkaToken::new(Arc::from("events"), 1, 100));

    assert_eq!(set.len(), 2);
}

// --- Metrics Tests ---

#[test]
fn test_kafka_metrics_default() {
    let metrics = KafkaMetrics::default();

    assert_eq!(metrics.messages_sent, 0);
    assert_eq!(metrics.messages_received, 0);
    assert_eq!(metrics.bytes_sent, 0);
    assert_eq!(metrics.bytes_received, 0);
    assert!(metrics.brokers.is_empty());
    assert!(metrics.partition_lag.is_empty());
}

#[test]
fn test_total_consumer_lag() {
    let mut metrics = KafkaMetrics::default();
    metrics.partition_lag.insert(("events".to_string(), 0), 100);
    metrics.partition_lag.insert(("events".to_string(), 1), 200);
    metrics.partition_lag.insert(("events".to_string(), 2), 50);

    assert_eq!(total_consumer_lag(&metrics), 350);
}

#[test]
fn test_healthy_broker_count() {
    let mut metrics = KafkaMetrics::default();
    metrics.brokers.insert(
        "broker1".to_string(),
        BrokerMetrics {
            state: "UP".to_string(),
            ..Default::default()
        },
    );
    metrics.brokers.insert(
        "broker2".to_string(),
        BrokerMetrics {
            state: "DOWN".to_string(),
            ..Default::default()
        },
    );
    metrics.brokers.insert(
        "broker3".to_string(),
        BrokerMetrics {
            state: "UP".to_string(),
            ..Default::default()
        },
    );

    assert_eq!(healthy_broker_count(&metrics), 2);
}

#[test]
fn test_stats_context_creation() {
    let ctx = StatsContext::new();
    let metrics = ctx.get_metrics();

    assert_eq!(metrics.messages_sent, 0);
    assert!(ctx.get_raw_stats().is_none());
}

#[test]
fn test_stats_context_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<StatsContext>();
}

// --- TopicInfo Tests ---

#[test]
fn test_topic_info_debug() {
    let info = TopicInfo {
        name: "events".to_string(),
        partition_count: 12,
        replication_factor: 3,
    };

    let debug = format!("{info:?}");
    assert!(debug.contains("events"));
    assert!(debug.contains("12"));
    assert!(debug.contains('3'));
}

// --- Integration Tests (require running Kafka) ---

fn get_test_config() -> Option<KafkaConfig> {
    let brokers = std::env::var("TEST_KAFKA_BROKERS").ok()?;

    Some(KafkaConfig {
        brokers: brokers.split(',').map(|s| s.to_string()).collect(),
        group: std::env::var("TEST_KAFKA_GROUP")
            .unwrap_or_else(|_| "hyperi-rustlib-test-group".to_string()),
        topics: vec![
            std::env::var("TEST_KAFKA_TOPIC").unwrap_or_else(|_| "hyperi-rustlib-test".to_string())
        ],
        ..Default::default()
    })
}

#[tokio::test]
#[ignore = "requires Kafka broker - set TEST_KAFKA_BROKERS to run"]
async fn test_kafka_transport_connection() {
    use hyperi_rustlib::transport::kafka::KafkaTransport;

    let Some(config) = get_test_config() else {
        eprintln!("Skipping: TEST_KAFKA_BROKERS not set");
        return;
    };

    let transport = KafkaTransport::new(&config).await;
    assert!(
        transport.is_ok(),
        "Failed to connect: {:?}",
        transport.err()
    );
}

#[tokio::test]
#[ignore = "requires Kafka broker - set TEST_KAFKA_BROKERS to run"]
async fn test_kafka_admin_list_topics() {
    let Some(config) = get_test_config() else {
        eprintln!("Skipping: TEST_KAFKA_BROKERS not set");
        return;
    };

    let admin = KafkaAdmin::new(&config);
    assert!(admin.is_ok(), "Failed to create admin: {:?}", admin.err());

    let admin = admin.unwrap();
    let topics = admin.list_topics();
    assert!(topics.is_ok(), "Failed to list topics: {:?}", topics.err());

    println!("Available topics: {:?}", topics.unwrap());
}

#[tokio::test]
#[ignore = "requires Kafka broker - set TEST_KAFKA_BROKERS to run"]
async fn test_kafka_admin_describe_topic() {
    let Some(config) = get_test_config() else {
        eprintln!("Skipping: TEST_KAFKA_BROKERS not set");
        return;
    };

    let admin = KafkaAdmin::new(&config).unwrap();
    let topic = config.topics.first().unwrap();

    let info = admin.describe_topic(topic);
    if let Ok(info) = info {
        println!("Topic info: {info:?}");
        assert_eq!(info.name, *topic);
        assert!(info.partition_count > 0);
    } else {
        eprintln!("Topic {topic} not found (expected in integration tests)");
    }
}

#[tokio::test]
#[ignore = "requires Kafka broker - set TEST_KAFKA_BROKERS to run"]
async fn test_kafka_send_receive_batch() {
    use hyperi_rustlib::transport::{kafka::KafkaTransport, Transport};

    let Some(mut config) = get_test_config() else {
        eprintln!("Skipping: TEST_KAFKA_BROKERS not set");
        return;
    };

    // Use unique group to avoid interference
    config.group = format!("hyperi-rustlib-test-{}", std::process::id());

    let transport = KafkaTransport::new(&config).await.unwrap();
    let topic = config.topics.first().unwrap();

    // Send a batch of messages
    for i in 0..10 {
        let payload = format!(r#"{{"id": {i}, "data": "test"}}"#);
        let result = transport.send(topic, payload.as_bytes()).await;
        assert!(result.is_ok(), "Send failed: {result:?}");
    }

    // Receive messages (may not get all if topic is shared)
    let messages = transport.recv(100).await;
    assert!(messages.is_ok(), "Recv failed: {:?}", messages.err());

    let messages = messages.unwrap();
    println!("Received {} messages", messages.len());

    // Commit if we got messages
    if !messages.is_empty() {
        let tokens: Vec<_> = messages.iter().map(|m| m.token.clone()).collect();
        let result = transport.commit(&tokens).await;
        assert!(result.is_ok(), "Commit failed: {:?}", result.err());
    }
}

#[tokio::test]
#[ignore = "requires Kafka broker - set TEST_KAFKA_BROKERS to run"]
async fn test_kafka_consumer_lag() {
    let Some(config) = get_test_config() else {
        eprintln!("Skipping: TEST_KAFKA_BROKERS not set");
        return;
    };

    let admin = KafkaAdmin::new(&config).unwrap();
    let topic = config.topics.first().unwrap();

    let lag = admin.get_consumer_lag(&config.group, topic).await;
    if let Ok(lag) = lag {
        println!("Consumer lag per partition: {lag:?}");
        for (partition, lag) in lag {
            println!("  Partition {partition}: lag {lag}");
        }
    } else {
        eprintln!("Could not get lag (may need messages in topic)");
    }
}
