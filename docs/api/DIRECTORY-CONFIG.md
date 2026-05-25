# Directory config

A YAML directory-backed config store — distinct from the [8-layer
config cascade](../core-pillars/CONFIG.md). Use this for ops-managed
config that changes between deploys without a code change: detection
rules, scrub patterns, routing tables, allow/deny lists, any artefact
that ops curates as a set of YAML files in a directory.

The cascade is for *application config* (set at startup, optionally
hot-reloaded). This is for *operational data* (read continuously,
written via tooling or git push).

---

## When to use which

| Need | Use |
|------|-----|
| App config (Kafka brokers, ports, log level) | [Config cascade](../core-pillars/CONFIG.md) |
| Per-environment overrides of app config | Cascade — `settings.{env}.yaml` |
| One YAML per logical object, ops adds/removes them | This module |
| File-system-shaped data store ops can edit by hand | This module |
| Git as the audit trail for changes | This module with `directory-config-git` |

If you find yourself growing the cascade into a "rules engine", you
want this module instead.

---

## Usage

```rust
use hyperi_rustlib::directory_config::{
    DirectoryConfigStore, DirectoryConfigStoreConfig,
};

let mut store = DirectoryConfigStore::new(DirectoryConfigStoreConfig {
    directory: "/etc/dfe/rules".into(),
    write_mode: WriteMode::ReadWrite,
    git: None,
    ..Default::default()
}).await?;

store.start().await?;

// List every YAML file in the directory (one per "table"):
let tables = store.list_tables().await;
// tables == ["scrub_patterns", "routing_rules", "allowlist", ...]

// Read one file as untyped YAML or typed struct:
let raw = store.get("scrub_patterns").await?;
let typed: ScrubPatterns = store.get_as("scrub_patterns").await?;

// Lookup a key within a table:
let v = store.get_key("routing_rules", "kafka.events.high").await?;
```

`start()` opens any required watch handles. `stop()` cleans them up.
The store stays usable through reloads — readers see the latest version
once a successful refresh completes.

---

## Writes and locking

When `write_mode` is `ReadWrite`, the store can mutate files in place:

```rust
store.set("routing_rules", "kafka.events.high", &Pattern { ... }).await?;
store.delete_key("routing_rules", "kafka.events.low").await?;
```

File-level locking via `std::fs::File::{lock,unlock}` (stable since
Rust 1.89) — no third-party lock crate. Concurrent writes from the
same process serialise; cross-process writes are also safe.

`ReadOnly` mode rejects writes at the API. Use this in services that
should never mutate the store (typical case — most consumers read,
only an admin tool writes).

---

## Git integration (optional)

With the `directory-config-git` feature, the store can commit and push
changes to a backing git repo via `libgit2`:

```rust
let config = DirectoryConfigStoreConfig {
    git: Some(GitBackend {
        remote: "git@github.com:org/dfe-rules.git".into(),
        branch: "main".into(),
        auto_pull_interval: Duration::from_secs(60),
        commit_message_template: "ops: {table} update".into(),
        ..Default::default()
    }),
    ..Default::default()
};
```

Each `set` / `delete_key` produces a git commit; pulls happen on the
configured interval (skipping if a write is in progress). The audit
trail of who-changed-what lives in git history.

`is_git() -> bool` and `current_branch() -> Option<String>` expose the
backing repo state.

---

## Change notifications

`on_change()` returns a `broadcast::Receiver<ChangeEvent>` consumers
can listen to:

```rust
let mut rx = store.on_change();
while let Ok(event) = rx.recv().await {
    tracing::info!(table = %event.table, "directory config changed");
    rebuild_rules_engine(event.table).await;
}
```

Events fire on writes (local or pulled from git). Use this for
hot-reload of derived data structures.

---

## Config shape

```yaml
directory_config:
  directory: /etc/dfe/rules
  write_mode: read_only         # read_only | read_write
  refresh_interval: 5s
  git:
    remote: "git@github.com:org/dfe-rules.git"
    branch: main
    auto_pull_interval: 60s
```

---

## API surface

| Item | Purpose |
|------|---------|
| `DirectoryConfigStore::new(config)` | Build a store (async — opens the directory) |
| `.start()` | Open watches and start refresh loop |
| `.stop()` | Shut down watches |
| `.list_tables() -> Vec<String>` | Every YAML file name in the directory |
| `.get(table) -> serde_yaml_ng::Value` | Read one table as untyped YAML |
| `.get_as::<T>(table) -> T` | Read and deserialise into a typed struct |
| `.get_key(table, key) -> serde_yaml_ng::Value` | Read one nested key |
| `.set(table, key, value)` | Mutate one nested key (requires `ReadWrite`) |
| `.delete_key(table, key)` | Remove one nested key |
| `.on_change() -> broadcast::Receiver<ChangeEvent>` | Subscribe to writes / pulls |
| `.write_mode() -> WriteMode` | Read back the store mode |
| `.is_git() -> bool` | True if backed by git |
| `.current_branch() -> Option<String>` | Backing repo branch name |

---

## Related

- [../core-pillars/CONFIG.md](../core-pillars/CONFIG.md) — app config cascade
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) — `directory-config`, `directory-config-git`
- Source: [../../src/directory_config/](../../src/directory_config/)
