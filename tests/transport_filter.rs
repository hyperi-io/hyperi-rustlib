// Project:   hyperi-rustlib
// File:      tests/transport_filter.rs
// Purpose:   Integration + adversarial tests for transport filter engine
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Transport filter engine integration tests.
//!
//! Tests the full round-trip: configure filters on MemoryTransport, inject
//! messages, verify filtering behaviour in recv() and send().
//!
//! Includes:
//! - Expected failures (invalid CEL, tier rejections, DLQ without config)
//! - Sample data from real DFE pipelines
//! - Adversarial inputs (binary garbage, truncated JSON, Unicode, 1MB payloads)

#![cfg(feature = "transport-memory")]

use hyperi_rustlib::transport::filter::{
    FilterAction, FilterDisposition, FilterRule, TransportFilterEngine, TransportFilterTierConfig,
};
use hyperi_rustlib::transport::memory::{MemoryConfig, MemoryTransport};
use hyperi_rustlib::transport::{TransportReceiver, TransportSender};

// ============================================================================
// Helper: build a MemoryTransport with inbound filters
// ============================================================================

fn transport_with_inbound_filters(filters: Vec<FilterRule>) -> MemoryTransport {
    MemoryTransport::new(&MemoryConfig {
        buffer_size: 1000,
        recv_timeout_ms: 50,
        filters_in: filters,
        filters_out: Vec::new(),
    })
}

fn transport_with_outbound_filters(filters: Vec<FilterRule>) -> MemoryTransport {
    MemoryTransport::new(&MemoryConfig {
        buffer_size: 1000,
        recv_timeout_ms: 50,
        filters_in: Vec::new(),
        filters_out: filters,
    })
}

fn transport_no_filters() -> MemoryTransport {
    MemoryTransport::new(&MemoryConfig {
        buffer_size: 1000,
        recv_timeout_ms: 50,
        ..Default::default()
    })
}

// ============================================================================
// Section 1: MemoryTransport Round-Trip Integration Tests
// ============================================================================

