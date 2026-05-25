# Runtime Context

`RuntimeContext` is the per-process metadata bundle detected once at
startup, cached in a `OnceLock`, and read by every module that needs
to know about its deployment environment. Pod name, namespace, node
name, cgroup memory limit, CPU quota — all of it sits behind one
`runtime_context()` call.

The point is to stop modules each rolling their own env-var probes
and cgroup file reads. Detection happens once. Everything else is a
`&'static` borrow.

---

## What's detected

```rust
pub struct RuntimeContext {
    pub environment: Environment,         // K8s / Docker / Container / BareMetal
    pub pod_name: Option<String>,
    pub namespace: Option<String>,
    pub node_name: Option<String>,
    pub container_id: Option<String>,
    pub memory_limit_bytes: Option<u64>,  // None when uncapped or bare metal
    pub cpu_quota_cores: Option<f64>,     // None when uncapped or bare metal
}
```

The `Environment` enum is the four-way classifier — anything that
isn't K8s, Docker, or a generic container is bare metal.

| Variant | Detected by |
|---------|-------------|
| `Kubernetes` | `/var/run/secrets/kubernetes.io/serviceaccount/token` exists, or `KUBERNETES_SERVICE_HOST` env var present |
| `Docker` | `/.dockerenv` exists |
| `Container` | `/proc/1/cgroup` or `/proc/1/mountinfo` mention `/docker/`, `/kubepods/`, `/lxc/`, `/containerd/` |
| `BareMetal` | none of the above |

Detection priority runs highest-confidence first — a K8s pod is
also a container, but you want to know it's K8s.

---

## Field detection

| Field | Source |
|-------|--------|
| `environment` | `Environment::detect()` |
| `pod_name` | `POD_NAME` env, falling back to `HOSTNAME` in container environments |
| `namespace` | `POD_NAMESPACE` env, falling back to `/var/run/secrets/kubernetes.io/serviceaccount/namespace` |
| `node_name` | `NODE_NAME` env |
| `container_id` | `HOSTNAME` env when in any container |
| `memory_limit_bytes` | `/sys/fs/cgroup/memory.max` (cgroup v2) — `None` if value is `"max"` |
| `cpu_quota_cores` | `/sys/fs/cgroup/cpu.max` (cgroup v2) — parsed as `quota / period`, `None` if `"max"` |

Bare metal short-circuits container-only reads — no cgroup probes
fire, all container-only fields stay `None`.

The cgroup v1 fallback for the memory limit lives in
[`memory/cgroup.rs`](../../src/memory/cgroup.rs) — the
`RuntimeContext` path only reads cgroup v2. Apps that need v1
compatibility for the limit value itself go through `MemoryGuard`,
which checks v2 then v1 then total system memory.

---

## Global singleton

```rust
use hyperi_rustlib::env::runtime_context;

let ctx = runtime_context();   // &'static RuntimeContext
tracing::info!(
    environment = %ctx.environment,
    pod_name = ?ctx.pod_name,
    "service starting"
);
```

`runtime_context()` is the only public access path. It returns a
`&'static RuntimeContext` populated on first call via
`RuntimeContext::detect()` and cached in `RUNTIME_CONTEXT:
OnceLock<RuntimeContext>`. Subsequent calls are a single atomic
load.

Modules read pod metadata, container ID, and cgroup limits through
this — no env-var probes scattered across the codebase, no
duplicated cgroup file reads.

---

## App env (separate from RuntimeContext)

`get_app_env()` resolves the deployment environment name (dev,
staging, prod) and is intentionally separate from `RuntimeContext`:

```rust
pub fn get_app_env() -> String {
    std::env::var("APP_ENV")
        .or_else(|_| std::env::var("ENVIRONMENT"))
        .or_else(|_| std::env::var("ENV"))
        .unwrap_or_else(|_| "development".to_string())
}
```

Precedence: `APP_ENV` → `ENVIRONMENT` → `ENV` → `"development"`.
The config cascade uses this to resolve `settings.{env}.yaml` — see
[CONFIG.md](../core-pillars/CONFIG.md).

