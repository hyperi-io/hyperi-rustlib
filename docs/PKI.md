# hs-rustlib TLS/PKI Integration Guide

## Overview

This document covers TLS configuration for hs-rustlib components. All TLS settings follow the [PKI standards](../ai/standards/common/PKI.md) with profile-based security levels.

## Security Profiles

| Profile | TLS Certs | Use Case |
| ------- | --------- | -------- |
| **Prod** | ECDSA P-384 | Production, customer-facing (corporate default) |
| DevTest | ECDSA P-256 | Dev, staging, internal tools |
| HighSec | P-384 + FIPS | Federal/defence contracts |

**Default profile: `Prod`** - If unsure, use Prod.

---

## Planned: `hs_rustlib::tls` Module

The following module will provide zero-config TLS with profile-based defaults.

### TLS Config Factory

```rust
// Future API - hs_rustlib::tls module
use hs_rustlib::tls::{create_tls_config, TlsProfile};

// Usage
let config = create_tls_config(TlsProfile::Prod)?;      // Corporate default (P-384)
let config = create_tls_config(TlsProfile::DevTest)?;   // Dev/staging (P-256)
let config = create_tls_config(TlsProfile::HighSec)?;   // Federal/CNSA 2.0

// With custom CA
let config = create_tls_config(TlsProfile::Prod)
    .with_ca_cert("/path/to/ca.crt")?;
```

### Implementation Reference

```rust
//! hs_rustlib/src/tls.rs - TLS configuration factory
//!
//! Implementation notes for contributors.

use rustls::{
    ClientConfig, RootCertStore,
    crypto::aws_lc_rs as crypto_provider,
};
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;

/// Security profile for TLS configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsProfile {
    /// Production: P-384, SHA-384, TLS 1.2+
    Prod,
    /// Development/Testing: P-256, SHA-256, TLS 1.2+
    DevTest,
    /// Federal/CNSA 2.0: P-384, SHA-384, TLS 1.3 only, FIPS
    HighSec,
}

impl Default for TlsProfile {
    fn default() -> Self {
        Self::Prod
    }
}

/// TLS configuration builder.
pub struct TlsConfigBuilder {
    profile: TlsProfile,
    ca_certs: Option<String>,
    client_cert: Option<String>,
    client_key: Option<String>,
    verify_hostname: bool,
}

impl TlsConfigBuilder {
    /// Create a new builder with the specified profile.
    pub fn new(profile: TlsProfile) -> Self {
        Self {
            profile,
            ca_certs: None,
            client_cert: None,
            client_key: None,
            verify_hostname: true,
        }
    }

    /// Add custom CA certificate.
    pub fn with_ca_cert(mut self, path: &str) -> Self {
        self.ca_certs = Some(path.to_string());
        self
    }

    /// Add client certificate for mTLS.
    pub fn with_client_cert(mut self, cert: &str, key: &str) -> Self {
        self.client_cert = Some(cert.to_string());
        self.client_key = Some(key.to_string());
        self
    }

    /// Build the rustls ClientConfig.
    pub fn build(self) -> Result<ClientConfig, Box<dyn std::error::Error>> {
        let mut root_store = RootCertStore::empty();

        // Load CA certificates
        if let Some(ca_path) = &self.ca_certs {
            let file = File::open(ca_path)?;
            let mut reader = BufReader::new(file);
            let certs = rustls_pemfile::certs(&mut reader)
                .filter_map(|r| r.ok())
                .collect::<Vec<_>>();

            for cert in certs {
                root_store.add(cert)?;
            }
        } else {
            // Use webpki-roots (Mozilla CA bundle)
            root_store.extend(
                webpki_roots::TLS_SERVER_ROOTS
                    .iter()
                    .cloned()
            );
        }

        // Build config based on profile
        let builder = ClientConfig::builder_with_provider(
            Arc::new(crypto_provider::default_provider())
        );

        let config = match self.profile {
            TlsProfile::HighSec => {
                // TLS 1.3 only for highsec
                builder
                    .with_protocol_versions(&[&rustls::version::TLS13])?
                    .with_root_certificates(root_store)
                    .with_no_client_auth()
            }
            _ => {
                // TLS 1.2+ for prod/devtest
                builder
                    .with_protocol_versions(&[
                        &rustls::version::TLS13,
                        &rustls::version::TLS12,
                    ])?
                    .with_root_certificates(root_store)
                    .with_no_client_auth()
            }
        };

        Ok(config)
    }
}

/// Create TLS configuration with profile-appropriate settings.
///
/// # Example
///
/// ```rust
/// use hs_rustlib::tls::{create_tls_config, TlsProfile};
///
/// let config = create_tls_config(TlsProfile::Prod)?;
/// ```
pub fn create_tls_config(profile: TlsProfile) -> TlsConfigBuilder {
    TlsConfigBuilder::new(profile)
}
```

---

## Kafka TLS Configuration

### Current: `KafkaConfig`

The existing Kafka transport supports SSL via config fields:

```rust
use hs_rustlib::transport::kafka::KafkaConfig;

