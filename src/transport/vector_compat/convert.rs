// SPDX-License-Identifier: FSL-1.1-ALv2
// Copyright (c) 2026 HYPERI PTY LIMITED

//! Bidirectional conversion between Vector protobuf types and serde_json::Value.
//!
//! This is the ONLY place Vector's type system touches ours. Everything
//! downstream is pure DFE `Message<serde_json::Value>`.

use super::proto::event;

/// Convert a Vector `EventWrapper` to a JSON value.
///
/// Returns `None` for metrics (we only handle logs and traces).
#[must_use]
pub fn event_wrapper_to_json(wrapper: &event::EventWrapper) -> Option<serde_json::Value> {
    use event::event_wrapper::Event;

    match wrapper.event.as_ref()? {
        Event::Log(log) => Some(log_to_json(log)),
        Event::Trace(trace) => Some(trace_to_json(trace)),
        Event::Metric(_) => None, // DFE doesn't handle metrics
    }
}

/// Convert a JSON value to a Vector `EventWrapper` (as a Log event).
#[must_use]
pub fn json_to_event_wrapper(value: &serde_json::Value) -> event::EventWrapper {
    event::EventWrapper {
        event: Some(event::event_wrapper::Event::Log(json_to_log(value))),
    }
}

/// Convert a Vector `Log` to a JSON value.
///
/// Prefers `log.value` (newer API) over `log.fields` (deprecated).
fn log_to_json(log: &event::Log) -> serde_json::Value {
    // Prefer the unified value field (newer Vector versions)
    if let Some(value) = &log.value {
        return vector_value_to_json(value);
    }

    // Fall back to deprecated fields map
    let mut map = serde_json::Map::with_capacity(log.fields.len());
    for (k, v) in &log.fields {
        map.insert(k.clone(), vector_value_to_json(v));
    }
    serde_json::Value::Object(map)
}

/// Convert a Vector `Trace` to a JSON value.
fn trace_to_json(trace: &event::Trace) -> serde_json::Value {
    let mut map = serde_json::Map::with_capacity(trace.fields.len());
    for (k, v) in &trace.fields {
        map.insert(k.clone(), vector_value_to_json(v));
    }
    serde_json::Value::Object(map)
}

/// Convert a Vector protobuf `Value` to a `serde_json::Value`.
///
/// Handles all variants: bytes, timestamp, integer, float, bool, map, array, null.
fn vector_value_to_json(value: &event::Value) -> serde_json::Value {
    use event::value::Kind;

    match &value.kind {
        Some(Kind::RawBytes(bytes)) => {
            // Try UTF-8 first, fall back to lossy conversion
            match std::str::from_utf8(bytes) {
                Ok(s) => serde_json::Value::String(s.to_string()),
                Err(_) => serde_json::Value::String(String::from_utf8_lossy(bytes).into_owned()),
            }
        }
        Some(Kind::Timestamp(ts)) => {
            // Convert protobuf Timestamp to RFC 3339 string
            // Our pipeline's TimestampValidator handles both epoch ms and ISO strings
            let millis = ts.seconds * 1000 + i64::from(ts.nanos) / 1_000_000;
            serde_json::Value::Number(serde_json::Number::from(millis))
        }
        Some(Kind::Integer(i)) => serde_json::json!(*i),
        Some(Kind::Float(f)) => {
            serde_json::Number::from_f64(*f)
                .map_or(serde_json::Value::Null, serde_json::Value::Number)
        }
        Some(Kind::Boolean(b)) => serde_json::Value::Bool(*b),
        Some(Kind::Map(map)) => {
            let mut obj = serde_json::Map::with_capacity(map.fields.len());
            for (k, v) in &map.fields {
                obj.insert(k.clone(), vector_value_to_json(v));
            }
            serde_json::Value::Object(obj)
        }
        Some(Kind::Array(arr)) => {
            serde_json::Value::Array(arr.items.iter().map(vector_value_to_json).collect())
        }
        Some(Kind::Null(_)) | None => serde_json::Value::Null,
    }
}

/// Convert a `serde_json::Value` to a Vector protobuf `Value`.
fn json_to_vector_value(value: &serde_json::Value) -> event::Value {
    use event::value::Kind;

    let kind = match value {
        serde_json::Value::Null => Some(Kind::Null(event::ValueNull::NullValue.into())),
        serde_json::Value::Bool(b) => Some(Kind::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(Kind::Integer(i))
            } else {
                Some(Kind::Float(n.as_f64().unwrap_or(0.0)))
            }
        }
        serde_json::Value::String(s) => Some(Kind::RawBytes(s.as_bytes().to_vec())),
        serde_json::Value::Array(arr) => Some(Kind::Array(event::ValueArray {
            items: arr.iter().map(json_to_vector_value).collect(),
        })),
        serde_json::Value::Object(map) => Some(Kind::Map(event::ValueMap {
            fields: map
                .iter()
                .map(|(k, v)| (k.clone(), json_to_vector_value(v)))
                .collect(),
        })),
    };

    event::Value { kind }
}