#[tokio::test]
async fn inbound_filter_drops_matching_messages() {
    let transport = transport_with_inbound_filters(vec![FilterRule {
        expression: r#"status == "poison""#.into(),
        action: FilterAction::Drop,
    }]);

    // Inject 5 messages: 2 poison, 3 healthy
    transport
        .inject(None, br#"{"status":"ok","id":1}"#.to_vec())
        .await
        .unwrap();
    transport
        .inject(None, br#"{"status":"poison","id":2}"#.to_vec())
        .await
        .unwrap();
    transport
        .inject(None, br#"{"status":"ok","id":3}"#.to_vec())
        .await
        .unwrap();
    transport
        .inject(None, br#"{"status":"poison","id":4}"#.to_vec())
        .await
        .unwrap();
    transport
        .inject(None, br#"{"status":"ok","id":5}"#.to_vec())
        .await
        .unwrap();

    let messages = transport.recv(10).await.unwrap();
    assert_eq!(
        messages.len(),
        3,
        "Should receive 3 messages (2 poison dropped)"
    );
}

#[tokio::test]
async fn inbound_filter_dlq_removes_from_batch() {
    let transport = transport_with_inbound_filters(vec![FilterRule {
        expression: "has(_internal)".into(),
        action: FilterAction::Dlq,
    }]);

    transport
        .inject(None, br#"{"data":"keep"}"#.to_vec())
        .await
        .unwrap();
    transport
        .inject(None, br#"{"_internal":true,"data":"dlq"}"#.to_vec())
        .await
        .unwrap();
    transport
        .inject(None, br#"{"data":"also_keep"}"#.to_vec())
        .await
        .unwrap();

    let messages = transport.recv(10).await.unwrap();
    assert_eq!(
        messages.len(),
        2,
        "DLQ message should be removed from batch"
    );
}

#[tokio::test]
async fn outbound_filter_blocks_send() {
    let transport = transport_with_outbound_filters(vec![FilterRule {
        expression: "has(debug)".into(),
        action: FilterAction::Drop,
    }]);

    // Send a debug message — should be silently dropped
    let result = transport
        .send("topic", br#"{"debug":true,"msg":"test"}"#)
        .await;
    assert!(
        result.is_ok(),
        "Filtered send should return Ok (silent drop)"
    );

    // Send a normal message — should go through
    let result = transport.send("topic", br#"{"msg":"normal"}"#).await;
    assert!(result.is_ok());

    // Only the normal message should be receivable
    let messages = transport.recv(10).await.unwrap();
    assert_eq!(
        messages.len(),
        1,
        "Only non-filtered message should be received"
    );
}

#[tokio::test]
async fn outbound_filter_dlq_returns_filtered_dlq() {
    let transport = transport_with_outbound_filters(vec![FilterRule {
        expression: r#"status == "bad""#.into(),
        action: FilterAction::Dlq,
    }]);

    let result = transport
        .send("topic", br#"{"status":"bad","data":"x"}"#)
        .await;
    assert!(
        result.is_filtered_dlq(),
        "DLQ filter should return FilteredDlq"
    );

    let result = transport
        .send("topic", br#"{"status":"good","data":"x"}"#)
        .await;
    assert!(result.is_ok(), "Non-matching message should send normally");
}

#[tokio::test]
async fn no_filters_passthrough() {
    let transport = transport_no_filters();

    for i in 0..10 {
        transport
            .inject(None, format!(r#"{{"id":{i}}}"#).into_bytes())
            .await
            .unwrap();
    }

    let messages = transport.recv(20).await.unwrap();
    assert_eq!(
        messages.len(),
        10,
        "All messages should pass through with no filters"
    );
}

#[tokio::test]
async fn first_match_wins_integration() {
    let transport = transport_with_inbound_filters(vec![
        FilterRule {
            expression: r#"status == "a""#.into(),
            action: FilterAction::Drop,
        },
        FilterRule {
            expression: r#"status == "b""#.into(),
            action: FilterAction::Dlq,
        },
        FilterRule {
            expression: "has(status)".into(),
            action: FilterAction::Drop,
        },
    ]);

    transport
        .inject(None, br#"{"status":"a"}"#.to_vec())
        .await
        .unwrap(); // matches filter 0 → drop
    transport
        .inject(None, br#"{"status":"b"}"#.to_vec())
        .await
        .unwrap(); // matches filter 1 → dlq
    transport
        .inject(None, br#"{"status":"c"}"#.to_vec())
        .await
        .unwrap(); // matches filter 2 → drop
    transport
        .inject(None, br#"{"no_status":true}"#.to_vec())
        .await
        .unwrap(); // matches nothing → pass

    let messages = transport.recv(10).await.unwrap();
    assert_eq!(messages.len(), 1, "Only the no-status message should pass");
}

#[tokio::test]
async fn mixed_tier1_filters() {
    let transport = transport_with_inbound_filters(vec![
        FilterRule {
            expression: "has(_internal)".into(),
            action: FilterAction::Drop,
        },
        FilterRule {
            expression: r#"source == "test""#.into(),
            action: FilterAction::Drop,
        },
        FilterRule {
            expression: r#"host.startsWith("debug-")"#.into(),
            action: FilterAction::Drop,
        },
    ]);

    transport
        .inject(None, br#"{"_internal":true}"#.to_vec())
        .await
        .unwrap(); // drop (exists)
    transport
        .inject(None, br#"{"source":"test"}"#.to_vec())
        .await
        .unwrap(); // drop (equals)
    transport
        .inject(None, br#"{"host":"debug-web01"}"#.to_vec())
        .await
        .unwrap(); // drop (startsWith)
    transport
        .inject(None, br#"{"host":"prod-web01","source":"live"}"#.to_vec())
        .await
        .unwrap(); // pass

    let messages = transport.recv(10).await.unwrap();
    assert_eq!(messages.len(), 1);
}

// ============================================================================
// Section 2: Expected Failure Tests
// ============================================================================

#[test]
fn expected_fail_tier2_without_opt_in() {
    let rules = vec![FilterRule {
        expression: r#"severity > 3 && source != "internal""#.into(),
        action: FilterAction::Drop,
    }];
    let result = TransportFilterEngine::new(&rules, &[], &TransportFilterTierConfig::default());
    assert!(result.is_err(), "Tier 2 should be rejected without opt-in");
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Tier 2"), "Error should mention tier: {err}");
    assert!(
        err.contains("allow_cel_filters_in"),
        "Error should mention config to enable: {err}"
    );
}

#[test]
fn expected_fail_tier3_without_complex_opt_in() {
    let tier_config = TransportFilterTierConfig {
        allow_cel_filters_in: true, // Tier 2 enabled, but NOT Tier 3
        ..Default::default()
    };
    let rules = vec![FilterRule {
        expression: r#"field.matches("^prod-.*")"#.into(),
        action: FilterAction::Drop,
    }];
    let result = TransportFilterEngine::new(&rules, &[], &tier_config);
    assert!(
        result.is_err(),
        "Tier 3 should be rejected without complex opt-in"
    );
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Tier 3"), "Error should mention tier: {err}");
}

#[test]
fn expected_fail_tier3_iteration_blocked() {
    let tier_config = TransportFilterTierConfig {
        allow_cel_filters_in: true,
        ..Default::default()
    };
    let rules = vec![FilterRule {
        expression: r#"tags.exists(t, t == "pii")"#.into(),
        action: FilterAction::Dlq,
    }];
    let result = TransportFilterEngine::new(&rules, &[], &tier_config);
    assert!(result.is_err(), "Iteration should be Tier 3");
}

#[test]
fn expected_fail_invalid_cel_syntax() {
    let rules = vec![FilterRule {
        expression: "this is not valid ((( CEL syntax )))".into(),
        action: FilterAction::Drop,
    }];
    let result = TransportFilterEngine::new(&rules, &[], &TransportFilterTierConfig::default());
    assert!(result.is_err(), "Invalid CEL should fail at construction");
}

#[test]
fn expected_fail_empty_expression() {
    let rules = vec![FilterRule {
        expression: String::new(),
        action: FilterAction::Drop,
    }];
    let result = TransportFilterEngine::new(&rules, &[], &TransportFilterTierConfig::default());
    assert!(result.is_err(), "Empty expression should fail");
}

#[test]
fn expected_fail_whitespace_only_expression() {
    let rules = vec![FilterRule {
        expression: "   ".into(),
        action: FilterAction::Drop,
    }];
    let result = TransportFilterEngine::new(&rules, &[], &TransportFilterTierConfig::default());
    assert!(result.is_err(), "Whitespace-only expression should fail");
}

// ============================================================================
// Section 3: Sample Data Tests (real DFE pipeline payloads)
// ============================================================================

#[test]
fn sample_data_syslog_event() {
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: r#"source_type == "syslog""#.into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    let syslog_event = br#"{"source_type":"syslog","host":"web01","facility":"auth","severity":6,"message":"user login","_timestamp":"2026-01-01T00:00:00Z"}"#;
    assert_eq!(engine.apply_inbound(syslog_event), FilterDisposition::Drop);

    let windows_event =
        br#"{"source_type":"windows_event","host":"dc01","event_id":4624,"message":"logon"}"#;
    assert_eq!(engine.apply_inbound(windows_event), FilterDisposition::Pass);
}

#[test]
fn sample_data_nested_cloud_event() {
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: r#"metadata.source == "aws""#.into(),
            action: FilterAction::Dlq,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    let aws_event = br#"{"metadata":{"source":"aws","region":"ap-southeast-2"},"event_type":"cloudtrail","data":{"user":"admin"}}"#;
    assert_eq!(engine.apply_inbound(aws_event), FilterDisposition::Dlq);

    let azure_event =
        br#"{"metadata":{"source":"azure","tenant":"contoso"},"event_type":"activity_log"}"#;
    assert_eq!(engine.apply_inbound(azure_event), FilterDisposition::Pass);
}

#[test]
fn sample_data_dfe_loader_routing() {
    // Loader uses _table field for routing — filter out internal tables
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: r#"_table.startsWith("_internal")"#.into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    let internal = br#"{"_table":"_internal_metrics","value":42}"#;
    assert_eq!(engine.apply_inbound(internal), FilterDisposition::Drop);

    let normal = br#"{"_table":"auth_events","user":"admin","action":"login"}"#;
    assert_eq!(engine.apply_inbound(normal), FilterDisposition::Pass);
}

#[test]
fn sample_data_receiver_poison_message() {
    let engine = TransportFilterEngine::new(
        &[
            FilterRule {
                expression: r#"status == "poison""#.into(),
                action: FilterAction::Dlq,
            },
            FilterRule {
                expression: "!has(_table)".into(),
                action: FilterAction::Dlq,
            },
        ],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    let poison = br#"{"status":"poison","_table":"events","data":"bad"}"#;
    assert_eq!(engine.apply_inbound(poison), FilterDisposition::Dlq);

    let no_table = br#"{"status":"ok","data":"missing routing field"}"#;
    assert_eq!(engine.apply_inbound(no_table), FilterDisposition::Dlq);

    let valid = br#"{"status":"ok","_table":"events","data":"good"}"#;
    assert_eq!(engine.apply_inbound(valid), FilterDisposition::Pass);
}

#[test]
fn sample_data_fetcher_debug_filter() {
    // Fetcher outbound: don't send debug events downstream
    let engine = TransportFilterEngine::new(
        &[],
        &[FilterRule {
            expression: "has(debug)".into(),
            action: FilterAction::Drop,
        }],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    let debug_event = br#"{"debug":true,"source":"aws","data":"test"}"#;
    assert_eq!(engine.apply_outbound(debug_event), FilterDisposition::Drop);

    let real_event = br#"{"source":"aws","event_type":"cloudtrail","data":{"user":"admin"}}"#;
    assert_eq!(engine.apply_outbound(real_event), FilterDisposition::Pass);
}

// ============================================================================
// Section 4: Adversarial Tests
// ============================================================================

#[test]
fn adversarial_binary_garbage() {
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: "has(_table)".into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    let garbage: &[u8] = &[0xFF, 0xFE, 0x00, 0x01, 0x02, 0xAB, 0xCD];
    assert_eq!(engine.apply_inbound(garbage), FilterDisposition::Pass);
}

#[test]
fn adversarial_truncated_json() {
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: "has(_table)".into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    // Truncated JSON with `"_table":` pattern: the memmem fast-path detects
    // the field exists pattern, so it matches the filter (Drop action).
    // This is safe filtering behaviour — broken JSON containing the field
    // pattern is treated as if the field exists.
    assert_eq!(
        engine.apply_inbound(br#"{"_table":"ev"#),
        FilterDisposition::Drop
    );
    // Truncated before the colon: pattern not present, no match
    assert_eq!(
        engine.apply_inbound(br#"{"_table"#),
        FilterDisposition::Pass
    );
    assert_eq!(engine.apply_inbound(br#"{"#), FilterDisposition::Pass);
}

#[test]
fn adversarial_empty_payload() {
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: "has(anything)".into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    assert_eq!(engine.apply_inbound(b""), FilterDisposition::Pass);
}

#[test]
fn adversarial_json_null() {
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: "has(field)".into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    assert_eq!(engine.apply_inbound(b"null"), FilterDisposition::Pass);
}

#[test]
fn adversarial_json_array() {
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: "has(field)".into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    assert_eq!(engine.apply_inbound(b"[1,2,3]"), FilterDisposition::Pass);
}

#[test]
fn adversarial_json_string() {
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: "has(field)".into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    assert_eq!(
        engine.apply_inbound(br#""just a string""#),
        FilterDisposition::Pass
    );
}

#[test]
fn adversarial_cel_chars_in_value() {
    // Field value contains characters that look like CEL operators
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: r#"field == "value with == and && in it""#.into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    let payload = br#"{"field":"value with == and && in it"}"#;
    assert_eq!(engine.apply_inbound(payload), FilterDisposition::Drop);

    let other = br#"{"field":"normal value"}"#;
    assert_eq!(engine.apply_inbound(other), FilterDisposition::Pass);
}

#[test]
fn adversarial_large_payload_1mb() {
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: r#"status == "poison""#.into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    // 1MB payload with the poison field buried at the start
    let mut payload = br#"{"status":"poison","data":""#.to_vec();
    payload.extend(vec![b'x'; 1_000_000]);
    payload.extend(br#""}"#);

    assert_eq!(engine.apply_inbound(&payload), FilterDisposition::Drop);
}

#[test]
fn adversarial_many_filters() {
    let rules: Vec<FilterRule> = (0..100)
        .map(|i| FilterRule {
            expression: format!(r#"field_{i} == "value_{i}""#),
            action: FilterAction::Drop,
        })
        .collect();

    let engine =
        TransportFilterEngine::new(&rules, &[], &TransportFilterTierConfig::default()).unwrap();

    // Message matching filter 99 (last one)
    let payload = br#"{"field_99":"value_99"}"#;
    assert_eq!(engine.apply_inbound(payload), FilterDisposition::Drop);

    // Message matching nothing
    let payload = br#"{"field_999":"value_999"}"#;
    assert_eq!(engine.apply_inbound(payload), FilterDisposition::Pass);
}

#[test]
fn adversarial_missing_field_no_error() {
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: r#"nonexistent_field == "value""#.into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    let payload = br#"{"other":"data"}"#;
    // Field missing → no match (not error)
    assert_eq!(engine.apply_inbound(payload), FilterDisposition::Pass);
}

#[test]
fn adversarial_unicode_field_names() {
    // Note: sonic_rs handles UTF-8 field names correctly
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: r#"has(name)"#.into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    let payload = br#"{"name":"\u00e9v\u00e9nement","id":1}"#;
    assert_eq!(engine.apply_inbound(payload), FilterDisposition::Drop);
}

#[test]
fn adversarial_deeply_nested_path() {
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: r#"a.b.c.d == "deep""#.into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    let payload = br#"{"a":{"b":{"c":{"d":"deep"}}}}"#;
    assert_eq!(engine.apply_inbound(payload), FilterDisposition::Drop);

    let shallow = br#"{"a":{"b":"leaf"}}"#;
    assert_eq!(engine.apply_inbound(shallow), FilterDisposition::Pass);
}

#[test]
fn adversarial_msgpack_bypasses_filters() {
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: "has(_table)".into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    // MsgPack fixmap with _table key — should bypass filter (not crash)
    let msgpack = &[
        0x81, 0xa6, 0x5f, 0x74, 0x61, 0x62, 0x6c, 0x65, 0xa6, 0x65, 0x76, 0x65, 0x6e, 0x74, 0x73,
    ];
    assert_eq!(engine.apply_inbound(msgpack), FilterDisposition::Pass);
}

// ============================================================================
// Section 5: Engine API Tests
// ============================================================================

#[test]
fn engine_empty_has_no_overhead() {
    let engine = TransportFilterEngine::empty();
    assert!(!engine.has_inbound_filters());
    assert!(!engine.has_outbound_filters());
    assert!(!engine.has_dlq_filters_in());
    assert!(!engine.has_dlq_filters_out());
    assert_eq!(
        engine.apply_inbound(br#"{"any":"thing"}"#),
        FilterDisposition::Pass
    );
}

#[test]
fn engine_direction_independence() {
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: "has(in_only)".into(),
            action: FilterAction::Drop,
        }],
        &[FilterRule {
            expression: "has(out_only)".into(),
            action: FilterAction::Drop,
        }],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    assert!(engine.has_inbound_filters());
    assert!(engine.has_outbound_filters());

    // in_only field: dropped inbound, passes outbound
    let payload = br#"{"in_only":true}"#;
    assert_eq!(engine.apply_inbound(payload), FilterDisposition::Drop);
    assert_eq!(engine.apply_outbound(payload), FilterDisposition::Pass);

    // out_only field: passes inbound, dropped outbound
    let payload = br#"{"out_only":true}"#;
    assert_eq!(engine.apply_inbound(payload), FilterDisposition::Pass);
    assert_eq!(engine.apply_outbound(payload), FilterDisposition::Drop);
}

#[test]
fn engine_dlq_filter_detection() {
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: "has(x)".into(),
            action: FilterAction::Dlq,
        }],
        &[FilterRule {
            expression: "has(y)".into(),
            action: FilterAction::Drop,
        }],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    assert!(engine.has_dlq_filters_in());
    assert!(!engine.has_dlq_filters_out());
}

#[test]
fn config_deserializes_from_yaml() {
    let yaml = r#"
filters_in:
  - expression: 'has(_table)'
    action: drop
  - expression: 'status == "poison"'
    action: dlq
filters_out:
  - expression: 'has(debug)'
    action: drop
"#;

    #[derive(serde::Deserialize)]
    struct TestConfig {
        #[serde(default)]
        filters_in: Vec<FilterRule>,
        #[serde(default)]
        filters_out: Vec<FilterRule>,
    }

    let config: TestConfig = serde_yaml_ng::from_str(yaml).unwrap();
    assert_eq!(config.filters_in.len(), 2);
    assert_eq!(config.filters_out.len(), 1);
    assert_eq!(config.filters_in[0].expression, "has(_table)");
    assert_eq!(config.filters_in[0].action, FilterAction::Drop);
    assert_eq!(config.filters_in[1].action, FilterAction::Dlq);
}

#[test]
fn tier_config_deserializes_from_yaml() {
    let yaml = r#"
allow_cel_filters_in: true
allow_complex_filters_out: true
"#;
    let config: TransportFilterTierConfig = serde_yaml_ng::from_str(yaml).unwrap();
    assert!(config.allow_cel_filters_in);
    assert!(!config.allow_cel_filters_out);
    assert!(!config.allow_complex_filters_in);
    assert!(config.allow_complex_filters_out);
}

#[test]
fn empty_filters_config_deserializes() {
    let yaml = "{}";
    #[derive(serde::Deserialize)]
    struct TestConfig {
        #[serde(default)]
        filters_in: Vec<FilterRule>,
    }
    let config: TestConfig = serde_yaml_ng::from_str(yaml).unwrap();
    assert!(config.filters_in.is_empty());
}

// ============================================================================
// Section 6: Tier Classification Verification
// ============================================================================

#[test]
fn tier1_patterns_all_accepted_by_default() {
    let expressions = [
        "has(field)",
        "!has(field)",
        r#"field == "value""#,
        r#"field != "value""#,
        r#"field.startsWith("prefix")"#,
        r#"field.endsWith("suffix")"#,
        r#"field.contains("sub")"#,
        r#"nested.path == "value""#,
        r#"a.b.c.d == "deep""#,
    ];

    for expr in &expressions {
        let result = TransportFilterEngine::new(
            &[FilterRule {
                expression: (*expr).into(),
                action: FilterAction::Drop,
            }],
            &[],
            &TransportFilterTierConfig::default(),
        );
        assert!(
            result.is_ok(),
            "Tier 1 expression should be accepted by default: {expr}"
        );
    }
}

#[test]
fn tier2_patterns_rejected_by_default() {
    let expressions = [
        r#"severity > 3 && source != "internal""#,
        r#"count >= 100"#,
        r#"a == b"#, // field-to-field comparison
    ];

    for expr in &expressions {
        let result = TransportFilterEngine::new(
            &[FilterRule {
                expression: (*expr).into(),
                action: FilterAction::Drop,
            }],
            &[],
            &TransportFilterTierConfig::default(),
        );
        assert!(
            result.is_err(),
            "Tier 2 expression should be rejected by default: {expr}"
        );
    }
}

// ============================================================================
// Section 7: Tier 2/3 CEL Evaluation Tests (requires expression feature)
// ============================================================================

#[cfg(feature = "expression")]
#[test]
fn tier2_compound_expression_evaluates_correctly() {
    let tier_config = TransportFilterTierConfig {
        allow_cel_filters_in: true,
        ..Default::default()
    };
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: r#"severity > 3 && source != "internal""#.into(),
            action: FilterAction::Dlq,
        }],
        &[],
        &tier_config,
    )
    .unwrap();

    // Matches: severity > 3 AND source != "internal"
    let match_payload = br#"{"severity":5,"source":"external"}"#;
    assert_eq!(engine.apply_inbound(match_payload), FilterDisposition::Dlq);

    // No match: severity not > 3
    let no_match = br#"{"severity":1,"source":"external"}"#;
    assert_eq!(engine.apply_inbound(no_match), FilterDisposition::Pass);

    // No match: source IS "internal"
    let internal = br#"{"severity":10,"source":"internal"}"#;
    assert_eq!(engine.apply_inbound(internal), FilterDisposition::Pass);
}

#[cfg(feature = "expression")]
#[test]
fn tier2_size_function() {
    let tier_config = TransportFilterTierConfig {
        allow_cel_filters_in: true,
        ..Default::default()
    };
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: "size(items) > 0".into(),
            action: FilterAction::Drop,
        }],
        &[],
        &tier_config,
    )
    .unwrap();

    let with_items = br#"{"items":["a","b","c"]}"#;
    assert_eq!(engine.apply_inbound(with_items), FilterDisposition::Drop);

    let empty_items = br#"{"items":[]}"#;
    assert_eq!(engine.apply_inbound(empty_items), FilterDisposition::Pass);
}

#[cfg(feature = "expression")]
#[test]
fn tier2_field_to_field_comparison() {
    let tier_config = TransportFilterTierConfig {
        allow_cel_filters_in: true,
        ..Default::default()
    };
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: "expected == actual".into(),
            action: FilterAction::Drop,
        }],
        &[],
        &tier_config,
    )
    .unwrap();

    let matching = br#"{"expected":"x","actual":"x"}"#;
    assert_eq!(engine.apply_inbound(matching), FilterDisposition::Drop);

    let mismatched = br#"{"expected":"x","actual":"y"}"#;
    assert_eq!(engine.apply_inbound(mismatched), FilterDisposition::Pass);
}

#[cfg(feature = "expression")]
#[test]
fn tier3_regex_evaluates_correctly() {
    let tier_config = TransportFilterTierConfig {
        allow_complex_filters_in: true,
        ..Default::default()
    };
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: r#"host.matches("^prod-.*$")"#.into(),
            action: FilterAction::Drop,
        }],
        &[],
        &tier_config,
    )
    .unwrap();

    let prod = br#"{"host":"prod-web01"}"#;
    assert_eq!(engine.apply_inbound(prod), FilterDisposition::Drop);

    let dev = br#"{"host":"dev-web01"}"#;
    assert_eq!(engine.apply_inbound(dev), FilterDisposition::Pass);
}

#[cfg(feature = "expression")]
#[test]
fn tier2_missing_field_safe() {
    let tier_config = TransportFilterTierConfig {
        allow_cel_filters_in: true,
        ..Default::default()
    };
    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: r#"severity > 3 && source != "internal""#.into(),
            action: FilterAction::Drop,
        }],
        &[],
        &tier_config,
    )
    .unwrap();

    // Missing severity field — should NOT match (evaluate_condition returns false on missing)
    let no_severity = br#"{"source":"external"}"#;
    assert_eq!(engine.apply_inbound(no_severity), FilterDisposition::Pass);
}

#[test]
fn tier3_patterns_rejected_by_default() {
    let expressions = [
        r#"field.matches("^prod-.*")"#,
        r#"tags.exists(t, t == "pii")"#,
    ];

    for expr in &expressions {
        let result = TransportFilterEngine::new(
            &[FilterRule {
                expression: (*expr).into(),
                action: FilterAction::Drop,
            }],
            &[],
            &TransportFilterTierConfig::default(),
        );
        assert!(
            result.is_err(),
            "Tier 3 expression should be rejected by default: {expr}"
        );
    }
}
