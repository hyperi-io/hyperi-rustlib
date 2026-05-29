# Integration

This walks through wiring `hyperi-rustlib` into a new DFE service from
empty `Cargo.toml` to running binary. The goal is a self-contained recipe;
the deep-dive details for each line live in the subsystem docs.

If you already know what a `DfeApp` is and just want the trait signature,
jump to [cli/app.rs](../src/cli/app.rs).

---

## 1. Pick your feature set

Two questions:

- **Is this a service** (long-running, pipelines data) or a **tool**
  (one-shot CLI)?
- **What transports** does it touch — Kafka, gRPC, file, none?

For a typical DFE service that talks to Kafka, ingests gRPC, scales under
KEDA, and ships container artefacts, the `Cargo.toml` reads:

```toml
[dependencies.hyperi-rustlib]
version = "2"
features = [
    "cli-service",          # DfeApp trait, run_app, ServiceRuntime
    "config-reload",        # Hot-reload of the config cascade
    "logger",               # Always
    "metrics-dfe",          # DFE-specific metric groups
    "transport-kafka",      # Kafka producer + consumer
    "transport-grpc",       # gRPC server + client
    "tiered-sink",          # Resilient delivery with disk spillover
    "dlq-kafka",            # DLQ to a Kafka topic
    "spool",                # Disk-backed FIFO
    "memory",               # Cgroup-aware OOM prevention
    "scaling",              # KEDA pressure signal
    "worker-batch",         # BatchEngine (SIMD parse + parallel transform)
    "http-server",          # /healthz, /readyz, /metrics, /config
    "deployment",           # DeploymentContract + generators
    "version-check",        # Startup probe to version API
    "expression",           # CEL for transport filters
]
```

