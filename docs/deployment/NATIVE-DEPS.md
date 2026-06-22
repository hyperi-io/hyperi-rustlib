# Native Deps

rustlib dynamically links against system C libraries -- rdkafka,
libgit2, zstd, openssl, zlib -- instead of static compilation. Saves
~30 minutes of C++ build per CI run. Cost: the runtime container needs
the `.so` files present.

`NativeDepsContract` is the bookkeeping. The Dockerfile generator reads
it and emits the right `apt-get` block -- no per-app hand-coding, no "I
forgot to add `libssl3`" outages.

---

## Auto-detection from features

The usual path is `for_rustlib_features()`. Pass the same feature flags
the app enables on hyperi-rustlib, get back the runtime packages and
any custom APT repos:

```rust
use hyperi_rustlib::deployment::NativeDepsContract;

let deps = NativeDepsContract::for_rustlib_features(
    &["transport-kafka", "spool", "tiered-sink", "secrets"],
    "ubuntu:24.04",
);
// deps.apt_repos    = [Confluent repo (librdkafka1, codename=noble)]
// deps.apt_packages = ["libssl3", "zlib1g", "libzstd1"]
```

The base image string picks the APT codename for custom repos:

| Base image substring | Codename |
|----------------------|----------|
| `bookworm` | `bookworm` |
| `jammy` | `jammy` |
| `focal` | `focal` |
| anything else (incl. `ubuntu:24.04`) | `noble` |

---

## Feature -> package map

| Feature(s) | APT repo | Runtime packages |
|------------|----------|-------------------|
| `transport-kafka`, `dlq-kafka` (or any `dlq-kafka-*`) | Confluent (`packages.confluent.io/clients/deb`) | `librdkafka1`, `libssl3`, `zlib1g` |
| `spool`, `tiered-sink` | -- | `libzstd1` |
| `http`, `secrets*`, `transport*`, `config-postgres`, `otel*` | -- | `libssl3`, `zlib1g` |
| `directory-config-git` | -- | `libgit2-1.7` |
| Pure-Rust features (`cli`, `logger`, `deployment`, `metrics`, ...) | -- | none |

Deduplication is automatic: enabling both `transport-kafka` and `http`
adds `libssl3` once.

---

## Confluent repo auto-add

`librdkafka` in Debian/Ubuntu repos lags the protocol. The Confluent
APT repo carries the current build and is auto-added when the Kafka
feature is detected. Generated Dockerfile fragment:

```dockerfile
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl netcat-openbsd iputils-ping gnupg \
    && curl -fsSL https://packages.confluent.io/clients/deb/archive.key \
       | gpg --dearmor -o /usr/share/keyrings/confluent-clients.gpg \
    && echo "deb [signed-by=/usr/share/keyrings/confluent-clients.gpg] \
       https://packages.confluent.io/clients/deb noble main" \
       > /etc/apt/sources.list.d/confluent-clients.list \
    && apt-get update && apt-get install -y --no-install-recommends \
       librdkafka1 libssl3 zlib1g \
    && rm -rf /var/lib/apt/lists/*
```

`gnupg` is pulled in automatically whenever a custom repo is needed
(for `gpg --dearmor`).

---

## Build host vs runtime host

| Where | Needs |
|-------|-------|
| **Build host** (CI runner doing `cargo build`) | `-dev` packages: `librdkafka-dev`, `libgit2-dev`, `libzstd-dev`, `libssl-dev`, `zlib1g-dev` |
| **Runtime host** (the container image) | `.so` runtimes: `librdkafka1`, `libgit2-1.7`, `libzstd1`, `libssl3`, `zlib1g` |

`NativeDepsContract` describes the **runtime** side only -- what ships
in the image. hyperi-ci handles build-host packages separately by
sniffing `Cargo.lock` for `-sys` crates and installing matching `-dev`
packages on the runner.

---

## Reading from `Cargo.toml`

`from_cargo_toml()` parses the `hyperi-rustlib` features array from the
app's `Cargo.toml` and runs the same mapping -- for tooling that won't
hard-code the feature list:

```rust
let deps = NativeDepsContract::from_cargo_toml(
    Path::new("Cargo.toml"),
    "ubuntu:24.04",
);
```

Single-line and multi-line `features = [...]` forms are both
recognised. Returns empty (`is_empty() == true`) on parse failure or
when the dependency is absent -- no panic, no surprise build break.

---

## Empty by default

`NativeDepsContract::default()` is empty. A contract that doesn't
populate `native_deps` gives a Dockerfile with only base packages
(`ca-certificates`, `curl`, `netcat-openbsd`, `iputils-ping`). Apps
opt in by populating the field, usually via `for_rustlib_features()`.

A pure-Rust-feature service shouldn't carry unused system libraries.
Opt-in keeps the image lean.

---

## Codename override

For a base image rustlib doesn't recognise, set
`AptRepoContract::codename` directly. The field is empty when derived;
set it by hand and the generator uses your value as-is. Useful on a
private base image where the substring match misses.

```rust
let repo = AptRepoContract {
    key_url: "https://example.com/key.asc".into(),
    keyring: "/usr/share/keyrings/example.gpg".into(),
    url: "https://example.com/apt".into(),
    codename: "trixie".into(),       // explicit, no derivation
    packages: vec!["libexample0".into()],
};
```

---

## Validation

No `validate_native_deps()` -- the generator is the source of truth.
To catch drift between an app's features and its installed packages,
run `generate-artefacts` in CI and diff the produced `Dockerfile.runtime`
against the committed copy. See [ARTEFACTS.md](ARTEFACTS.md) for the
drift-detection pattern.

---

## API surface

| Item | Purpose |
|------|---------|
| `NativeDepsContract` | The contract -- `apt_repos` + `apt_packages` |
| `NativeDepsContract::for_rustlib_features(&[..], base)` | Build from feature names |
| `NativeDepsContract::from_cargo_toml(path, base)` | Parse features out of `Cargo.toml` |
| `NativeDepsContract::is_empty()` | True if no packages to install |
| `AptRepoContract` | One custom APT repo (`key_url`, `keyring`, `url`, `codename`, `packages`) |

---

## Related

- [CONTRACT.md](CONTRACT.md) -- `native_deps` field on the contract
- [ARTEFACTS.md](ARTEFACTS.md) -- the generated APT block in `Dockerfile.runtime`
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) -- which features pull which deps
- README -- full build-host vs runtime-host package tables
- Source: [../../src/deployment/native_deps.rs](../../src/deployment/native_deps.rs)
