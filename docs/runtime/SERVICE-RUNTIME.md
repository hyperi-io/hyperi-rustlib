# Service Runtime

`ServiceRuntime` is the pre-wired infrastructure object that every
DFE service receives from `run_app` before its `run_service` method
is called. It collapses the identical startup boilerplate every
service would otherwise hand-write into a single typed struct.

A service author writes `DfeApp::run_service(config, runtime)` and
uses the runtime's fields directly -- metrics manager, memory guard,
shutdown token, worker pool, batch engine, scaling pressure,
self-regulation governor, K8s context. Nothing to plumb, nothing to
remember to register.

---

## What's in the runtime

| Field | Type | Feature gate | Always present? |
|-------|------|--------------|-----------------|
| `metrics` | `MetricsManager` | always (with `metrics`) | yes |
| `dfe` | `Arc<DfeMetrics>` | always (with `metrics`) | yes |
| `memory_guard` | `Arc<MemoryGuard>` | `memory` | yes |
| `shutdown` | `CancellationToken` | always | yes |
| `context` | `&'static RuntimeContext` | always | yes |
| `worker_pool` | `Option<Arc<AdaptiveWorkerPool>>` | `worker-pool` | optional |
| `batch_engine` | `Option<Arc<BatchEngine>>` | `worker-batch` | optional |
| `scaling` | `Option<Arc<ScalingPressure>>` | `scaling` | optional |
| `governor` | `Option<SelfRegulationGovernor>` | `governor` | optional (default-on) |

The pillars (`metrics`, `dfe`, `shutdown`, `context`) are always
present. The optional fields are `Some(...)` when their feature is
on and configuration succeeds -- `None` if construction fails (logged
as a warning, not fatal).

`governor` is the exception: its feature is on by default. It is
`None` only when opted out via `self_regulation.enabled = false`, in
which case nothing is constructed and the data path is byte-identical
to pre-governor. See [SELF-REGULATION.md](SELF-REGULATION.md).

The whole bundle is enabled via the `cli-service` feature, which
pulls in `metrics + memory + scaling + worker-pool + shutdown +
governor` so that a single feature flag gives a downstream app the
full service-runtime profile.

---

## Lifecycle

```mermaid
flowchart LR
    A[main] --> B["run_app::&lt;A: DfeApp&gt;"]
    B --> C[parse CommonArgs]
    C --> D[init logger]
    D --> E[app.load_config]
    E --> F[ServiceRuntime::build]
    F --> G["app.run_service(config, runtime)"]
    G --> H[wait on shutdown token]
```

Step by step inside `run_app` for the default `run` subcommand:

1. Resolve the subcommand from `app.command()` -- defaults to `Run`.
2. Install the logger with the service name and version injected for
   JSON output.
3. Call `app.load_config(args.config.as_deref())` -- apps own this
   step so they can deserialise into their own typed config.
4. Build `ServiceRuntime`:
   - Construct `MetricsManager`, register `DfeMetrics`.
   - Construct `MemoryGuard` from env prefix (cgroup auto-detect).
   - Construct the self-regulation governor from the same guard if
     `governor` is on and not opted out. Built before the worker pool,
     batch engine, and transports so its pressure and byte budget can
     thread into all of them.
   - Construct `ScalingPressure` from cascade if `scaling` is on,
     wire it into the metrics manager.
   - Construct `AdaptiveWorkerPool` from cascade if `worker-pool`
     is on, register its metrics, hand it the memory guard and
     scaling pressure.
   - Construct `BatchEngine` if `worker-batch` is on, auto-wire it
     to the metrics manager and memory guard, wire the governor's
     byte budget into its governed run path.
   - Install signal handler -- returns `CancellationToken`.
   - Start the worker pool scaling loop.
   - Start the metrics server on `args.metrics_addr`.
   - Fire-and-forget version check if `version-check` is on.
5. Call `app.run_service(config, runtime)`.

The service author's code starts at step 5 -- everything before that
is the framework.

---

## DfeApp trait

```rust
pub trait DfeApp: Sized {
    type Config: DeserializeOwned + Debug + Send + Sync;

    fn name(&self) -> &str;
    fn env_prefix(&self) -> &str;
    fn version_info(&self) -> VersionInfo;
    fn common_args(&self) -> &CommonArgs;
    fn load_config(&self, path: Option<&str>) -> Result<Self::Config, CliError>;
    fn run_service(
        &self,
        config: Self::Config,
        runtime: ServiceRuntime,
    ) -> impl Future<Output = Result<(), CliError>> + Send;

    // Optional -- defaults provided.
    fn command(&self) -> Option<&StandardCommand> { None }
    fn scaling_components(&self, _: &Self::Config) -> Vec<ScalingComponent> { vec![] }  // cfg: scaling
    fn register_metrics(&self, _: &MetricsManager) {}                                   // cfg: metrics | otel-metrics
    fn deployment_contract(&self) -> Option<DeploymentContract> { None }                // cfg: deployment
}
```

