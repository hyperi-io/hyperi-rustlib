# Config

Call `config::setup` once at startup; every module then reads one typed view of
the whole cascade through the same API.

The cascade is **8 layers with PostgreSQL wired, 7 otherwise** -- same code path,
gated by the `config-postgres` feature.

---

## Cascade order (highest priority first)

| # | Source | Notes |
|---|--------|-------|
| 1 | CLI args | `Config::merge_cli(args)` after `Config::new` |
| 2 | Environment variables | Prefix from `ConfigOptions::env_prefix`, double-underscore for nesting (`DFE_LOADER__KAFKA__BROKERS`) |
| 3 | `.env` file | Loaded by `dotenvy` into env vars -- same precedence as layer 2 |
| 4 | PostgreSQL (optional) | Only present with `config-postgres` (async path) |
| 5 | `settings.{env}.yaml` | `{env}` from `APP_ENV` / `ENVIRONMENT` / `ENV` (default `development`) |
| 6 | `settings.yaml` | Team defaults, committed |
| 7 | `defaults.yaml` | Fallback baseline |
| 8 | Hard-coded defaults | `log_level=info`, `log_format=auto` |

Each YAML layer is searched in this order, first match wins:

```
./<name>.yaml             ./<name>.yml
./config/<name>.yaml      ./config/<name>.yml
/config/<name>.yaml       /config/<name>.yml
~/.config/<app_name>/<name>.yaml      ~/.config/<app_name>/<name>.yml
```

`<app_name>` resolves from `ConfigOptions::app_name`, then `APP_NAME`, then
`HYPERI_LIB_APP_NAME`. No name -> the home-config path is skipped.

---

## Usage

### Setup + ad-hoc reads

```rust
use hyperi_rustlib::config::{self, ConfigOptions};

config::setup(ConfigOptions { env_prefix: "DFE_LOADER".into(), ..Default::default() })?;

let cfg = config::get();
let brokers = cfg.get_string_list("kafka.brokers").unwrap();
let timeout = cfg.get_duration("kafka.session_timeout").unwrap_or(Duration::from_secs(30));
```

`get_string` / `get_int` / `get_bool` / `get_duration` / `get_string_list` cover
the primitive lookups.

### Typed deserialisation (dominant pattern)

```rust
let cfg: LoaderConfig = config::get().unmarshal()?;          // whole cascade
let kafka: KafkaConfig = config::get().unmarshal_key("kafka")?;  // one section
```

### Registered sections

`unmarshal_key_registered::<T>(key)` deserialises a section AND auto-registers it
in the global section registry, so the `/config` endpoint can dump it with no
per-section wiring.

```rust
let kafka: KafkaConfig = config::get().unmarshal_key_registered::<KafkaConfig>("kafka")?;
```

Modules with sensible defaults expose a `from_cascade` constructor wrapping this
call: `TransportFilterEngine::from_cascade("filters")?`.

---

## Sensitive fields

Masking is applied at the serialisation boundary, not at the call site. Three
concentric defences:

1. `#[serde(skip_serializing)]` on secret fields -- explicit but easy to forget.
2. Name-based redaction (registry dump + logger) -- catches commonly-named fields
   (`password`, `token`, `api_key`, `client_secret`, ...).
3. `SensitiveString` newtype -- always serialises as `"***REDACTED***"`, prints
   the same in `Debug` / `Display`. Only `.expose()` reveals the value, and call
   sites are grep-able for review.

Use `SensitiveString` for anything crossing a serialisation boundary:

```rust
#[derive(Deserialize)]
struct KafkaConfig {
    pub brokers: Vec<String>,
    pub username: Option<SensitiveString>,
    pub password: Option<SensitiveString>,
}

let creds = format!("{}:{}",
    cfg.kafka.username.as_ref().unwrap().expose(),
    cfg.kafka.password.as_ref().unwrap().expose());
```

See [sensitive.rs](../../src/config/sensitive.rs).

---

## Flat-env bridge (Kubernetes)

K8s `ConfigMap` / `env:` blocks emit flat env vars (`DFE_LOADER_KAFKA_BROKERS=...`),
but Figment's nested convention is double-underscore (`DFE_LOADER__KAFKA__BROKERS`).
Without bridging, K8s overrides silently fail to apply.

```rust
use hyperi_rustlib::config::flat_env::ApplyFlatEnv;

let cfg = Figment::new()
    .merge(Yaml::file("settings.yaml"))
    .apply_flat_env("DFE_LOADER_");
```