`is_helm()` is the other helper in `env.rs` — returns true if
`HELM_RELEASE_NAME` is set or `/etc/podinfo/labels` contains
`helm.sh/chart` / `app.kubernetes.io/managed-by="Helm"`. Used by
the deployment-contract generator to decide which Argo CD or
Helm-specific labels to emit.

---

## XDG / container-aware paths

`RuntimePaths` in [`runtime.rs`](../../src/runtime.rs) is the path
resolver — it returns the right directory for config, secrets,
data, temp, logs, cache, and runtime files for the current
environment.

| Path | Container (K8s / Docker / Container) | Bare metal (XDG) |
|------|--------------------------------------|------------------|
| `config_dir` | `/app/config` | `$XDG_CONFIG_HOME/<app>` |
| `secrets_dir` | `/app/secrets` | `~/.<app>/secrets` |
| `data_dir` | `/app/data` | `$XDG_DATA_HOME/<app>` |
| `temp_dir` | `/app/tmp` | `$TMPDIR/<app>` |
| `logs_dir` | `/app/logs` | `$XDG_DATA_HOME/<app>/logs` |
| `cache_dir` | `/app/cache` | `$XDG_CACHE_HOME/<app>` |
| `run_dir` | `/app/run` | `$XDG_RUNTIME_DIR/<app>` |

The container base path defaults to `/app` and is overridable via
the `CONTAINER_BASE_PATH` env var. App name on bare metal comes
from `APP_NAME`, defaulting to `hs-app`.

```rust
use hyperi_rustlib::runtime::RuntimePaths;

let paths = RuntimePaths::discover();
paths.ensure_dirs()?;              // mkdir -p all of them
let cfg_path = paths.config_dir.join("settings.yaml");
```

`discover_for(env)` is the test seam — feed in a specific
`Environment` and bypass detection.

---

## API surface

| Item | Purpose |
|------|---------|
| `Environment` | Four-way enum: `Kubernetes`, `Docker`, `Container`, `BareMetal` |
| `Environment::detect()` | One-shot detection, returns the variant |
| `Environment::is_container()` / `is_kubernetes()` / `is_docker()` / `is_bare_metal()` | Convenience predicates |
| `RuntimeContext` | The rich metadata struct |
| `RuntimeContext::detect()` | Build a fresh context (used internally by the global) |
| `RuntimeContext::is_kubernetes()` / `is_container()` / `is_bare_metal()` | Delegate to the embedded `Environment` |
| `runtime_context() -> &'static RuntimeContext` | The pillar reader — cached singleton |
| `get_app_env()` | Deployment environment name for cascade resolution |
| `is_helm()` | Helm-deployment predicate |
| `RuntimePaths` | Resolved config / data / cache / run paths |
| `RuntimePaths::discover()` | Auto-detect environment and build paths |
| `RuntimePaths::ensure_dirs()` | `mkdir -p` everything |

---

## Testing

`RuntimeContext::detect()` reads env vars and the filesystem — for
tests, use the `temp-env` crate to scope env-var changes and assert
on the returned struct. Don't call `runtime_context()` in tests
that mutate env, because the global caches the first detection.

For path tests, `RuntimePaths::discover_for(Environment::Docker)`
sidesteps detection and lets you assert against a known
environment.

`std::env::set_var` is `unsafe` in edition 2024 and forbidden in
this crate — see the `temp-env` pattern in
[tests/common](../../tests/common/).

---

## Related

- [SERVICE-RUNTIME.md](SERVICE-RUNTIME.md) — `ServiceRuntime` stores `&'static RuntimeContext`
- [MEMORY.md](MEMORY.md) — `MemoryGuard` reads the cgroup limit
- [../core-pillars/CONFIG.md](../core-pillars/CONFIG.md) — `get_app_env()` drives `settings.{env}.yaml` resolution
- [../ARCHITECTURE.md](../ARCHITECTURE.md) — pillar overview
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) — `env`, `runtime`
- Source: [../../src/env.rs](../../src/env.rs),
  [../../src/runtime.rs](../../src/runtime.rs)
