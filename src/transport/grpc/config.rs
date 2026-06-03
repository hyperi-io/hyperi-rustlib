// Project:   hyperi-rustlib
// File:      src/transport/grpc/config.rs
// Purpose:   gRPC transport configuration
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

use serde::{Deserialize, Serialize};

/// gRPC transport configuration.
///
/// Supports client mode (sending), server mode (receiving), or both.
///
/// # Client mode
///
/// Set `endpoint` to connect to a remote DFE gRPC server.
///
/// # Server mode
///
/// Set `listen` to accept incoming Push RPCs.
///
/// # Both
///
/// Set both for bidirectional communication (e.g., a relay node).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GrpcConfig {
    /// Server listen address (e.g., "0.0.0.0:6000").
    /// When set, the transport accepts incoming Push RPCs.
    pub listen: Option<String>,

    /// Client endpoint URI (e.g., "http://dfe-loader:6000").
    /// When set, the transport can send messages to a remote server.
    pub endpoint: Option<String>,

    /// Receive buffer size (messages buffered from incoming RPCs).
    pub recv_buffer_size: usize,

    /// Receive timeout in milliseconds (0 = non-blocking).
    pub recv_timeout_ms: u64,

    /// Per-RPC send deadline in milliseconds (0 = no deadline).
    ///
    /// Bounds a single `push` call so a hung or black-holing server cannot
    /// block a sender task forever. Applied as the gRPC request deadline
    /// (`grpc-timeout` header) on every outbound RPC. Default 30s.
    pub send_timeout_ms: u64,

    /// Maximum message size in bytes (both send and receive).
    pub max_message_size: usize,

    /// Enable gzip compression for gRPC messages.
    pub compression: bool,

    // --- Client TLS (tonic owns its TLS stack -- like Kafka/librdkafka -- so
    // these map the unified TlsTrust vocabulary onto tonic's ClientTlsConfig
    // rather than consuming crate::tls's rustls ClientConfig directly. Note:
    // in-cluster DFE gRPC is usually mesh-mTLS (Istio/Linkerd); set these only
    // for DIRECT TLS to a remote endpoint.) ---
    /// Enable client TLS for the `endpoint` connection.
    #[serde(default)]
    pub tls_enabled: bool,
    /// Private-CA PEM (maps to `TlsTrust.extra_roots`). When unset with
    /// `tls_enabled`, falls back to OS native roots.
    #[serde(default)]
    pub tls_ca_path: Option<String>,
    /// Domain name for SNI / certificate verification (overrides the URI host).
    #[serde(default)]
    pub tls_domain: Option<String>,
    /// Client certificate PEM for mTLS (with `tls_client_key_path`).
    #[serde(default)]
    pub tls_client_cert_path: Option<String>,
    /// Client key PEM for mTLS (with `tls_client_cert_path`).
    #[serde(default)]
    pub tls_client_key_path: Option<String>,

    /// Enable Vector wire protocol compatibility on the same server.
    /// When true, the server also accepts `/vector.Vector/PushEvents` RPCs
    /// from legacy Vector sinks.
    /// Requires the `transport-grpc-vector-compat` feature.
    #[cfg(feature = "transport-grpc-vector-compat")]
    pub vector_compat: bool,

    /// Inbound message filters (applied on recv before caller sees messages).
    pub filters_in: Vec<crate::transport::filter::FilterRule>,

    /// Outbound message filters (applied on send before transport dispatches).
    pub filters_out: Vec<crate::transport::filter::FilterRule>,
}

impl Default for GrpcConfig {
    fn default() -> Self {
        Self {
            listen: None,
            endpoint: None,
            recv_buffer_size: 10_000,
            recv_timeout_ms: 100,
            send_timeout_ms: 30_000, // 30s -- bound a single push RPC
            max_message_size: 16 * 1024 * 1024, // 16 MB
            compression: false,
            tls_enabled: false,
            tls_ca_path: None,
            tls_domain: None,
            tls_client_cert_path: None,
            tls_client_key_path: None,
            #[cfg(feature = "transport-grpc-vector-compat")]
            vector_compat: false,
            filters_in: Vec::new(),
            filters_out: Vec::new(),
        }
    }
}

impl GrpcConfig {
    /// Load from the config cascade under the `grpc` key.
    #[must_use]
    pub fn from_cascade() -> Self {
        #[cfg(feature = "config")]
        {
            if let Some(cfg) = crate::config::try_get()
                && let Ok(grpc) = cfg.unmarshal_key_registered::<Self>("grpc")
            {
                return grpc;
            }
        }
        Self::default()
    }

    /// Create a server-only config.
    #[must_use]
    pub fn server(listen: &str) -> Self {
        Self {
            listen: Some(listen.to_string()),
            ..Default::default()
        }
    }

    /// Create a client-only config.
    #[must_use]
    pub fn client(endpoint: &str) -> Self {
        Self {
            endpoint: Some(endpoint.to_string()),
            ..Default::default()
        }
    }

    /// Enable gzip compression.
    #[must_use]
    pub fn with_compression(mut self) -> Self {
        self.compression = true;
        self
    }

    /// Set max message size.
    #[must_use]
    pub fn with_max_message_size(mut self, size: usize) -> Self {
        self.max_message_size = size;
        self
    }

    /// Enable Vector wire protocol compatibility (feature-gated).
    #[cfg(feature = "transport-grpc-vector-compat")]
    #[must_use]
    pub fn with_vector_compat(mut self) -> Self {
        self.vector_compat = true;
        self
    }
}
