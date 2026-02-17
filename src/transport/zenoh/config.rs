// Project:   hyperi-rustlib
// File:      src/transport/zenoh/config.rs
// Purpose:   Zenoh transport configuration
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

use serde::{Deserialize, Serialize};

/// Zenoh operation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZenohMode {
    /// Peer mode - direct mesh networking without routers.
    /// Best for small deployments (<10 nodes) or dev/test.
    #[default]
    Peer,
    /// Client mode - connects to Zenoh routers.
    /// Best for larger deployments with router infrastructure.
    Client,
    /// Router mode - this instance acts as a router.
    Router,
}

impl std::fmt::Display for ZenohMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Peer => write!(f, "peer"),
            Self::Client => write!(f, "client"),
            Self::Router => write!(f, "router"),
        }
    }
}

/// Zenoh reliability mode for subscribers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZenohReliability {
    /// Best-effort delivery (faster, may lose messages).
    BestEffort,
    /// Reliable delivery with retransmission (slower, no loss in-flight).
    #[default]
    Reliable,
}

/// Zenoh congestion control for publishers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZenohCongestionControl {
    /// Block if receiver can't keep up.
    #[default]
    Block,
    /// Drop messages if receiver can't keep up.
    Drop,
}

/// Zenoh transport configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZenohConfig {
    /// Operation mode (peer, client, router).
    #[serde(default)]
    pub mode: ZenohMode,

    /// Endpoints to connect to (e.g., "tcp/localhost:7447").
    #[serde(default)]
    pub connect: Vec<String>,

    /// Endpoints to listen on (e.g., "tcp/0.0.0.0:7447").
    #[serde(default)]
    pub listen: Vec<String>,

    /// Key expressions to subscribe to (e.g., "events/**").
    #[serde(default)]
    pub subscribe: Vec<String>,

    /// Enable shared memory for same-node communication.
    #[serde(default = "default_shm_enabled")]
    pub shm_enabled: bool,

    /// Shared memory buffer size in bytes.
    #[serde(default = "default_shm_size")]
    pub shm_size: usize,

    /// Subscriber reliability mode.
    #[serde(default)]
    pub reliability: ZenohReliability,

    /// Publisher congestion control.
    #[serde(default)]
    pub congestion_control: ZenohCongestionControl,

    /// Receive buffer size (number of messages).
    #[serde(default = "default_recv_buffer")]
    pub recv_buffer_size: usize,

    /// Receive timeout in milliseconds (0 = no wait).
    #[serde(default)]
    pub recv_timeout_ms: u64,
}

fn default_shm_enabled() -> bool {
    true
}

fn default_shm_size() -> usize {
    64 * 1024 * 1024 // 64 MB
}

fn default_recv_buffer() -> usize {
    1000
}

impl Default for ZenohConfig {
    fn default() -> Self {
        Self {
            mode: ZenohMode::Peer,
            connect: Vec::new(),
            listen: Vec::new(),
            subscribe: Vec::new(),
            shm_enabled: default_shm_enabled(),
            shm_size: default_shm_size(),
            reliability: ZenohReliability::Reliable,
            congestion_control: ZenohCongestionControl::Block,
            recv_buffer_size: default_recv_buffer(),
            recv_timeout_ms: 0,
        }
    }
}

impl ZenohConfig {
    /// Create a minimal peer config for dev/test.
    #[must_use]
    pub fn peer(subscribe: Vec<String>) -> Self {
        Self {
            mode: ZenohMode::Peer,
            subscribe,
            ..Default::default()
        }
    }

    /// Create a client config connecting to routers.
    #[must_use]
    pub fn client(connect: Vec<String>, subscribe: Vec<String>) -> Self {
        Self {
            mode: ZenohMode::Client,
            connect,
            subscribe,
            ..Default::default()
        }
    }

    /// Create a router config.
    #[must_use]
    pub fn router(listen: Vec<String>, connect: Vec<String>) -> Self {
        Self {
            mode: ZenohMode::Router,
            listen,
            connect,
            ..Default::default()
        }
    }

    /// Convert to JSON5 configuration string for Zenoh.
    #[must_use]
    pub fn to_json5(&self) -> String {
        use std::fmt::Write;
        let mut config = String::from("{\n");

        // Mode
        let _ = writeln!(config, "  mode: \"{}\",", self.mode);

        // Connect endpoints
        if !self.connect.is_empty() {
            config.push_str("  connect: { endpoints: [");
            for (i, ep) in self.connect.iter().enumerate() {
                if i > 0 {
                    config.push_str(", ");
                }
                let _ = write!(config, "\"{ep}\"");
            }
            config.push_str("] },\n");
        }

        // Listen endpoints
        if !self.listen.is_empty() {
            config.push_str("  listen: { endpoints: [");
            for (i, ep) in self.listen.iter().enumerate() {
                if i > 0 {
                    config.push_str(", ");
                }
                let _ = write!(config, "\"{ep}\"");
            }
            config.push_str("] },\n");
        }

        // Shared memory
        if self.shm_enabled {
            config.push_str("  transport: { shared_memory: { enabled: true } },\n");
        }

        config.push('}');
        config
    }
}
