// Project:   hyperi-rustlib
// File:      src/otel_tracing/mod.rs
// Purpose:   OpenTelemetry trace span exporter (OTLP) + tracing-subscriber bridge
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! OpenTelemetry distributed tracing — span export via OTLP.
//!
//! Bridges the `tracing` ecosystem (`tracing::span!`, `#[instrument]`,
//! `tracing::info_span!`) to OpenTelemetry spans that get exported via
//! OTLP to a collector or backend (Tempo, Jaeger, Honeycomb, etc.).
//!
//! Closes the loop on the framework's W3C traceparent propagation:
//! [`crate::transport::propagation`] reads the current OTel context
//! (set externally) and propagates it across transport boundaries.
//! Without this module wired up, internal `tracing::span!`s never become
//! OTel spans, leaving distributed traces with broken segments.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use hyperi_rustlib::otel_tracing::{OtelTracingConfig, build_tracer_layer};
//! use tracing_subscriber::layer::SubscriberExt;
//! use tracing_subscriber::util::SubscriberInitExt;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let config = OtelTracingConfig {
//!     service_name: "dfe-loader".into(),
//!     endpoint: "http://otel-collector:4317".into(),
//!     ..Default::default()
//! };
//! let (otel_layer, _provider) = build_tracer_layer(&config)?;
//!
//! tracing_subscriber::registry()
//!     .with(tracing_subscriber::fmt::layer())
//!     .with(otel_layer)
//!     .init();
//!
//! tracing::info_span!("startup").in_scope(|| {
//!     tracing::info!("application booted");
//! });
//! # Ok(())
//! # }
//! ```
//!
//! # Why a separate module from `otel-metrics`
//!
//! Metrics and traces have independent lifecycles, samplers, and exporters.
//! Mixing them under one feature gate forced consumers who only want one
//! to pull in the other. They share `OtelProtocol` and the OTLP endpoint
//! discipline but otherwise operate independently.

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::SdkTracerProvider;
use serde::{Deserialize, Serialize};
use tracing_opentelemetry::OpenTelemetryLayer;

/// OTLP transport protocol (mirrors [`crate::metrics::OtelProtocol`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OtelTracingProtocol {
    /// gRPC with tonic (default; OTLP-native, lowest overhead).
    #[default]
    Grpc,
    /// HTTP with protobuf body.
    Http,
}

/// OpenTelemetry tracing configuration.
///
/// Resolves env-var overrides at build time:
/// - `OTEL_EXPORTER_OTLP_ENDPOINT` overrides `endpoint`
/// - `OTEL_EXPORTER_OTLP_PROTOCOL` (`grpc` | `http/protobuf` | `http`) overrides `protocol`
/// - `OTEL_SERVICE_NAME` overrides `service_name`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OtelTracingConfig {
    /// OTLP endpoint (default `http://localhost:4317` for gRPC).
    pub endpoint: String,
    /// Wire protocol.
    pub protocol: OtelTracingProtocol,
    /// `service.name` resource attribute.
    pub service_name: String,
    /// Batch exporter scheduled-delay (milliseconds).
    pub batch_scheduled_delay_ms: u64,
    /// Batch exporter max queue size.
    pub batch_max_queue_size: usize,
}

impl Default for OtelTracingConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://localhost:4317".into(),
            protocol: OtelTracingProtocol::Grpc,
            service_name: env!("CARGO_PKG_NAME").into(),
            batch_scheduled_delay_ms: 5_000,
            batch_max_queue_size: 2_048,
        }
    }
}

/// Errors when building the OTel tracer.
#[derive(Debug, thiserror::Error)]
pub enum OtelTracingError {
    /// OTLP exporter construction failed.
    #[error("OTLP {protocol:?} span exporter: {source}")]
    ExporterBuild {
        /// The protocol attempted.
        protocol: OtelTracingProtocol,
        /// Underlying error.
        source: opentelemetry_otlp::ExporterBuildError,
    },
}

fn resolve(config: &OtelTracingConfig) -> OtelTracingConfig {
    let mut resolved = config.clone();
    if let Ok(v) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
        resolved.endpoint = v;
    }
    if let Ok(v) = std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL") {
        resolved.protocol = match v.as_str() {
            "http/protobuf" | "http" => OtelTracingProtocol::Http,
            _ => OtelTracingProtocol::Grpc,
        };
    }
    if let Ok(v) = std::env::var("OTEL_SERVICE_NAME") {
        resolved.service_name = v;
    }
    resolved
}

fn build_span_exporter(
    protocol: OtelTracingProtocol,
    endpoint: &str,
) -> Result<opentelemetry_otlp::SpanExporter, OtelTracingError> {
    let result = match protocol {
        OtelTracingProtocol::Grpc => opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(endpoint)
            .build(),
        OtelTracingProtocol::Http => opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(endpoint)
            .build(),
    };
    result.map_err(|source| OtelTracingError::ExporterBuild { protocol, source })
}

/// Build an OTel tracer + tracing-subscriber layer ready for composition.
///
/// Sets the resulting [`SdkTracerProvider`] as the **global** tracer
/// provider (so [`crate::transport::propagation`] picks it up). The
/// returned layer should be added to a `tracing_subscriber::Registry`.
///
/// The provider is also returned so callers can `.shutdown()` it on
/// graceful exit (otherwise the batch exporter loses queued spans).
///
/// # Errors
///
/// Returns [`OtelTracingError::ExporterBuild`] if the OTLP exporter
/// cannot be initialised (typically endpoint format / TLS setup issues).
pub fn build_tracer_layer<S>(
    config: &OtelTracingConfig,
) -> Result<
    (
        OpenTelemetryLayer<S, opentelemetry_sdk::trace::Tracer>,
        SdkTracerProvider,
    ),
    OtelTracingError,
>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let resolved = resolve(config);

    let exporter = build_span_exporter(resolved.protocol, &resolved.endpoint)?;

    let resource = Resource::builder()
        .with_service_name(resolved.service_name.clone())
        .build();

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();

    let tracer = provider.tracer("hyperi-rustlib");

    // Install as global so propagation.rs picks up the active context.
    opentelemetry::global::set_tracer_provider(provider.clone());

    let layer = tracing_opentelemetry::layer().with_tracer(tracer);

    tracing::info!(
        endpoint = %resolved.endpoint,
        protocol = ?resolved.protocol,
        service_name = %resolved.service_name,
        scheduled_delay_ms = resolved.batch_scheduled_delay_ms,
        "OTel tracing layer built"
    );

    Ok((layer, provider))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_round_trip() {
        let cfg = OtelTracingConfig::default();
        assert_eq!(cfg.protocol, OtelTracingProtocol::Grpc);
        assert!(!cfg.endpoint.is_empty());
        assert!(!cfg.service_name.is_empty());
    }

    #[test]
    fn resolve_picks_up_env_overrides() {
        // SAFETY: temp_env handles cleanup; env mutations are scoped.
        temp_env::with_vars(
            [
                (
                    "OTEL_EXPORTER_OTLP_ENDPOINT",
                    Some("http://my-collector:4317"),
                ),
                ("OTEL_EXPORTER_OTLP_PROTOCOL", Some("http/protobuf")),
                ("OTEL_SERVICE_NAME", Some("test-service")),
            ],
            || {
                let r = resolve(&OtelTracingConfig::default());
                assert_eq!(r.endpoint, "http://my-collector:4317");
                assert_eq!(r.protocol, OtelTracingProtocol::Http);
                assert_eq!(r.service_name, "test-service");
            },
        );
    }
}
