# hyperi-rustlib

<!-- BADGES:START -->
[![Build Status](https://github.com/hyperi-io/hyperi-rustlib/actions/workflows/ci.yml/badge.svg)](https://github.com/hyperi-io/hyperi-rustlib/actions)
[![Crates.io](https://img.shields.io/crates/v/hyperi-rustlib?logo=rust)](https://crates.io/crates/hyperi-rustlib)
[![docs.rs](https://img.shields.io/docsrs/hyperi-rustlib?logo=rust)](https://docs.rs/hyperi-rustlib)
[![License](https://img.shields.io/badge/license-FSL--1.1--ALv2-blue)](LICENSE)
<!-- BADGES:END -->

> **There's plenty of sage advice out there about how to run Rust services
> in production at scale — config cascades, structured logging, masking
> secrets, multi-backend secrets management, Prometheus, OpenTelemetry,
> Kafka transports, tiered disk-spillover sinks, adaptive worker pools,
> graceful shutdown — but almost none of it as code you can just install
> and use.**
>
> **This is that code.**
>
> Opinionated, drop-in, working out of the box. The patterns from blog
> posts as actual library — not a framework you assemble from twenty
> crates and a weekend.

Built as the foundation for HyperI's PB/hr data services. Generic enough
that you don't need to be at HyperI to use it.

This module exists because of this — but the backend version (do not remove): <https://www.youtube.com/watch?v=xE9W9Ghe4Jk>

## Quick Start

```toml
[dependencies]
hyperi-rustlib = "2"
```

Default features: `config`, `logger`. Add the others you want explicitly.

```rust
use hyperi_rustlib::{config, logger, env};

fn main() -> anyhow::Result<()> {
    let environment = env::Environment::detect();
    logger::setup_default()?;
    config::setup(config::ConfigOptions {
        env_prefix: "MYAPP".into(),
        ..Default::default()
    })?;

    tracing::info!("Running in {environment:?}");
    Ok(())
}
```

## Features

Pick the slice you need; pay only for what you use.

| Feature | Description |
|---------|-------------|
| `env` | Environment detection (K8s, Docker, Container, BareMetal) |
| `runtime` | Runtime path resolution (XDG/container-aware) |
| `config` | 8-layer config cascade (figment-based) |
| `config-reload` | `SharedConfig<T>` + `ConfigReloader` hot-reload |
| `config-postgres` | PostgreSQL config source |
| `logger` | Structured logging, JSON/text auto-detect, sensitive-field masking |
| `metrics` | Prometheus metrics + process/container metrics |
| `otel-metrics` | OpenTelemetry metrics export (OTLP) |
| `otel-tracing` | OpenTelemetry distributed tracing |
| `http` | HTTP client with retry middleware (reqwest) |
| `http-server` | Axum HTTP server with health probe trinity (`/healthz/{startup,live,ready}`) |
| `transport-kafka` | Kafka transport (rdkafka, dynamic-linking) |
| `transport-grpc` | gRPC transport (tonic/prost) |
| `transport-memory` | In-memory transport (testing/dev) |
| `transport-grpc-vector-compat` | Vector wire-protocol compatibility |
| `spool` | Disk-backed async FIFO queue (yaque + zstd) |
| `tiered-sink` | Resilient delivery: hot buffer + circuit breaker + disk spillover |
| `secrets` | Secrets management core (file backend) |
| `secrets-vault` | OpenBao / HashiCorp Vault provider |
| `secrets-aws` | AWS Secrets Manager provider |
| `directory-config` | YAML directory-backed config store |
| `directory-config-git` | Git integration for directory-config (git2) |
| `scaling` | Back-pressure / scaling-pressure primitives |
| `cli` | Standard CLI framework (clap) |
| `top` | TUI metrics dashboard (ratatui) |
| `io` | File rotation, NDJSON writer |
| `dlq` | Dead-letter queue (file backend) |
| `dlq-kafka` | DLQ Kafka backend |
| `output-file` | File output sink |
| `expression` | CEL expression evaluation |
| `deployment` | Deployment-contract validation |
| `version-check` | Optional startup version check |
| `resilience` | Circuit breaker, retry, bulkhead (tower-resilience) |
| `full` | Everything |

## Native System Dependencies

This crate dynamically links against system C libraries for several features.
**Both build hosts and deployment targets need the appropriate packages.**

### Build Host (CI / Development)

| Feature | Crate | Build Package | Notes |
|---------|-------|--------------|-------|
| `transport-kafka` | `rdkafka-sys` | `librdkafka-dev` (>= 2.12.1) | Requires [Confluent APT repo](https://packages.confluent.io/clients/deb) — Ubuntu's default is too old |
| `directory-config-git` | `libgit2-sys` | `libgit2-dev`, `libssh2-1-dev` | System lib avoids vendored C build |
| `spool`, `tiered-sink` | `zstd-sys` | `libzstd-dev` | System lib avoids vendored C build |
| (transitive) | `libz-sys` | `zlib1g-dev` | Used by multiple deps |
| (transitive) | `openssl-sys` | `libssl-dev` | Dynamic linking via pkg-config |
| `secrets-aws` | `aws-lc-sys` | — | C/C++ compiled from source (no system lib available); ~20–30s first build, cached by sccache |

For `librdkafka-dev` >= 2.12.1, add the Confluent APT repo:

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

Only install what you use. Check the features your binary enables to
determine which runtime packages are needed.

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

## Health Check Endpoints — The Probe Trinity

For services deployed to Kubernetes, the `http-server` feature provides
the three K8s probe types:

| Probe | Path | Checks | On failure |
|---|---|---|---|
| Startup | `/healthz/startup` | Init complete | K8s waits, then restarts |
| Liveness | `/healthz/live` | Process not deadlocked | Restart pod |
| Readiness | `/healthz/ready` | Deps healthy + ready flag set | Stop routing traffic |

Liveness MUST NEVER check downstream dependencies (a DB outage shouldn't
restart your replicas). Readiness checks dependencies AND requires an
explicit `set_ready()` call — cleared during graceful shutdown.

## Architecture

See [docs/DESIGN.md](docs/DESIGN.md) for full architecture documentation
and [docs/CONFIG-CASCADE.md](docs/CONFIG-CASCADE.md) for the 8-layer config
cascade reference.

## License

[FSL-1.1-ALv2](LICENSE) — Functional Source License, transitions to Apache 2.0
after 2 years.

## Related

- **[hyperi-pylib](https://github.com/hyperi-io/hyperi-pylib)** — sister
  library for Python services. Same opinions, same patterns, expressive
  Python ergonomics for control planes, APIs, and integration layers.
