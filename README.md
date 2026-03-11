# hyperi-rustlib

Shared utility library for HyperI Rust applications. Provides configuration
management, structured logging, Prometheus metrics, environment detection,
transport abstractions, and more.

Rust equivalent of `hyperi-pylib` (Python) and `hyperi-golib` (Go).

## Quick Start

```toml
[dependencies]
hyperi-rustlib = { version = "1.14", registry = "hyperi" }
```

Default features: `config`, `logger`, `metrics`, `env`, `runtime`.

```rust
use hyperi_rustlib::{config, logger, env};

fn main() -> anyhow::Result<()> {
    let env = env::Environment::detect();
    logger::setup_default()?;
    config::setup(config::ConfigOptions {
        env_prefix: "MYAPP".into(),
        ..Default::default()
    })?;

    tracing::info!("Running in {env:?}");
    Ok(())
}
```

## Native System Dependencies

This crate dynamically links against system C libraries for several features.
**Both CI build hosts and deployment targets need the appropriate packages.**

### Build Host (CI / Development)

Install `-dev` packages for compilation. `hyperi-ci` handles this automatically
when `.hyperi-ci.yaml` is present — it detects which `-sys` crates are in
`Cargo.lock` and installs the matching `-dev` packages.

| Feature | Crate | Build Package | Notes |
|---------|-------|--------------|-------|
| `transport-kafka` | `rdkafka-sys` | `librdkafka-dev` (>= 2.12.1) | Requires [Confluent APT repo](https://packages.confluent.io/clients/deb) — Ubuntu's default is too old |
| `directory-config-git` | `libgit2-sys` | `libgit2-dev`, `libssh2-1-dev` | System lib avoids vendored C build |
| `spool`, `tiered-sink` | `zstd-sys` | `libzstd-dev` | System lib avoids vendored C build |
| (transitive) | `libz-sys` | `zlib1g-dev` | Used by multiple deps |
| (transitive) | `openssl-sys` | `libssl-dev` | Dynamic linking via pkg-config |
| `secrets-aws` | `aws-lc-sys` | — | C/C++ compiled from source (no system lib available); ~20-30s first build, cached by sccache |

All packages except `librdkafka-dev` are available from Ubuntu's default
repositories. For `librdkafka-dev` >= 2.12.1, add the Confluent APT repo:

```bash
curl -fsSL https://packages.confluent.io/clients/deb/archive.key \
  | sudo gpg --dearmor -o /usr/share/keyrings/confluent-clients.gpg
echo "deb [signed-by=/usr/share/keyrings/confluent-clients.gpg] \
  https://packages.confluent.io/clients/deb noble main" \
  | sudo tee /etc/apt/sources.list.d/confluent-clients.list
sudo apt-get update
sudo apt-get install -y librdkafka-dev libssl-dev libsasl2-dev pkg-config
```

### Deployment Host (Runtime)

The compiled binary links against `.so` files at runtime. Install the
**runtime** packages (not `-dev`) on deployment hosts or in Docker images.

| Feature | Runtime Package | Shared Object |
|---------|----------------|---------------|
| `transport-kafka` | `librdkafka1` (from Confluent repo) | `librdkafka.so.1` |
| `directory-config-git` | `libgit2-1.7` (or matching version) | `libgit2.so` |
| `spool`, `tiered-sink` | `libzstd1` | `libzstd.so.1` |
| (transitive) | `zlib1g` | `libz.so.1` |
| (transitive) | `libssl3` | `libssl.so.3` |

**Only install what you use.** Downstream binaries (dfe-loader, dfe-receiver,
etc.) only link against libraries for the features they enable. Check each
project's `Cargo.toml` features to determine which runtime packages are needed.

### Docker Example

```dockerfile
# Build stage
FROM rust:1.94 AS builder
RUN apt-get update && apt-get install -y \
    pkg-config libssl-dev librdkafka-dev libgit2-dev libzstd-dev
COPY . .
RUN cargo build --release

# Runtime stage
FROM ubuntu:24.04
RUN apt-get update && apt-get install -y --no-install-recommends \
    librdkafka1 libssl3 libgit2-1.7 libzstd1 ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/myapp /usr/local/bin/
```

For `librdkafka1`, add the Confluent APT repo to both build and runtime stages.

## Features

| Feature | Description |
|---------|-------------|
| `env` | Environment detection (K8s, Docker, Container, BareMetal) |
| `runtime` | Runtime path resolution (XDG/container-aware) |
| `config` | 8-layer config cascade (figment) |
| `config-reload` | `SharedConfig<T>` + `ConfigReloader` hot-reload |
| `config-postgres` | PostgreSQL config source |
| `logger` | Structured logging with JSON/text + sensitive field masking |
| `metrics` | Prometheus metrics + process/container metrics |
| `otel-metrics` | OpenTelemetry metrics export (OTLP) |
| `otel-tracing` | OpenTelemetry distributed tracing |
| `http` | HTTP client with retry middleware (reqwest) |
| `http-server` | Axum HTTP server with health endpoints |
| `transport-kafka` | Kafka transport (rdkafka, dynamic-linking) |
| `transport-grpc` | gRPC transport (tonic/prost) |
| `transport-memory` | In-memory transport (testing/dev) |
| `transport-grpc-vector-compat` | Vector wire-protocol compatibility |
| `spool` | Disk-backed async FIFO queue (yaque + zstd) |
| `tiered-sink` | Resilient delivery with circuit breaker + disk spillover |
| `secrets` | Secrets management core |
| `secrets-vault` | OpenBao/Vault provider |
| `secrets-aws` | AWS Secrets Manager provider |
| `directory-config` | YAML directory-backed config store |
| `directory-config-git` | Git integration for directory-config (git2) |
| `scaling` | Back-pressure / scaling pressure primitives |
| `cli` | Standard CLI framework (clap) |
| `top` | TUI metrics dashboard (ratatui) |
| `io` | File rotation, NDJSON writer |
| `dlq` | Dead-letter queue (file backend) |
| `dlq-kafka` | DLQ Kafka backend |
| `output-file` | File output sink |
| `expression` | CEL expression evaluation |
| `deployment` | Deployment contract validation |
| `version-check` | Startup version check |
| `resilience` | Circuit breaker, retry, bulkhead (tower-resilience) |
| `full` | All features |

## Architecture

See [docs/DESIGN.md](docs/DESIGN.md) for full architecture documentation.

## Configuration Cascade

See [docs/CONFIG-CASCADE.md](docs/CONFIG-CASCADE.md) for the 8-layer
configuration cascade reference.

## Registry

Published to `hyperi` registry (JFrog Artifactory at `hypersec.jfrog.io`).

```toml
# .cargo/config.toml
[registries.hyperi]
index = "sparse+https://hypersec.jfrog.io/artifactory/git/hyperi-cargo-virtual.git"
```

## Licence

FSL-1.1-ALv2 — Copyright (c) 2026 HYPERI PTY LIMITED