let config = KafkaConfig {
    brokers: vec!["kafka:9093".into()],
    security_protocol: "SSL".into(),
    ssl_ca_location: Some("/path/to/ca.crt".into()),
    ssl_certificate_location: Some("/path/to/client.crt".into()),
    ssl_key_location: Some("/path/to/client.key".into()),
    ssl_skip_verify: false,
    ..Default::default()
};
```

### Planned Enhancement

Profile-based SSL configuration:

```rust
// Future API
use hs_rustlib::transport::kafka::KafkaConfig;
use hs_rustlib::tls::TlsProfile;

// Auto-loads TLS config from settings
let config = KafkaConfig::from_settings()?;

// Or explicit profile
let config = KafkaConfig::with_profile(TlsProfile::Prod)
    .brokers(&["kafka:9093"])
    .build()?;
```

### Implementation Reference

```rust
// Enhancement to hs_rustlib::transport::kafka::config

use crate::tls::TlsProfile;
use std::collections::HashMap;

impl KafkaConfig {
    /// Get librdkafka SSL configuration for profile.
    pub fn ssl_config_for_profile(profile: TlsProfile) -> HashMap<String, String> {
        let mut config = HashMap::new();

        config.insert("security.protocol".into(), "SSL".into());

        match profile {
            TlsProfile::Prod | TlsProfile::HighSec => {
                // Full hostname verification
                config.insert(
                    "ssl.endpoint.identification.algorithm".into(),
                    "https".into()
                );
            }
            TlsProfile::DevTest => {
                // No hostname verification for dev
                config.insert(
                    "ssl.endpoint.identification.algorithm".into(),
                    "".into()
                );
            }
        }

        config
    }

    /// Create config with TLS profile settings.
    pub fn with_profile(profile: TlsProfile) -> KafkaConfigBuilder {
        KafkaConfigBuilder::new().profile(profile)
    }
}
```

### Manual Kafka mTLS Setup

```rust
use rdkafka::config::ClientConfig;
use rdkafka::producer::FutureProducer;

// Production mTLS configuration
let producer: FutureProducer = ClientConfig::new()
    .set("bootstrap.servers", "kafka:9093")
    .set("security.protocol", "SSL")

    // CA certificate (verify broker)
    .set("ssl.ca.location", "/etc/kafka/ca.crt")

    // Client certificate (mTLS authentication)
    .set("ssl.certificate.location", "/etc/kafka/client.crt")
    .set("ssl.key.location", "/etc/kafka/client.key")

    // Hostname verification (prod/highsec only)
    .set("ssl.endpoint.identification.algorithm", "https")

    .create()
    .expect("Producer creation failed");
```

---

## HTTP Client Configuration

### reqwest with TLS

```rust
use reqwest::Client;
use std::fs;

// Simple: use system CA (works for most cases)
let client = Client::new();

// Custom CA bundle
let ca_cert = fs::read("/path/to/ca.crt")?;
let cert = reqwest::Certificate::from_pem(&ca_cert)?;

let client = Client::builder()
    .add_root_certificate(cert)
    .build()?;

// Client certificate (mTLS)
let client_cert = fs::read("/path/to/client.crt")?;
let client_key = fs::read("/path/to/client.key")?;
let identity = reqwest::Identity::from_pem(
    &[client_cert, client_key].concat()
)?;

let client = Client::builder()
    .add_root_certificate(cert)
    .identity(identity)
    .build()?;
```

### hyper with rustls

```rust
use hyper::Client;
use hyper_rustls::HttpsConnectorBuilder;
use rustls::ClientConfig;
use std::sync::Arc;

// Create rustls config
let tls_config = create_tls_config(TlsProfile::Prod).build()?;

// Create HTTPS connector
let connector = HttpsConnectorBuilder::new()
    .with_tls_config(tls_config)
    .https_only()
    .enable_http1()
    .enable_http2()
    .build();

let client = Client::builder()
    .build::<_, hyper::Body>(connector);
```

---

## Database TLS Configuration

### PostgreSQL with tokio-postgres

```rust
use tokio_postgres::{Config, NoTls};
use rustls::ClientConfig;
use tokio_postgres_rustls::MakeRustlsConnect;

// Create TLS config
let tls_config = create_tls_config(TlsProfile::Prod)
    .with_ca_cert("/path/to/ca.crt")
    .build()?;

let tls = MakeRustlsConnect::new(tls_config);

// Connect with TLS
let (client, connection) = Config::new()
    .host("db.example.com")
    .user("app")
    .password("secret")
    .dbname("production")
    .connect(tls)
    .await?;
