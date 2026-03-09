// Project:   hyperi-rustlib
// File:      src/metrics/otel.rs
// Purpose:   OTel MeterProvider setup and recorder installation
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! OpenTelemetry metrics setup.
//!
//! Configures an OTel `SdkMeterProvider` with an OTLP periodic exporter
//! via the `metrics-exporter-opentelemetry` crate. When both the `metrics`
//! (Prometheus) and `otel-metrics` features are enabled, a `Fanout` recorder
//! sends each measurement to both backends.

use std::time::Duration;

use opentelemetry_otlp::WithExportConfig;

use super::MetricsError;
use super::otel_types::{OtelMetricsConfig, OtelProtocol};

/// Resolved OTLP configuration after applying env var overrides.
struct ResolvedOtelConfig {
    endpoint: String,
    protocol: OtelProtocol,
    export_interval: Duration,
    service_name: String,
}

/// Resolve OTel config with env var overrides.
fn resolve_config(config: &OtelMetricsConfig) -> ResolvedOtelConfig {
    let endpoint =
        std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").unwrap_or_else(|_| config.endpoint.clone());

    let protocol = std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL")
        .ok()
        .and_then(|p| match p.as_str() {
            "grpc" => Some(OtelProtocol::Grpc),
            "http/protobuf" | "http" => Some(OtelProtocol::Http),
            _ => None,
        })
        .unwrap_or(config.protocol);

    let export_interval = std::env::var("OTEL_METRIC_EXPORT_INTERVAL")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or_else(
            || Duration::from_secs(config.export_interval_secs),
            Duration::from_millis,
        );

    let service_name =
        std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| config.service_name.clone());

    ResolvedOtelConfig {
        endpoint,
        protocol,
        export_interval,
        service_name,
    }
}

/// Build the OTLP metric exporter for the given protocol and endpoint.
fn build_otlp_exporter(
    protocol: OtelProtocol,
    endpoint: &str,
) -> Result<opentelemetry_otlp::MetricExporter, MetricsError> {
    match protocol {
        OtelProtocol::Grpc => opentelemetry_otlp::MetricExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .build()
            .map_err(|e| MetricsError::BuildError(format!("OTel gRPC exporter: {e}"))),
        OtelProtocol::Http => opentelemetry_otlp::MetricExporter::builder()
            .with_http()
            .with_endpoint(endpoint)
            .build()
            .map_err(|e| MetricsError::BuildError(format!("OTel HTTP exporter: {e}"))),
    }
}

/// Build the OTel recorder (without installing it globally).
///
/// Uses `metrics-exporter-opentelemetry` to create a `metrics::Recorder`
/// backed by an OTel `SdkMeterProvider` with an OTLP periodic exporter.
///
/// The recorder is returned but NOT installed — the caller decides whether
/// to install it directly or compose it with Prometheus via Fanout.
pub(crate) fn build_otel_recorder(
    scope_name: &str,
    config: &OtelMetricsConfig,
) -> Result<
    (
        metrics_exporter_opentelemetry::Recorder,
        opentelemetry_sdk::metrics::SdkMeterProvider,
    ),
    MetricsError,
> {
    let resolved = resolve_config(config);

    let exporter = build_otlp_exporter(resolved.protocol, &resolved.endpoint)?;
    let reader = opentelemetry_sdk::metrics::PeriodicReader::builder(exporter)
        .with_interval(resolved.export_interval)
        .build();

    // Build resource
    let mut resource_builder = opentelemetry_sdk::Resource::builder();
    if !resolved.service_name.is_empty() {
        resource_builder = resource_builder.with_service_name(resolved.service_name);
    }
    let resource = resource_builder.build();

    // Use with_meter_provider to attach our OTLP reader to the internal provider
    let reader_for_closure = reader;
    let resource_for_closure = resource;

    // build() returns (SdkMeterProvider, Recorder) — provider first
    let (provider, recorder) =
        metrics_exporter_opentelemetry::Recorder::builder(scope_name.to_string())
            .with_meter_provider(move |mpb| {
                mpb.with_reader(reader_for_closure)
                    .with_resource(resource_for_closure)
            })
            .build();

    tracing::info!(
        endpoint = %resolved.endpoint,
        protocol = ?resolved.protocol,
        export_interval_secs = resolved.export_interval.as_secs(),
        "OTel metrics recorder built"
    );

    Ok((recorder, provider))
}
