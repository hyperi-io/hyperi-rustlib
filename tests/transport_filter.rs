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

// ============================================================================
// Section 8: take_filtered_dlq_entries() — DLQ buffering integration
// ============================================================================

#[tokio::test]
async fn dlq_filter_entries_exposed_via_take() {
    let transport = transport_with_inbound_filters(vec![FilterRule {
        expression: r#"status == "poison""#.into(),
        action: FilterAction::Dlq,
    }]);

    transport
        .inject(None, br#"{"status":"ok","id":1}"#.to_vec())
        .await
        .unwrap();
    transport
        .inject(None, br#"{"status":"poison","id":2}"#.to_vec())
        .await
        .unwrap();
    transport
        .inject(None, br#"{"status":"poison","id":3}"#.to_vec())
        .await
        .unwrap();

    let messages = transport.recv(10).await.unwrap();
    assert_eq!(
        messages.len(),
        1,
        "Only the non-poison message should be in result"
    );

    // The DLQ entries should be exposed via take_filtered_dlq_entries
    let dlq_entries = transport.take_filtered_dlq_entries();
    assert_eq!(dlq_entries.len(), 2, "Two DLQ entries should be staged");
    assert!(dlq_entries[0].payload.windows(6).any(|w| w == b"poison"));
    assert!(dlq_entries[1].payload.windows(6).any(|w| w == b"poison"));
}

#[tokio::test]
async fn take_filtered_dlq_entries_drains_buffer() {
    let transport = transport_with_inbound_filters(vec![FilterRule {
        expression: "has(_internal)".into(),
        action: FilterAction::Dlq,
    }]);

    transport
        .inject(None, br#"{"_internal":true}"#.to_vec())
        .await
        .unwrap();
    let _ = transport.recv(10).await.unwrap();

    // First take returns the entry
    let first = transport.take_filtered_dlq_entries();
    assert_eq!(first.len(), 1);

    // Second take returns empty (buffer drained)
    let second = transport.take_filtered_dlq_entries();
    assert!(second.is_empty(), "Buffer should be drained after take");
}

#[tokio::test]
async fn drop_filter_does_not_populate_dlq_buffer() {
    let transport = transport_with_inbound_filters(vec![FilterRule {
        expression: r#"status == "drop_me""#.into(),
        action: FilterAction::Drop,
    }]);

    transport
        .inject(None, br#"{"status":"drop_me"}"#.to_vec())
        .await
        .unwrap();
    transport
        .inject(None, br#"{"status":"ok"}"#.to_vec())
        .await
        .unwrap();

    let messages = transport.recv(10).await.unwrap();
    assert_eq!(messages.len(), 1);

    // Drop action should NOT populate the DLQ buffer
    let dlq_entries = transport.take_filtered_dlq_entries();
    assert!(
        dlq_entries.is_empty(),
        "Drop action should not populate DLQ buffer"
    );
}

#[tokio::test]
async fn no_filters_no_dlq_buffer_overhead() {
    let transport = transport_no_filters();
    transport
        .inject(None, br#"{"any":"thing"}"#.to_vec())
        .await
        .unwrap();
    let _ = transport.recv(10).await.unwrap();
    let dlq_entries = transport.take_filtered_dlq_entries();
    assert!(dlq_entries.is_empty());
}

// ============================================================================
// Section 9: Memmem false positive (documented limitation)
// ============================================================================

#[test]
fn memmem_false_positive_nested_field_matches_top_level_filter() {
    // KNOWN LIMITATION: the memmem fast-path for `has(<single-field>)` searches
    // for the literal `"<field>":` byte pattern anywhere in the payload. It does
    // NOT verify that the field appears at the TOP LEVEL of the JSON object.
    //
    // If the same field name occurs at a nested level, the fast-path will match
    // even though a strict CEL `has()` would not.
    //
    // Example: filter is `has(_table)` (top-level), payload is
    //   {"data":{"_table":"events"}}
    // The bytes contain `"_table":`, so memmem matches and the filter triggers.
    //
    // This is a deliberate tradeoff for the ~50% performance gain on the most
    // common transport filter (existence checks on top-level routing fields).
    // Workaround for users who need strict top-level matching: use a nested
    // path like `has(some.nested._table)` which forces the slower sonic-rs path.

    let engine = TransportFilterEngine::new(
        &[FilterRule {
            expression: "has(_table)".into(),
            action: FilterAction::Drop,
        }],
        &[],
        &TransportFilterTierConfig::default(),
    )
    .unwrap();

    // Real top-level field — correct match
    let real_match = br#"{"_table":"events"}"#;
    assert_eq!(engine.apply_inbound(real_match), FilterDisposition::Drop);

    // Nested field at non-top-level — documented false positive
    let nested_payload = br#"{"data":{"_table":"events"}}"#;
    assert_eq!(
        engine.apply_inbound(nested_payload),
        FilterDisposition::Drop,
        "Documented false positive: memmem fast-path matches nested occurrences"
    );

    // Sound case: well-formed JSON with field name only inside an escaped
    // string value never triggers a false positive — JSON encoding requires
    // a `\` before any embedded `\"`, so the literal byte pattern `"_table":`
    // never appears inside a string value.
    let escaped_in_value = br#"{"description":"event with \"_table\": substring"}"#;
    assert_eq!(
        engine.apply_inbound(escaped_in_value),
        FilterDisposition::Pass,
        "Escaped quotes prevent the literal `\"_table\":` pattern from appearing in a string value"
    );
}

// ============================================================================
// Section 10: Concurrency (Send + Sync via tokio::spawn)
// ============================================================================

#[tokio::test]
async fn engine_send_sync_concurrent_evaluation() {
    use std::sync::Arc;

    let engine = Arc::new(
        TransportFilterEngine::new(
            &[FilterRule {
                expression: r#"status == "poison""#.into(),
                action: FilterAction::Drop,
            }],
            &[],
            &TransportFilterTierConfig::default(),
        )
        .unwrap(),
    );

    let mut handles = Vec::new();
    for i in 0..32 {
        let engine = Arc::clone(&engine);
        handles.push(tokio::spawn(async move {
            let mut drops = 0u32;
            let mut passes = 0u32;
            for j in 0..1000 {
                let payload = if j % 3 == 0 {
                    br#"{"status":"poison","id":1}"#.to_vec()
                } else {
                    format!(r#"{{"id":{j},"thread":{i}}}"#).into_bytes()
                };
                match engine.apply_inbound(&payload) {
                    FilterDisposition::Drop => drops += 1,
                    FilterDisposition::Pass => passes += 1,
                    FilterDisposition::Dlq => {}
                }
            }
            (drops, passes)
        }));
    }

    let mut total_drops = 0u32;
    let mut total_passes = 0u32;
    for h in handles {
        let (d, p) = h.await.unwrap();
        total_drops += d;
        total_passes += p;
    }

    // 32 threads × 1000 messages = 32000 total
    assert_eq!(total_drops + total_passes, 32_000);
    // ~33% are poison
    assert!(total_drops > 10_000 && total_drops < 12_000);
}

#[test]
fn filter_action_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<TransportFilterEngine>();
    assert_send_sync::<FilterRule>();
    assert_send_sync::<FilterAction>();
    assert_send_sync::<FilterDisposition>();
}

// ============================================================================
// Section 11: Per-transport smoke (verify field_engine field exists in all)
// ============================================================================

#[tokio::test]
async fn smoke_memory_transport_filters_field_present() {
    // Construct MemoryTransport with filter config — verifies field exists
    let transport = MemoryTransport::new(&MemoryConfig {
        buffer_size: 100,
        recv_timeout_ms: 50,
        filters_in: vec![FilterRule {
            expression: "has(_drop_me)".into(),
            action: FilterAction::Drop,
        }],
        filters_out: vec![FilterRule {
            expression: "has(_drop_me)".into(),
            action: FilterAction::Drop,
        }],
    });

    // Filter actually works
    transport
        .inject(None, br#"{"_drop_me":true}"#.to_vec())
        .await
        .unwrap();
    transport
        .inject(None, br#"{"keep":true}"#.to_vec())
        .await
        .unwrap();

    let messages = transport.recv(10).await.unwrap();
    assert_eq!(messages.len(), 1, "Filter must be wired in MemoryTransport");
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

// ============================================================================
// Section 12: Python <-> Rust classifier parity (shared fixture)
// ============================================================================
//
// Loads tests/fixtures/cel_classifier_parity.json and verifies the Rust
// classifier produces the same tier, op, and field results as the fixture
// expects. The dfe-engine Python test in
// `/projects/dfe-engine/tests/unit/test_cel/test_parity.py` runs the SAME
// fixture through the Python classifier in `dfe_engine.cel.classify`.
//
// If both tests pass on their respective sides, the UI validator and the
// runtime engine agree on classification — no drift.
//
// To add a new test case, edit the fixture in BOTH:
//   * /projects/hyperi-rustlib/tests/fixtures/cel_classifier_parity.json
//   * /projects/dfe-engine/tests/fixtures/cel_classifier_parity.json
// They must remain byte-identical.

#[test]
fn classifier_matches_python_fixture() {
    use hyperi_rustlib::transport::filter::classify::{ClassifyResult, Tier1Op, classify};

    #[derive(serde::Deserialize)]
    struct Fixture {
        cases: Vec<Case>,
    }

    #[derive(serde::Deserialize)]
    struct Case {
        expression: String,
        tier: u8,
        #[serde(default)]
        op_kind: Option<String>,
        #[serde(default)]
        op_field: Option<String>,
        #[serde(default)]
        op_value: Option<String>,
        #[serde(default)]
        fields: Option<Vec<String>>,
    }

    let raw =
        std::fs::read_to_string("tests/fixtures/cel_classifier_parity.json").expect("read fixture");
    let fixture: Fixture = serde_json::from_str(&raw).expect("parse fixture");

    for case in &fixture.cases {
        let result = classify(&case.expression)
            .unwrap_or_else(|e| panic!("classify failed for {:?}: {}", case.expression, e));

        let actual_tier_num: u8 = match result.tier() {
            hyperi_rustlib::transport::filter::FilterTier::Tier1 => 1,
            hyperi_rustlib::transport::filter::FilterTier::Tier2 => 2,
            hyperi_rustlib::transport::filter::FilterTier::Tier3 => 3,
        };
        assert_eq!(
            actual_tier_num, case.tier,
            "tier mismatch for {:?}: expected={} actual={}",
            case.expression, case.tier, actual_tier_num
        );

        if case.tier == 1 {
            let ClassifyResult::Tier1(op) = &result else {
                panic!(
                    "Tier 1 expected for {:?}, got {:?}",
                    case.expression, result
                );
            };
            let (kind, field, value) = match op {
                Tier1Op::FieldExists { field } => ("field_exists", field.as_str(), None),
                Tier1Op::FieldNotExists { field } => ("field_not_exists", field.as_str(), None),
                Tier1Op::FieldEquals { field, value } => {
                    ("field_equals", field.as_str(), Some(value.as_str()))
                }
                Tier1Op::FieldNotEquals { field, value } => {
                    ("field_not_equals", field.as_str(), Some(value.as_str()))
                }
                Tier1Op::FieldStartsWith { field, prefix } => {
                    ("field_starts_with", field.as_str(), Some(prefix.as_str()))
                }
                Tier1Op::FieldEndsWith { field, suffix } => {
                    ("field_ends_with", field.as_str(), Some(suffix.as_str()))
                }
                Tier1Op::FieldContains { field, substring } => {
                    ("field_contains", field.as_str(), Some(substring.as_str()))
                }
            };
            let expected_kind = case.op_kind.as_deref().unwrap_or_else(|| {
                panic!(
                    "fixture missing op_kind for Tier 1 case {:?}",
                    case.expression
                )
            });
            assert_eq!(
                kind, expected_kind,
                "op_kind mismatch for {:?}",
                case.expression
            );
            let expected_field = case.op_field.as_deref().unwrap_or_else(|| {
                panic!(
                    "fixture missing op_field for Tier 1 case {:?}",
                    case.expression
                )
            });
            assert_eq!(
                field, expected_field,
                "op_field mismatch for {:?}",
                case.expression
            );
            if let Some(expected_value) = case.op_value.as_deref() {
                assert_eq!(
                    value,
                    Some(expected_value),
                    "op_value mismatch for {:?}",
                    case.expression
                );
            }
        } else {
            let actual_fields: Vec<String> = match &result {
                ClassifyResult::Tier2 { fields } | ClassifyResult::Tier3 { fields } => {
                    fields.clone()
                }
                ClassifyResult::Tier1(_) => unreachable!(),
            };
            let mut actual_sorted = actual_fields.clone();
            actual_sorted.sort();
            let mut expected_sorted = case.fields.clone().unwrap_or_default();
            expected_sorted.sort();
            assert_eq!(
                actual_sorted, expected_sorted,
                "fields mismatch for {:?}",
                case.expression
            );
        }
    }
}