```

### SQLx with TLS

```rust
use sqlx::postgres::{PgConnectOptions, PgSslMode};
use sqlx::PgPool;

// Prod profile: verify-full
let options = PgConnectOptions::new()
    .host("db.example.com")
    .username("app")
    .password("secret")
    .database("production")
    .ssl_mode(PgSslMode::VerifyFull)
    .ssl_root_cert("/path/to/ca.crt");

let pool = PgPool::connect_with(options).await?;

// DevTest profile: require (no verification)
let options = PgConnectOptions::new()
    .host("dev-db.local")
    .ssl_mode(PgSslMode::Require);
```

---

## Config Cascade

Settings can be provided via YAML config or environment variables:

### settings.yaml

```yaml
# settings.yaml
tls:
  profile: prod             # prod | devtest | highsec
  ca_bundle: auto           # auto = system/webpki-roots
  verify_hostname: true

kafka:
  security_protocol: SSL
  ssl_ca_location: /etc/kafka/ca.crt
  ssl_certificate_location: /etc/kafka/client.crt
  ssl_key_location: /etc/kafka/client.key
```

### Environment Variables

```bash
# TLS profile
HS_TLS_PROFILE=prod

# Kafka SSL
KAFKA_SSL_CA_LOCATION=/etc/kafka/ca.crt
KAFKA_SSL_CERTIFICATE_LOCATION=/etc/kafka/client.crt
KAFKA_SSL_KEY_LOCATION=/etc/kafka/client.key
```

---

## Certificate Generation

### Generate P-384 Certificate (Prod profile)

```bash
# Private key
openssl ecparam -genkey -name secp384r1 -out server.key

# CSR
openssl req -new -key server.key -out server.csr \
    -subj "/CN=app.example.com/O=HyperSec/C=AU"

# Self-signed (dev only)
openssl req -x509 -new -key server.key -out server.crt -days 365 \
    -subj "/CN=app.example.com/O=HyperSec/C=AU"
```

### Generate P-256 Certificate (DevTest profile)

```bash
# Private key
openssl ecparam -genkey -name prime256v1 -out server.key

# CSR
openssl req -new -key server.key -out server.csr \
    -subj "/CN=dev.local/O=HyperSec/C=AU"

# Self-signed
openssl req -x509 -new -key server.key -out server.crt -days 365 \
    -subj "/CN=dev.local/O=HyperSec/C=AU"
```

---

## Dependencies

Add to `Cargo.toml`:

```toml
[dependencies]
# TLS
rustls = { version = "0.23", features = ["aws-lc-rs"] }
rustls-pemfile = "2"
webpki-roots = "0.26"

# HTTP clients
reqwest = { version = "0.12", features = ["rustls-tls"] }
hyper-rustls = { version = "0.27", features = ["http1", "http2"] }

# Kafka
rdkafka = { version = "0.36", features = ["ssl"] }

# Database
sqlx = { version = "0.8", features = ["postgres", "runtime-tokio", "tls-rustls"] }
tokio-postgres-rustls = "0.12"
```

---

## Troubleshooting

### Certificate Verify Failed

```rust
// Error: certificate verify failed
//
// Causes:
// 1. Missing CA bundle
// 2. Self-signed cert not in trust store
// 3. Certificate expired
// 4. Hostname mismatch

// Debug: inspect certificate
use x509_parser::prelude::*;
use std::fs;

let cert_pem = fs::read("/path/to/cert.crt")?;
let (_, pem) = parse_x509_pem(&cert_pem)?;
let cert = pem.parse_x509()?;

println!("Subject: {:?}", cert.subject());
println!("Issuer: {:?}", cert.issuer());
println!("Valid until: {:?}", cert.validity().not_after);
```

### Kafka SSL Handshake Failed

```rust
// Error: SSL handshake failed
//
// Common causes:
// 1. Wrong protocol (check ssl vs sasl_ssl)
// 2. Missing client cert for mTLS
// 3. CA doesn't trust broker cert

// Debug: enable rdkafka debug logging
use rdkafka::config::ClientConfig;

let producer = ClientConfig::new()
    .set("bootstrap.servers", "kafka:9093")
    .set("security.protocol", "SSL")
    .set("ssl.ca.location", "/path/to/ca.crt")
    .set("debug", "security,broker")  // Enable debug
    .create::<FutureProducer>()
    .expect("Producer creation failed");
```

---

## References

- [PKI Standards](../ai/standards/common/PKI.md) - Full PKI/TLS standards
- [rustls documentation](https://docs.rs/rustls)
- [webpki-roots](https://docs.rs/webpki-roots) - Mozilla CA bundle for Rust
- [rdkafka SSL configuration](https://github.com/fede1024/rust-rdkafka)
- [SQLx TLS](https://docs.rs/sqlx/latest/sqlx/postgres/struct.PgConnectOptions.html)
