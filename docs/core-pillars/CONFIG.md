# Config

The config cascade is what makes the auto-wiring pitch work — a service
that calls `config::setup` once at startup has access to a single
typed view of every layer (CLI args, env vars, YAML files, defaults)
through the same API.

The cascade is **8 layers when PostgreSQL is wired**, **7 layers
otherwise** — the same code path, gated by the `config-postgres`
feature.

---

## Cascade order (highest priority first)

| # | Source | Notes |
|---|--------|-------|
| 1 | CLI args | App calls `Config::merge_cli(args)` after `Config::new` |
| 2 | Environment variables | Prefix from `ConfigOptions::env_prefix`, double-underscore for nesting (`DFE_LOADER__KAFKA__BROKERS`) |
| 3 | `.env` file | Loaded by `dotenvy` into env vars — same precedence as layer 2 |
| 4 | PostgreSQL (optional) | Only present if `config-postgres` feature is on |
| 5 | `settings.{env}.yaml` | Per-environment overrides; `{env}` resolved from `APP_ENV` / `ENVIRONMENT` / `ENV` (default `"development"`) |
| 6 | `settings.yaml` | Team defaults, committed |
| 7 | `defaults.yaml` | Safe fallback baseline |
| 8 | Hard-coded defaults | `HardcodedDefaults` struct — `log_level=info`, `log_format=auto` |

Each YAML layer is searched in this order, first match wins for that
layer:

```
./<name>.yaml             ./<name>.yml
./config/<name>.yaml      ./config/<name>.yml
/config/<name>.yaml       /config/<name>.yml
~/.config/<app_name>/<name>.yaml      ~/.config/<app_name>/<name>.yml
```

`<app_name>` resolves from `ConfigOptions::app_name`, then `APP_NAME`,
then `HYPERI_LIB_APP_NAME`, then `None` (the home-config path is
skipped if no name is available).

---

## Usage

### One-shot setup

```rust
use hyperi_rustlib::config::{self, ConfigOptions};

config::setup(ConfigOptions {
    env_prefix: "DFE_LOADER".into(),
    ..Default::default()
})?;
```

After that, any module can pull from the cascade:

```rust
use hyperi_rustlib::config;

let cfg = config::get();
let brokers: Vec<String> = cfg.get_string_list("kafka.brokers").unwrap();
let timeout = cfg.get_duration("kafka.session_timeout").unwrap_or(Duration::from_secs(30));
```

### Typed deserialisation

The dominant pattern. Define a `serde` struct, deserialise the whole
config or a section into it:

```rust
#[derive(Deserialize)]
struct LoaderConfig {
    kafka: KafkaConfig,
    clickhouse: ClickHouseConfig,
}

let cfg: LoaderConfig = config::get().unmarshal()?;
// or
let kafka: KafkaConfig = config::get().unmarshal_key("kafka")?;
```

### Registered sections

Use `unmarshal_key_registered::<T>(key)` instead of `unmarshal_key` and
the section auto-registers in the global section registry. The
`/config` admin endpoint dumps every registered section (with secrets
redacted) without any per-section wiring at the endpoint side.

```rust
let kafka: KafkaConfig = config::get()
    .unmarshal_key_registered::<KafkaConfig>("kafka")?;
```

Modules that have sensible defaults expose a `from_cascade` constructor
that wraps this call:

```rust
let engine = TransportFilterEngine::from_cascade("filters")?;
```

---

## Sensitive fields

Three concentric defences against accidental secret leakage:

1. `#[serde(skip_serializing)]` on secret fields — most explicit but
   easy to forget.
2. Heuristic name-based redaction at the logging layer — catches
   commonly-named fields (`password`, `token`, `api_key`,
   `client_secret`, ...).
3. `SensitiveString` newtype — always serialises as `"***REDACTED***"`
   regardless of caller. Only `.expose()` reveals the value.

Use `SensitiveString` for anything that crosses a serialisation
boundary. The `.expose()` call sites are grep-able for review:

```rust
#[derive(Deserialize)]
struct KafkaConfig {
    pub brokers: Vec<String>,
    pub username: Option<SensitiveString>,
    pub password: Option<SensitiveString>,
}

// At the consumption site:
let creds = format!("{}:{}",
    cfg.kafka.username.as_ref().unwrap().expose(),
    cfg.kafka.password.as_ref().unwrap().expose());
```

See [sensitive.rs](../../src/config/sensitive.rs).

---

## Flat-env bridge (Kubernetes)

K8s `ConfigMap` and `env:` blocks emit flat env vars
(`DFE_LOADER_KAFKA_BROKERS=...`). Figment's nested-env convention is
double-underscore (`DFE_LOADER__KAFKA__BROKERS`). Without bridging, K8s
env-var overrides silently fail to apply.

