// Project:   hyperi-rustlib
// File:      src/metrics/otel_types.rs
// Purpose:   Configuration types for OTel metrics backend
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Configuration types for the OpenTelemetry metrics backend.
//!
//! These types extend `MetricsConfig` with OTel-specific options
//! when the `otel-metrics` feature is enabled.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// OTLP transport protocol.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OtelProtocol {
    /// gRPC (default, port 4317)
    #[default]
    Grpc,
    /// HTTP/protobuf (port 4318)
    Http,
}

impl OtelProtocol {
    /// Default endpoint for this protocol.
    #[must_use]
    pub fn default_endpoint(self) -> &'static str {
        match self {
            Self::Grpc => "http://localhost:4317",
            Self::Http => "http://localhost:4318",
        }
    }
}

/// OTel-specific configuration for the metrics backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OtelMetricsConfig {
    /// OTLP endpoint (default: protocol-dependent).
    ///
    /// Override via `OTEL_EXPORTER_OTLP_ENDPOINT` env var.
    pub endpoint: String,

    /// OTLP transport protocol.
    ///
    /// Override via `OTEL_EXPORTER_OTLP_PROTOCOL` env var.
    pub protocol: OtelProtocol,

    /// Service name reported in OTel resource.
    ///
    /// Override via `OTEL_SERVICE_NAME` env var.
    pub service_name: String,

    /// Additional OTLP headers (e.g. API keys for HyperDX).
    pub headers: HashMap<String, String>,

    /// Additional resource attributes.
    pub resource_attributes: HashMap<String, String>,

    /// Metric export interval in seconds (default: 60).
    ///
    /// Override via `OTEL_METRIC_EXPORT_INTERVAL` env var (in milliseconds).
    pub export_interval_secs: u64,
}

impl Default for OtelMetricsConfig {
    fn default() -> Self {
        Self {
            endpoint: OtelProtocol::Grpc.default_endpoint().to_string(),
            protocol: OtelProtocol::default(),
            service_name: String::new(),
            headers: HashMap::new(),
            resource_attributes: HashMap::new(),
            export_interval_secs: 60,
        }
    }
}