| Method | Required? | Purpose |
|--------|-----------|---------|
| `name` | yes | Service name -- drives metric namespace, log tags |
| `env_prefix` | yes | Prefix for env-var config overrides (`DFE_LOADER_*`) |
| `version_info` | yes | Version + commit + build timestamp |
| `common_args` | yes | Returns the embedded `CommonArgs` clap struct |
| `load_config` | yes | App-specific cascade load (typically `config::setup` + `unmarshal`) |
| `run_service` | yes | The actual service loop -- gets a fully wired runtime |
| `command` | no | Override to expose app-specific subcommands |
| `scaling_components` | no (cfg `scaling`) | Register app-specific KEDA signals (lag, queue depth) |
| `register_metrics` | no (cfg `metrics`) | Register app metrics for `metrics-manifest` / `generate-artefacts` |
| `deployment_contract` | no (cfg `deployment`) | Build the contract for `generate-artefacts` |

Apps that don't override the last four get sensible no-op defaults.
The last three only exist when their feature is compiled in.

---

## Standard subcommands

`run_app` dispatches on `StandardCommand` before reaching the
service loop. Every service gets the same six subcommands without
writing any extra code:

| Subcommand | Behaviour |
|------------|-----------|
| `run` | Default -- full lifecycle, ends in `run_service` |
| `version` | Print `version_info()` and exit |
| `config-check` | Load logger + config, print summary, exit non-zero on failure |
| `metrics-manifest` | Build a `MetricsManager`, call `register_metrics`, print manifest JSON, exit |
| `generate-artefacts --output-dir <dir>` | Emit `metrics-manifest.json`, `deployment-contract.json`, `container-manifest.json`, `Dockerfile.runtime`, `argocd-application.yaml` |
| `top` | Live metrics TUI (when `top` feature is on) |

`config-check` exists so CI can validate config without booting the
service. `metrics-manifest` and `generate-artefacts` exist so CI can
generate deployment artefacts deterministically -- same input, same
output, no timestamps.

---

## Readiness check

The runtime starts the HTTP server with a default readiness check
that returns true once startup completes. Each app overrides this
once it knows what "ready" means for its domain:

```rust
async fn run_service(&self, config: Self::Config, mut runtime: ServiceRuntime)
    -> Result<(), CliError>
{
    // ... wire pipeline ...
    let p = Arc::clone(&pipeline);
    runtime.set_readiness_check(move || p.is_consuming());
    // ... run loop ...
}
```

`set_readiness_check` takes a `Fn() -> bool + Send + Sync + 'static`
closure that the metrics server's `/readyz` handler calls on every
probe.

---

## What stays app-specific

The runtime deliberately stops short of full automation. These
remain in app code because they're genuinely domain-specific:

- Readiness criteria -- each service has its own "I can serve traffic" definition.
- Config hot-reload -- optional, and the reload semantics differ per app.
- Pipeline construction -- the whole point of the service.
- DLQ wiring -- varies by transport backend and policy.
- App-specific metric groups (`ConsumerMetrics`, `BufferMetrics`).

---

## API surface

| Item | Purpose |
|------|---------|
| `DfeApp` trait | Service contract -- implement to get the standard lifecycle |
| `run_app::<A>(app)` | Drives the lifecycle; matches subcommand, builds runtime, calls `run_service` |
| `ServiceRuntime` | Pre-wired infrastructure bundle -- built by `run_app`, passed to `run_service` |
| `ServiceRuntime::set_readiness_check(fn)` | Install the app's readiness criterion |
| `ServiceRuntime::batch_engine()` | Borrow the batch engine if `worker-batch` is on |
| `ServiceRuntime::governed_receiver(key)` | Build a receive transport with the governor's inbound brake wired in (cfg `governor` + `transport`); falls back to a plain receiver when the governor is off |
| `StandardCommand` | Subcommand enum -- apps embed via `#[command(flatten)]` |
| `CommonArgs` | Standard CLI flags (`--config`, `--log-level`, `--metrics-addr`, ...) |
| `VersionInfo` | Service version + commit + build timestamp |
| `CliError` | Lifecycle error type -- service errors wrap into `Service(String)` |

---

## Testing

`ServiceRuntime::build` is `pub(crate)` -- tests don't construct it
directly. For unit tests of `run_service`, build only the bits you
need (`MetricsManager::new_for_test`, `CancellationToken::new`,
explicit `MemoryGuard`) and skip the framework. Integration tests
that boot the full runtime go through `run_app` with a fake config
fixture.

---

## Related

- [../AUTO-WIRING.md](../AUTO-WIRING.md) -- singleton pattern across pillars
- [../INTEGRATION.md](../INTEGRATION.md) -- service skeleton recipe
- [RUNTIME-CONTEXT.md](RUNTIME-CONTEXT.md) -- `RuntimeContext` detection
- [MEMORY.md](MEMORY.md) -- `MemoryGuard`
- [SELF-REGULATION.md](SELF-REGULATION.md) -- governor, inbound brake, AIMD budget
- [../core-pillars/SHUTDOWN.md](../core-pillars/SHUTDOWN.md) -- signal handler, K8s pre-stop
- [../core-pillars/CONFIG.md](../core-pillars/CONFIG.md) -- cascade
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) -- `cli`, `cli-service`, `governor`
- Source: [../../src/cli/runtime.rs](../../src/cli/runtime.rs),
  [../../src/cli/app.rs](../../src/cli/app.rs),
  [../../src/cli/commands.rs](../../src/cli/commands.rs)
