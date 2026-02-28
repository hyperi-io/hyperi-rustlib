# Configuration Cascade

Reference for the `hyperi-rustlib` configuration system. The config module
provides a hierarchical cascade where each layer can override values from
layers below it.

Built on [Figment](https://docs.rs/figment), with `.env` loading via
[dotenvy](https://docs.rs/dotenvy) and YAML parsing via
[serde-yaml-ng](https://docs.rs/serde-yaml-ng).

**Feature flag:** `config`

---

## Layer Priority

Higher layers override lower layers. Later Figment `merge()` calls win.

```text
Priority (highest wins):

┌─────────────────────────────────────────────────────────────┐
│ 1. CLI arguments          (merged via merge_cli())          │
├─────────────────────────────────────────────────────────────┤
│ 2. Environment variables  (PREFIX_KEY, PREFIX_KEY__NESTED)  │
│    ↑ Includes .env values (loaded into env by dotenvy)      │
│    ↑ Real env vars always win over .env values              │
├─────────────────────────────────────────────────────────────┤
│ 3. PostgreSQL             (optional, config-postgres feat)  │
├─────────────────────────────────────────────────────────────┤
│ 4. settings.{env}.yaml    (e.g. settings.production.yaml)   │
├─────────────────────────────────────────────────────────────┤
│ 5. settings.yaml          (base application settings)       │
├─────────────────────────────────────────────────────────────┤
│ 6. defaults.yaml          (framework/library defaults)      │
├─────────────────────────────────────────────────────────────┤
│ 7. Hard-coded defaults    (log_level=info, log_format=auto) │
└─────────────────────────────────────────────────────────────┘
```

When the `config-postgres` feature is disabled, the PostgreSQL layer is
absent and the cascade has 6 effective layers. The numbering in source
comments refers to the full 8-layer version (counting `.env` as a
conceptual layer separate from env vars).

---

## Layer Details

### Layer 7: Hard-coded Defaults

The absolute fallback. Currently provides two values:

| Key | Default | Purpose |
|-----|---------|---------|
| `log_level` | `"info"` | Default log verbosity |
| `log_format` | `"auto"` | Auto-detect JSON (container) vs text (terminal) |

These are injected via `Serialized::defaults(HardcodedDefaults::default())`
as the first Figment merge — everything else overrides them.

### Layer 6: defaults.yaml

Framework-level defaults that ship with the application. Searched in:

1. `./defaults.yaml` or `./defaults.yml`
2. `./config/defaults.yaml` or `./config/defaults.yml`
3. Any paths in `ConfigOptions::config_paths`

Both `.yaml` and `.yml` are checked, `.yaml` first. If found in multiple
locations, all are merged (later paths override earlier).

### Layer 5: settings.yaml

Base application settings. Same search order as `defaults.yaml`:

1. `./settings.yaml` or `./settings.yml`
2. `./config/settings.yaml` or `./config/settings.yml`
3. Extra paths from `ConfigOptions::config_paths`

### Layer 4: settings.{env}.yaml

Environment-specific overrides. The `{env}` value is determined by:

1. `ConfigOptions::app_env` if explicitly set
2. Otherwise, auto-detected from environment variables:
   - `APP_ENV` (checked first)
   - `ENVIRONMENT`
   - `ENV`
   - Falls back to `"development"`

Example: in production with `APP_ENV=production`, the system loads
`settings.production.yaml`.

Common environment names: `development`, `staging`, `production`

### Layer 3: PostgreSQL (Optional — Built-For, Not Built-With)

**Feature flag:** `config-postgres`

**Status:** The PostgreSQL config layer is implemented and tested but is
**not currently used in production**. It exists so we can pivot to
centralised config management if needed in the future without redesigning
the cascade. File-based config + environment variables cover all current
deployment scenarios.

When enabled, configuration key-value pairs are loaded from a PostgreSQL
table. This layer sits above file-based config but below environment
variables, so database-stored config can be overridden by env vars in
emergencies without redeploying.

PostgreSQL config is loaded asynchronously via `Config::new_async()` or
`config::setup_async()`.

**Bootstrap problem:** The database connection itself is configured via
environment variables (not the cascade), using `PostgresConfigSource::from_env()`:

| Variable | Description | Default |
|----------|-------------|---------|
| `{PREFIX}_CONFIG_POSTGRES_ENABLED` | Enable this layer | `false` |
| `{PREFIX}_CONFIG_POSTGRES_URL` | Connection URL | — |
| `{PREFIX}_CONFIG_POSTGRES_NAMESPACE` | Multi-tenant namespace | `"default"` |
| `{PREFIX}_CONFIG_POSTGRES_CONNECT_TIMEOUT` | Connect timeout (secs) | `5` |
| `{PREFIX}_CONFIG_POSTGRES_QUERY_TIMEOUT` | Query timeout (secs) | `10` |
| `{PREFIX}_CONFIG_POSTGRES_RETRY_ATTEMPTS` | Retry count | `3` |
| `{PREFIX}_CONFIG_POSTGRES_RETRY_DELAY_MS` | Delay between retries | `1000` |
| `{PREFIX}_CONFIG_POSTGRES_OPTIONAL` | Continue if DB unavailable | `true` |

**Database schema:** expects a table with `key` (dot-notation string) and
`value` (JSON) columns, filtered by `namespace`.

**Fallback file:** When PostgreSQL is unavailable and `optional` is true,
config can be loaded from a cached JSON file. On successful DB load, the
config is written to the fallback file for future use.

| Variable | Description | Default |
|----------|-------------|---------|
| `{PREFIX}_CONFIG_FALLBACK_ENABLED` | Enable fallback file | `false` |
| `{PREFIX}_CONFIG_FALLBACK_FILE` | File path | — |
| `{PREFIX}_CONFIG_FALLBACK_MODE` | `replace` or `merge` | `replace` |

### Layer 2: Environment Variables

Environment variables with the configured prefix override all file-based
and database config. The prefix is set via `ConfigOptions::env_prefix`.

**Naming rules:**

| Pattern | Translates to | Example |
|---------|---------------|---------|
| `{PREFIX}_{KEY}` | flat key | `MYAPP_LOG_LEVEL` → `log_level` |
| `{PREFIX}_{A}__{B}` | nested key | `MYAPP_DATABASE__HOST` → `database.host` |
| `{PREFIX}_{A}__{B}__{C}` | deep nesting | `MYAPP_KAFKA__PRODUCER__ACKS` → `kafka.producer.acks` |

- Single underscore `_` separates words in a flat key (lowercased)
- Double underscore `__` creates dot-separated nesting
- All keys are lowercased during parsing
- The prefix is stripped automatically

**Implementation:** `Env::prefixed(&format!("{}_", prefix)).split("__")`

If `env_prefix` is empty, no environment variables are loaded into the
cascade (to avoid accidentally pulling in unrelated vars).

#### .env File Loading

`.env` files are loaded into the process environment via `dotenvy` *before*
the Figment cascade is built. This means `.env` values are available through
the environment variable layer — they are not a separate Figment provider.

**Default:** Only project `.env` is loaded. Home `~/.env` is opt-in.

**Load order** (project `.env` loaded first, home `.env` fills gaps):

1. Project `.env` (current working directory) — loaded via `dotenvy::dotenv()`
2. `~/.env` (home directory) — only when `load_home_dotenv = true`

Because `dotenvy` does **not** overwrite existing environment variables:

- Real env vars always win over `.env` values
- Project `.env` values win over `~/.env` values (loaded first, so they're
  already set when home `.env` is processed)

Control flags:

- `ConfigOptions::load_dotenv = false` — skip all `.env` loading
- `ConfigOptions::load_home_dotenv = true` — opt-in to load `~/.env`
  (default: `false`, matching hyperi-pylib's `dotenv_cascade`)

### Layer 1: CLI Arguments

CLI arguments have the highest priority. They are not loaded automatically —
the application merges them after construction:

```rust
let config = Config::new(opts)?
    .merge_cli(cli_args);  // cli_args must impl Serialize
```

This is typically a clap-derived struct with `#[derive(Serialize)]`.

---

## File Discovery

For each named config file (`defaults`, `settings`, `settings.{env}`), the
system searches these locations in order:

1. **Current directory:** `{name}.yaml`, then `{name}.yml`
2. **Config subdirectory:** `config/{name}.yaml`, then `config/{name}.yml`
3. **Container mount:** `/config/{name}.yaml`, then `{name}.yml`
   (always checked; no-op if `/config/` doesn't exist)
4. **User config:** `~/.config/{app_name}/{name}.yaml`, then `{name}.yml`
   (only checked when `app_name` is set via `ConfigOptions` or `APP_NAME`
   / `HYPERI_LIB_APP_NAME` env vars)
5. **Extra paths:** each path in `ConfigOptions::config_paths`, same extension order

If a file is found in multiple locations, all are merged. Later locations
override earlier ones within the same layer.

### App Name Resolution

The `app_name` used for user config discovery is resolved from:

1. `ConfigOptions::app_name` (explicit, highest priority)
2. `APP_NAME` environment variable
3. `HYPERI_LIB_APP_NAME` environment variable

If none are set, the user config directory is not searched.

---

## Merge Semantics

Figment uses **additive key-level merging**. Later merges override earlier
values at the individual key level, not the whole document:

```yaml
# defaults.yaml (layer 6)
database:
  host: localhost
  port: 5432
  pool_size: 10

# settings.production.yaml (layer 4)
database:
  host: db.prod.internal
  # port and pool_size are NOT lost — they carry forward from defaults
```

Result: `database.host = "db.prod.internal"`, `database.port = 5432`,
`database.pool_size = 10`

Environment variables fully replace at their key level:

```bash
MYAPP_DATABASE__HOST=db.override.internal
# Only overrides database.host, other database.* keys are untouched
```

---

## Accessing Config Values

### Getter Methods

```rust
let cfg = config::get();

cfg.get_string("database.host")          // Option<String>
cfg.get_int("database.port")             // Option<i64>
cfg.get_float("threshold")               // Option<f64>
cfg.get_bool("debug")                    // Option<bool>
cfg.get_duration("timeout")              // Option<Duration> — parses "30s", "5m", "1h"
cfg.get_string_list("kafka.brokers")     // Option<Vec<String>>
cfg.contains("some.key")                 // bool
```

### Typed Deserialisation

```rust
#[derive(Deserialize)]
struct DatabaseConfig {
    host: String,
    port: u16,
    pool_size: usize,
}

// Deserialise a sub-tree
let db: DatabaseConfig = cfg.unmarshal_key("database")?;

// Deserialise the entire config
let app: AppConfig = cfg.unmarshal()?;
```

### Duration Parsing

The `get_duration()` method parses human-readable strings:

| Input | Result |
|-------|--------|
| `"30s"` | 30 seconds |
| `"5m"` | 5 minutes (300s) |
| `"1h"` | 1 hour (3600s) |
| `"60"` | 60 seconds (plain number) |

---

## Global Singleton

The config module provides a global singleton for application-wide access:

```rust
// Initialise once at startup (sync)
config::setup(ConfigOptions {
    env_prefix: "MYAPP".into(),
    ..Default::default()
})?;

// Initialise once at startup (async, with PostgreSQL support)
config::setup_async(ConfigOptions { ... }).await?;

// Access anywhere
let cfg = config::get();       // panics if not initialised
let cfg = config::try_get();   // returns Option
```

`setup()` returns `ConfigError::AlreadyInitialised` if called twice.
`get()` panics if called before `setup()` — this is intentional to catch
missing initialisation early.

---

## Environment Variable Compatibility

The `env_compat` module (`config::env_compat`) provides standardised
environment variable definitions with legacy alias support. This is a
separate utility — it does not participate in the Figment cascade directly.

**How it works:**

1. Try the **standard** (preferred) variable name
2. If not set, try **legacy** (deprecated) names in order
3. If a legacy name is found, log a deprecation warning
4. Standard name always takes precedence if both are set

**Supported variable families:**

| Family | Standard prefix | Legacy prefixes |
|--------|----------------|-----------------|
| PostgreSQL | `PG*` | `POSTGRESQL_*`, `PG_*`, `POSTGRES_*` |
| Kafka | `KAFKA_*` | `KAFKA_BROKERS` → `KAFKA_BOOTSTRAP_SERVERS` |
| Vault/OpenBao | `VAULT_*` | `OPENBAO_*`, `BAO_*` |
| AWS | `AWS_*` | `AWS_ACCESS_KEY` → `AWS_ACCESS_KEY_ID` |
| ClickHouse | `CLICKHOUSE_*` | `CH_*` |

---

## Hot Reload

**Feature flag:** `config-reload`

Two components support config hot-reload:

### SharedConfig\<T\>

Thread-safe config holder with versioning:

```rust
let shared = SharedConfig::new(initial_config);

// Read (zero-copy via RwLock read guard)
let guard = shared.read();

// Closure-based read (avoids holding guard)
shared.with(|cfg| cfg.some_field.clone());

// Update atomically (bumps version, notifies watchers)
shared.update(new_config);

// Watch for changes
let mut rx = shared.subscribe();
```

Uses `parking_lot::RwLock` for efficient read-heavy access and a
`tokio::sync::watch` channel for change notifications. Each `update()`
increments a monotonic `AtomicU64` version counter.

### ConfigReloader\<T\>

Watches for changes and reloads config automatically. Three trigger modes
(any combination):

| Trigger | Description |
|---------|-------------|
| **SIGHUP** | Unix signal — standard daemon reload convention |
| **Periodic timer** | Reload every N seconds |
| **File polling** | Detect config file changes via mtime comparison |

The reloader:

1. Detects a trigger event
2. Debounces (default 500ms, prevents rapid reloads)
3. Calls `reload_fn` to produce new config
4. Calls `validate_fn` to check validity
5. If valid, updates the `SharedConfig`
6. If invalid, keeps the previous config and logs a warning

---

## Feature Flags

| Feature | Enables | Dependencies |
|---------|---------|--------------|
| `config` | Core cascade | figment, dotenvy, serde_yaml_ng, serde_json, toml, dirs, tracing |
| `config-reload` | Hot reload (`SharedConfig`, `ConfigReloader`) | config + parking_lot, tokio |
| `config-postgres` | PostgreSQL layer | config + sqlx, tokio, serde_json |

---

## Usage Example

```rust
use hyperi_rustlib::config::{self, Config, ConfigOptions};

// Basic setup
config::setup(ConfigOptions {
    env_prefix: "MYAPP".into(),
    load_dotenv: true,
    ..Default::default()
})?;

let cfg = config::get();

// Values follow the cascade: CLI > env > files > defaults
let host = cfg.get_string("database.host").unwrap_or_default();
let port = cfg.get_int("database.port").unwrap_or(5432);
let timeout = cfg.get_duration("request.timeout");
```

**With CLI args:**

```rust
#[derive(clap::Parser, serde::Serialize)]
struct Args {
    #[arg(long)]
    host: Option<String>,
    #[arg(long)]
    port: Option<u16>,
}

let args = Args::parse();
let config = Config::new(opts)?.merge_cli(args);
```

**With PostgreSQL (async):**

```rust
config::setup_async(ConfigOptions {
    env_prefix: "MYAPP".into(),
    ..Default::default()
}).await?;
```

---

## Source Files

| File | Purpose |
|------|---------|
| [src/config/mod.rs](../src/config/mod.rs) | Core cascade, `Config` struct, singleton |
| [src/config/env_compat.rs](../src/config/env_compat.rs) | Legacy env var aliases with deprecation |
| [src/config/postgres.rs](../src/config/postgres.rs) | PostgreSQL config source (feature-gated) |
| [src/config/reloader.rs](../src/config/reloader.rs) | `ConfigReloader` for hot-reload (feature-gated) |
| [src/config/shared.rs](../src/config/shared.rs) | `SharedConfig<T>` thread-safe holder (feature-gated) |
