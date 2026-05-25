# Native Deps

rustlib dynamically links against system C libraries — rdkafka,
libgit2, zstd, openssl, zlib — instead of statically compiling them.
Eliminates ~30 minutes of C++ build time per CI run. The cost: the
runtime container needs the `.so` files present.

`NativeDepsContract` is the bookkeeping. The Dockerfile generator
reads it and emits the right `apt-get` block — no per-app hand-coding,
no "I forgot to add `libssl3` to the runtime image" outages.

---

## Auto-detection from features

The dominant pattern is `for_rustlib_features()`. Pass the same
feature flags the app enables on hyperi-rustlib, get back the runtime
packages and any custom APT repos:

```rust
use hyperi_rustlib::deployment::NativeDepsContract;

let deps = NativeDepsContract::for_rustlib_features(
    &["transport-kafka", "spool", "tiered-sink", "secrets"],
    "ubuntu:24.04",
);
// deps.apt_repos    = [Confluent repo (librdkafka1, codename=noble)]
// deps.apt_packages = ["libssl3", "zlib1g", "libzstd1"]
```

The base image string is used to pick the right APT codename for
custom repos:

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
| `spool`, `tiered-sink` | — | `libzstd1` |
| `http`, `secrets*`, `transport*`, `config-postgres`, `otel*` | — | `libssl3`, `zlib1g` |
| `directory-config-git` | — | `libgit2-1.7` |
| Pure-Rust features (`cli`, `logger`, `deployment`, `metrics`, ...) | — | none |

Deduplication is automatic — enabling both `transport-kafka` and
`http` adds `libssl3` once.

---

## Confluent repo auto-add

`librdkafka` versions in Debian/Ubuntu repos lag the protocol. The
Confluent APT repo carries the current build and is auto-added when
the Kafka feature is detected. The generated Dockerfile fragment:

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

`gnupg` is pulled in automatically whenever any custom repo is needed
(for `gpg --dearmor`).

---

## Build host vs runtime host

The split that matters:

| Where | Needs |
|-------|-------|
| **Build host** (CI runner doing `cargo build`) | `-dev` packages: `librdkafka-dev`, `libgit2-dev`, `libzstd-dev`, `libssl-dev`, `zlib1g-dev` |
| **Runtime host** (the container image) | `.so` runtimes: `librdkafka1`, `libgit2-1.7`, `libzstd1`, `libssl3`, `zlib1g` |

`NativeDepsContract` only describes the **runtime** side — that's
what ships in the image. hyperi-ci handles the build-host packages
separately by sniffing `Cargo.lock` for `-sys` crates and installing
matching `-dev` packages on the runner.

---

## Reading from `Cargo.toml`

For tooling that doesn't want to hard-code the feature list,
`from_cargo_toml()` parses the `hyperi-rustlib` features array out of
the app's `Cargo.toml` and runs the same mapping:

```rust
let deps = NativeDepsContract::from_cargo_toml(
    Path::new("Cargo.toml"),
    "ubuntu:24.04",
);
```

Both single-line and multi-line `features = [...]` forms are
recognised. Returns empty (`is_empty() == true`) on parse failure or
when the dependency is absent — no panic, no error, no surprise build
break.

---

## Empty by default

`NativeDepsContract::default()` is empty. A contract that doesn't
populate `native_deps` produces a Dockerfile with only the base
packages (`ca-certificates`, `curl`, `netcat-openbsd`,
`iputils-ping`). Apps must opt in by populating the field — usually
via `for_rustlib_features()`.

Rationale: a service that only uses pure-Rust features shouldn't
carry unused system libraries. Opt-in keeps the image lean.

---

## Codename override

Apps that build against a base image rustlib doesn't recognise can
populate `AptRepoContract::codename` directly — the field is empty
when derived, but if you build the contract by hand and set it
explicitly, the generator uses your value as-is. Useful when running
on a derivative like `linuxserver/baseimage-debian:bookworm` where
the substring match catches the right codename anyway, or on a
private base image where it doesn't.

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

There is no `validate_native_deps()` — the generator is the source of
truth. To catch drift between an app's features and its installed
packages, run `generate-artefacts` in CI and diff the produced
`Dockerfile.runtime` against the committed copy. See
[ARTEFACTS.md](ARTEFACTS.md) for the drift-detection pattern.

---

## API surface

| Item | Purpose |
|------|---------|
| `NativeDepsContract` | The contract — `apt_repos` + `apt_packages` |
| `NativeDepsContract::for_rustlib_features(&[..], base)` | Build from feature names |
| `NativeDepsContract::from_cargo_toml(path, base)` | Parse features out of `Cargo.toml` |
| `NativeDepsContract::is_empty()` | True if no packages to install |
| `AptRepoContract` | One custom APT repo (`key_url`, `keyring`, `url`, `codename`, `packages`) |

---

## Related

- [CONTRACT.md](CONTRACT.md) — `native_deps` field on the contract
- [ARTEFACTS.md](ARTEFACTS.md) — the generated APT block in `Dockerfile.runtime`
- [../FEATURE-FLAGS.md](../FEATURE-FLAGS.md) — which features pull which deps
- README — full build-host vs runtime-host package tables
- Source: [../../src/deployment/native_deps.rs](../../src/deployment/native_deps.rs)