/// Convert a `serde_json::Value` to a Vector `Log` message.
#[allow(deprecated)]
fn json_to_log(value: &serde_json::Value) -> event::Log {
    event::Log {
        fields: std::collections::HashMap::new(), // Deprecated, leave empty
        value: Some(json_to_vector_value(value)),
        metadata: None,
        metadata_full: None,
    }
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;

    #[test]
    fn convert_string_value() {
        let v = event::Value {
            kind: Some(event::value::Kind::RawBytes(b"hello".to_vec())),
        };
        assert_eq!(vector_value_to_json(&v), serde_json::json!("hello"));
    }

    #[test]
    fn convert_integer_value() {
        let v = event::Value {
            kind: Some(event::value::Kind::Integer(42)),
        };
        assert_eq!(vector_value_to_json(&v), serde_json::json!(42));
    }

    #[test]
    fn convert_float_value() {
        let v = event::Value {
            kind: Some(event::value::Kind::Float(3.14)),
        };
        assert_eq!(vector_value_to_json(&v), serde_json::json!(3.14));
    }

    #[test]
    fn convert_bool_value() {
        let v = event::Value {
            kind: Some(event::value::Kind::Boolean(true)),
        };
        assert_eq!(vector_value_to_json(&v), serde_json::json!(true));
    }

    #[test]
    fn convert_null_value() {
        let v = event::Value {
            kind: Some(event::value::Kind::Null(0)),
        };
        assert_eq!(vector_value_to_json(&v), serde_json::Value::Null);
    }

    #[test]
    fn convert_none_value() {
        let v = event::Value { kind: None };
        assert_eq!(vector_value_to_json(&v), serde_json::Value::Null);
    }

    #[test]
    fn convert_timestamp_to_millis() {
        let v = event::Value {
            kind: Some(event::value::Kind::Timestamp(prost_types::Timestamp {
                seconds: 1_700_000_000,
                nanos: 500_000_000,
            })),
        };
        // 1700000000 * 1000 + 500 = 1700000000500
        assert_eq!(
            vector_value_to_json(&v),
            serde_json::json!(1_700_000_000_500_i64)
        );
    }

    #[test]
    fn convert_map_value() {
        let mut fields = std::collections::HashMap::new();
        fields.insert(
            "name".to_string(),
            event::Value {
                kind: Some(event::value::Kind::RawBytes(b"alice".to_vec())),
            },
        );
        fields.insert(
            "age".to_string(),
            event::Value {
                kind: Some(event::value::Kind::Integer(30)),
            },
        );

        let v = event::Value {
            kind: Some(event::value::Kind::Map(event::ValueMap { fields })),
        };

        let result = vector_value_to_json(&v);
        assert_eq!(result["name"], serde_json::json!("alice"));
        assert_eq!(result["age"], serde_json::json!(30));
    }

    #[test]
    fn convert_array_value() {
        let v = event::Value {
            kind: Some(event::value::Kind::Array(event::ValueArray {
                items: vec![
                    event::Value {
                        kind: Some(event::value::Kind::Integer(1)),
                    },
                    event::Value {
                        kind: Some(event::value::Kind::Integer(2)),
                    },
                ],
            })),
        };

        assert_eq!(vector_value_to_json(&v), serde_json::json!([1, 2]));
    }

    #[test]
    fn convert_log_with_value_field() {
        let log = event::Log {
            fields: std::collections::HashMap::new(),
            value: Some(event::Value {
                kind: Some(event::value::Kind::Map(event::ValueMap {
                    fields: {
                        let mut m = std::collections::HashMap::new();
                        m.insert(
                            "message".to_string(),
                            event::Value {
                                kind: Some(event::value::Kind::RawBytes(b"test log".to_vec())),
                            },
                        );
                        m
                    },
                })),
            }),
            metadata: None,
            metadata_full: None,
        };

        let json = log_to_json(&log);
        assert_eq!(json["message"], serde_json::json!("test log"));
    }

    #[test]
    fn convert_log_with_deprecated_fields() {
        let mut fields = std::collections::HashMap::new();
        fields.insert(
            "host".to_string(),
            event::Value {
                kind: Some(event::value::Kind::RawBytes(b"server-1".to_vec())),
            },
        );

        let log = event::Log {
            fields,
            value: None, // No unified value — use deprecated fields
            metadata: None,
            metadata_full: None,
        };

        let json = log_to_json(&log);
        assert_eq!(json["host"], serde_json::json!("server-1"));
    }

    #[test]
    fn event_wrapper_log_roundtrip() {
        let original = serde_json::json!({
            "message": "hello world",
            "level": "info",
            "count": 42
        });

        let wrapper = json_to_event_wrapper(&original);
        let result = event_wrapper_to_json(&wrapper).unwrap();

        assert_eq!(result["message"], serde_json::json!("hello world"));
        assert_eq!(result["level"], serde_json::json!("info"));
        assert_eq!(result["count"], serde_json::json!(42));
    }

    #[test]
    fn event_wrapper_metric_returns_none() {
        let wrapper = event::EventWrapper {
            event: Some(event::event_wrapper::Event::Metric(event::Metric {
                name: "cpu".to_string(),
                ..Default::default()
            })),
        };

        assert!(event_wrapper_to_json(&wrapper).is_none());
    }

    #[test]
    fn event_wrapper_empty_returns_none() {
        let wrapper = event::EventWrapper { event: None };
        assert!(event_wrapper_to_json(&wrapper).is_none());
    }

    #[test]
    fn nan_float_becomes_null() {
        let v = event::Value {
            kind: Some(event::value::Kind::Float(f64::NAN)),
        };
        assert_eq!(vector_value_to_json(&v), serde_json::Value::Null);
    }

    #[test]
    fn utf8_lossy_fallback() {
        // Invalid UTF-8 sequence
        let v = event::Value {
            kind: Some(event::value::Kind::RawBytes(vec![0xff, 0xfe, 0x41])),
        };
        let result = vector_value_to_json(&v);
        assert!(result.is_string());
        // Should contain the replacement character
        assert!(result.as_str().unwrap().contains('\u{FFFD}'));
    }
}