The `flat_env` module provides the bridge:

```rust
use hyperi_rustlib::config::flat_env::ApplyFlatEnv;

let cfg = Figment::new()
    .merge(Yaml::file("settings.yaml"))
    .apply_flat_env("DFE_LOADER_");
```

The function reads `DFE_LOADER_KAFKA_BROKERS` and merges it as
`kafka.brokers`. Apps now accept both flat (K8s-style) and nested
(double-underscore) overrides.

`load_typed(path, prefix)` and the `DfeApp::load_config` recipe wire
this in automatically. See [flat_env.rs](../../src/config/flat_env.rs).

---

## Hot-reload

`ConfigReloader<T>` watches the config file(s) and rebuilds the typed
view on change. Consumers see the new value through `SharedConfig<T>`:

```rust
let shared: SharedConfig<LoaderConfig> = ConfigReloader::start(
    "settings.yaml",
    Duration::from_secs(10),
    shutdown.clone(),
)?;

// Reader path — lock-free atomic swap:
let current = shared.load();
```

The reloader calls `registry::update(key)` after a successful reload,
which fires any subscribers registered via `registry::on_change(key, cb)`.
Modules that need to react to a section change subscribe at startup:

```rust
config::registry::on_change("kafka", |new_section| {
    tracing::info!("kafka config changed, reconnecting");
    // ...
});
```

See [reloader.rs](../../src/config/reloader.rs) and
[shared.rs](../../src/config/shared.rs).

---

## `/config` admin endpoint

When `http-server` + `cli-service` are enabled and
`enable_config_endpoint = true` in `ConfigOptions`, the runtime mounts
`GET /config` on the metrics server. The response is JSON, every
registered section dumped with secrets redacted via
`SensitiveString`'s `Serialize` impl.

Opt-in. Default is **off** to avoid exposing config on a port that may
be reachable inside the cluster.

See [http_server/config.rs](../../src/http_server/config.rs).

---

## API surface

| Item | Purpose |
|------|---------|
| `config::setup(opts)` | One-shot init — builds the cascade and stores it in the global `OnceLock` |
| `config::get() -> &Config` | Pillar reader, panics if not initialised |
| `config::try_get() -> Option<&Config>` | Pillar reader, returns `None` if not initialised |
| `Config::new(opts)` | Build a cascade without installing it globally (for tests) |
| `Config::new_async(opts)` | Async variant when `config-postgres` is on |
| `Config::merge_cli(args)` | Append CLI args as the highest-priority layer |
| `Config::get_*(key)` | Typed lookups for primitives (`String`, `i64`, `f64`, `bool`, `Duration`, `Vec<String>`) |
| `Config::unmarshal::<T>()` | Deserialise the whole cascade into `T` |
| `Config::unmarshal_key::<T>(key)` | Deserialise one section into `T` |
| `Config::unmarshal_key_registered::<T>(key)` | Same, plus registers the section |
| `config::flat_env::ApplyFlatEnv` | K8s flat-env bridge trait |
| `config::flat_env::load_typed(path, prefix)` | Convenience cascade-+-flat-env loader returning `T` |
| `config::registry::on_change(key, cb)` | Subscribe to reloads of a section |
| `config::registry::sections() -> Vec<...>` | Dump the registry (powers `/config`) |
| `config::SensitiveString` | Redacted-on-serialise wrapper |
| `ConfigReloader::start(path, interval, shutdown)` | Hot-reload driver |
| `SharedConfig<T>::load()` | Lock-free reader for the reloader's current value |

---

## Testing

The cascade reads env vars and files. For tests that don't want to
touch the global, build a `Config` directly:

```rust
let cfg = Config::new(ConfigOptions {
    env_prefix: "TEST".into(),
    config_paths: vec!["tests/fixtures/config".into()],
    load_dotenv: false,
    ..Default::default()
})?;
```

For tests that mutate env vars, use the `temp-env` crate — `std::env::set_var`
is `unsafe` in edition 2024 and we don't allow it in this crate. The
test-helper `setup_test_env` pattern is in
[tests/common](../../tests/common/).

---

## Related

- [AUTO-WIRING.md](../AUTO-WIRING.md) — config in the singleton model
- [INTEGRATION.md](../INTEGRATION.md) — `DfeApp::load_config` recipe
- [core-pillars/LOGGING.md](LOGGING.md) — sensitive-field masking in logs
- [FEATURE-FLAGS.md](../FEATURE-FLAGS.md) — `config`, `config-reload`,
  `config-postgres`