`cli-service` is doing most of the work — see
[FEATURE-FLAGS.md](FEATURE-FLAGS.md#cli-service) for what it pulls in
transitively.

A tooling-style CLI that doesn't need a metrics server or worker pool
takes `cli` instead, no `-service`:

```toml
features = ["cli", "config", "logger"]
```

For light apps that just need config + logs, the defaults (`config`,
`logger`) are enough — list nothing else.

---

## 2. Define your config

The cascade does the heavy lifting; you write a plain `serde` struct.

```rust
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct LoaderConfig {
    pub kafka: KafkaConfig,
    pub clickhouse: ClickHouseConfig,
    pub batch_size: usize,
}

#[derive(Debug, Deserialize)]
pub struct KafkaConfig {
    pub brokers: Vec<String>,
    pub topic: String,
    #[serde(default)]
    pub username: Option<hyperi_rustlib::SensitiveString>,
    #[serde(default)]
    pub password: Option<hyperi_rustlib::SensitiveString>,
}
```

Use `SensitiveString` for anything that should never appear in logs or in
the `/config` admin endpoint. See
[core-pillars/CONFIG.md](core-pillars/CONFIG.md#sensitive-fields).

YAML config files (`defaults.yaml`, `settings.yaml`,
`settings.{env}.yaml`) populate these structs through the cascade. ENV
vars override (`DFE_LOADER__KAFKA__BROKERS=...`, double underscore for
nesting).

---

## 3. Implement `DfeApp`

```rust
use hyperi_rustlib::cli::{
    CliError, CommonArgs, DfeApp, ScalingComponent, ServiceRuntime,
    StandardCommand, VersionInfo, run_app,
};
use hyperi_rustlib::metrics::MetricsManager;
use hyperi_rustlib::deployment::DeploymentContract;

#[derive(clap::Parser)]
pub struct LoaderCli {
    #[command(flatten)]
    common: CommonArgs,
    #[command(subcommand)]
    command: Option<StandardCommand>,
}

impl DfeApp for LoaderCli {
    type Config = LoaderConfig;

    fn name(&self) -> &str { "dfe-loader" }
    fn env_prefix(&self) -> &str { "DFE_LOADER" }
    fn version_info(&self) -> VersionInfo {
        VersionInfo::new("dfe-loader", env!("CARGO_PKG_VERSION"))
    }
    fn common_args(&self) -> &CommonArgs { &self.common }
    fn command(&self) -> Option<&StandardCommand> { self.command.as_ref() }

    fn load_config(&self, path: Option<&str>) -> Result<LoaderConfig, CliError> {
        // The cascade does the work; you just call it.
        hyperi_rustlib::config::load_typed(path, self.env_prefix())
            .map_err(CliError::from)
    }

    async fn run_service(
        &self,
        config: LoaderConfig,
        runtime: ServiceRuntime,
    ) -> Result<(), CliError> {
        // runtime gives you metrics, memory guard, scaling pressure,
        // worker pool, shutdown token, runtime context — already wired.
        let shutdown = runtime.shutdown_token();
        let workers = runtime.worker_pool().expect("cli-service wires this");

        // Build your pipeline using the runtime's primitives.
        let pipeline = LoaderPipeline::new(config, workers);
        pipeline.run_until(shutdown).await.map_err(CliError::from)
    }

    fn scaling_components(&self, config: &LoaderConfig) -> Vec<ScalingComponent> {
        // KEDA reads ScalingPressure; you register the signals.
        vec![
            ScalingComponent::new("buffer_depth", 0.40),
            ScalingComponent::new("consumer_lag", 0.40),
            ScalingComponent::new("error_rate", 0.20),
        ]
    }

    fn register_metrics(&self, mgr: &MetricsManager) {
        DfeMetrics::register(mgr);
        // ... app-specific metric registrations
    }

    fn deployment_contract(&self) -> Option<DeploymentContract> {
        Some(crate::deployment::contract())
    }
}
```

Notes on the trait surface:

- `load_config` is yours to define — typically one call into
  `config::load_typed` or `Config::from_cascade::<T>`.
- `run_service` is where the actual work lives. Everything else is
  scaffolding.
- `scaling_components`, `register_metrics`, `deployment_contract` all
  have default no-op impls. Override only what you need.
- The `ServiceRuntime` parameter is the gift: see
  [runtime/SERVICE-RUNTIME.md](runtime/SERVICE-RUNTIME.md) for what's
  inside it.

---

## 4. `main.rs`

```rust
use clap::Parser;
use hyperi_rustlib::cli::{CliError, run_app};

#[tokio::main]
async fn main() -> Result<(), CliError> {
    let app = LoaderCli::parse();
    run_app(app).await
}
```

That's the whole `main.rs`. `run_app` handles the subcommand dispatch
(`run`, `version`, `config-check`, `metrics-manifest`,
`generate-artefacts`), initialises the logger, loads config, builds the
runtime, and calls your `run_service`.

---

## 5. Wire the YAML config files

In your app repo:

```
config/
├── defaults.yaml          # Safe fallback baseline (always loaded last)
├── settings.yaml          # Team defaults (committed)
└── settings.production.yaml   # Per-env overrides (committed)
.env                       # Local-dev secrets (gitignored)
```

The cascade searches `./`, `./config/`, `/config/`,
`~/.config/dfe-loader/` for each file in turn. See
[core-pillars/CONFIG.md](core-pillars/CONFIG.md#cascade) for the full
priority order.

---

## 6. Verify

```bash
# Validate config without running
dfe-loader config-check --config config/settings.yaml

# Print the metric catalogue
dfe-loader metrics-manifest > metrics-manifest.json

# Generate deployment artefacts (Dockerfile, chart/, argocd-application.yaml)
dfe-loader generate-artefacts --output-dir ci/

# Run it
dfe-loader run --config config/settings.yaml
```

`config-check` walks the cascade and prints what got loaded. Use it in
CI to catch config-file mistakes before container build.

`generate-artefacts` writes the full `ci/` directory deployment side
needs. See [deployment/ARTEFACTS.md](deployment/ARTEFACTS.md) for what
ends up there.

---

## 7. What you didn't have to write

For the dfe-loader-shaped app above, the code you actually write is:

- a `Cargo.toml` dependency block
- your config struct definitions
- the `DfeApp` impl
- a near-trivial `main.rs`
- the `deployment_contract()` builder
- your actual pipeline business logic in `run_service` — the bulk of it

What you skipped:

| Skipped | Source |
|---------|--------|
| `tracing-subscriber` setup, JSON/text autodetect | `logger::setup_default` |
| `figment` cascade, env-var nesting, `.env` loading, sensitive masking | `config::load_typed` |
| Prometheus exporter, `/metrics` endpoint, process metrics | `MetricsManager::new` |
| axum HTTP server, probe routes, `/metrics`, `/config` | `http_server` (wired by `ServiceRuntime`) |
| OTel SDK setup, OTLP exporter, traceparent propagation | `otel-tracing` + `transport-trace` |
| `HealthRegistry`, `/healthz` / `/readyz` / `/startupz` | `health` + `http_server` |
| SIGTERM/SIGINT, K8s pre-stop delay, cancellation propagation | `shutdown` (wired by `ServiceRuntime`) |
| `MemoryGuard`, cgroup-aware OOM prevention | `memory` (wired by `ServiceRuntime`) |
| Rayon pool sizing, pressure-based scaling | `worker_pool` (wired by `ServiceRuntime`) |
| Disk-spillover spool, circuit breaker, retry, DLQ fallback | `tiered_sink` (composed in `run_service`) |
| Dockerfile, Helm chart, ArgoCD Application, container manifest | `deployment_contract()` + `generate_*` |
| KEDA ScaledObject + TriggerAuthentication | `KedaContract` (auto-included in Helm chart) |

See [AUTO-WIRING.md](AUTO-WIRING.md) for the full
"if-you-call-X-you-get-Y" model.

---

## Reference apps

The six core DFE apps are the canonical examples. Read whichever is
closest in shape to what you're building:

| App | Best for |
|-----|----------|
| [dfe-loader](https://github.com/hyperi-io/dfe-loader) | Kafka in, ClickHouse out — the most complete `cli-service` integration |
| [dfe-receiver](https://github.com/hyperi-io/dfe-receiver) | gRPC ingress + Kafka publish — push-mode entry |
| [dfe-fetcher](https://github.com/hyperi-io/dfe-fetcher) | Pull-mode (AWS/Azure/M365/GCP) ingress |
| [dfe-archiver](https://github.com/hyperi-io/dfe-archiver) | Long-term storage sink |
| [dfe-transform-vrl](https://github.com/hyperi-io/dfe-transform-vrl) | Embedded VRL transform engine |
| [dfe-transform-vector](https://github.com/hyperi-io/dfe-transform-vector) | Thin wrapper around Vector.dev |