`apply_flat_env` reads `DFE_LOADER_KAFKA_BROKERS` and merges it as `kafka.brokers`,
so apps accept both flat (K8s) and nested (double-underscore) forms.
`flat_env::load_config(path, prefix)` and the `DfeApp::load_config` recipe wire
this in. See [flat_env.rs](../../src/config/flat_env.rs).

---

## Hot-reload

`ConfigReloader<T>` watches the config file (and/or SIGHUP / periodic timer),
rebuilds the typed view on change, and publishes it through `SharedConfig<T>`.
The reloader is built with a reload closure, then started:

```rust
let reloader = ConfigReloader::new(reloader_config, shared.clone(), reload_fn)
    .with_registry_update("kafka");   // fire registry::update("kafka") after reload
let _handle = reloader.start();        // -> JoinHandle<()>; spawns the watch task

// Reader path -- lock-free:
let current = shared.get();            // clone; or shared.read() for a guard
```

`with_registry_update(key)` makes the reloader call `registry::update(key)` after a
successful reload, firing any `registry::on_change(key, cb)` subscribers:

```rust
config::registry::on_change("kafka", |new_section| {
    tracing::info!("kafka config changed, reconnecting");
});
```

See [reloader.rs](../../src/config/reloader.rs) and [shared.rs](../../src/config/shared.rs).

---

## `/config` admin endpoint

Set `enable_config_endpoint = true` (in the HTTP server options) and the runtime
mounts `GET /config` on the metrics server, returning JSON with every registered
section, secrets redacted via `SensitiveString`'s `Serialize` impl.

Opt-in, **off by default** -- it would otherwise expose config on a port reachable
inside the cluster. See [http_server/config.rs](../../src/http_server/config.rs).

---

## API surface

| Item | Purpose |
|------|---------|
| `config::setup(opts)` | One-shot init -- builds the cascade into the global `OnceLock` |
| `config::get() -> &Config` | Reader, panics if not initialised |
| `config::try_get() -> Option<&Config>` | Reader, `None` if not initialised |
| `Config::new(opts)` / `Config::new_async(opts)` | Build without installing globally (`_async` for `config-postgres`) |
| `Config::merge_cli(args)` | Append CLI args as the highest-priority layer |
| `Config::get_*(key)` | Typed primitive lookups (`String`, `i64`, `f64`, `bool`, `Duration`, `Vec<String>`) |
| `Config::unmarshal::<T>()` | Deserialise the whole cascade into `T` |
| `Config::unmarshal_key::<T>(key)` | Deserialise one section |
| `Config::unmarshal_key_registered::<T>(key)` | Same, plus registers the section |
| `config::flat_env::ApplyFlatEnv` | K8s flat-env bridge trait |
| `config::flat_env::load_config(path, prefix)` | Cascade + flat-env loader returning `T` |
| `config::registry::on_change(key, cb)` | Subscribe to reloads of a section |
| `config::registry::update(key, &value)` | Re-register a section + notify subscribers |
| `config::registry::sections() -> Vec<...>` | Dump the registry (powers `/config`) |
| `config::SensitiveString` | Redacted-on-serialise wrapper; `.expose()` to read |
| `ConfigReloader::new(...).start() -> JoinHandle<()>` | Hot-reload driver (builder-style) |
| `SharedConfig<T>::get()` / `read()` | Lock-free reader for the reloader's current value |

---

## Testing

Build a `Config` directly to avoid touching the global:

```rust
let cfg = Config::new(ConfigOptions {
    env_prefix: "TEST".into(),
    config_paths: vec!["tests/fixtures/config".into()],
    load_dotenv: false,
    ..Default::default()
})?;
```

For tests mutating env vars, use the `temp-env` crate -- `std::env::set_var` is
`unsafe` in edition 2024 and this crate forbids unsafe. The `setup_test_env`
helper is in [tests/common](../../tests/common/).

---

## Related

- [AUTO-WIRING.md](../AUTO-WIRING.md) -- config in the singleton model
- [INTEGRATION.md](../INTEGRATION.md) -- `DfeApp::load_config` recipe
- [core-pillars/LOGGING.md](LOGGING.md) -- sensitive-field masking in logs
- [FEATURE-FLAGS.md](../FEATURE-FLAGS.md) -- `config`, `config-reload`, `config-postgres`
- Source: [`src/config/mod.rs`](../../src/config/mod.rs), [`src/config/registry.rs`](../../src/config/registry.rs), [`src/config/reloader.rs`](../../src/config/reloader.rs)
