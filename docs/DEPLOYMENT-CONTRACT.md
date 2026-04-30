# Deployment Contract — Architecture & Lifecycle

**Status:** active
**Schema version:** 2 (current)
**Audience:** rustlib, pylib, hyperi-ci, and downstream app maintainers

---

## What this is

A **language-agnostic JSON contract** describing everything a CI/CD pipeline
needs to know to deploy a HyperI service: container image config, Helm values,
KEDA scaling, ArgoCD wiring, runtime native deps. The Rust struct in this repo
is one *producer* of the JSON; pylib and hyperi-ci are the others.

Apps don't roll their own Dockerfile, Helm chart, ArgoCD Application, etc. They
declare the contract once; everything else is derived deterministically.

---

## The three-tier producer model

```
                ┌─────────────────────────────────────────┐
                │  deployment-contract.schema.json        │
                │  (JSON Schema — language-agnostic SSoT) │
                └──────────────────────┬──────────────────┘
                                       │
   ┌───────────────────────┬───────────┼──────────────────────────────────┐
   │                       │           │                                  │
   ▼                       ▼           ▼                                  ▼
┌──────────────┐    ┌──────────────┐    ┌─────────────────────────────────────┐
│ Tier 1       │    │ Tier 2       │    │ Tier 3                              │
│ rustlib      │    │ pylib        │    │ hyperi-ci (templater for the rest) │
│              │    │              │    │                                     │
│ Rust struct  │    │ Pydantic     │    │ Reads ci/deployment-contract.json   │
│ + generate_* │    │ model + gen  │    │ Templates Dockerfile/argocd/etc.    │
└──────┬───────┘    └──────┬───────┘    └──────────────────┬──────────────────┘
       │ <bin> generate-artefacts                          │ hyperi-ci emit-artefacts ci/
       │                   │                               │
       ▼                   ▼                               ▼
            ┌───────────────────────────────────────┐
            │ ci/  (committed in app repo)          │
            │  ├── deployment-contract.json         │  ← canonical SSoT instance
            │  ├── deployment-contract.schema.json  │  ← schema reference
            │  ├── Dockerfile                       │  ← generated
            │  ├── Dockerfile.runtime               │  ← generated (CI fragment)
            │  ├── container-manifest.json          │  ← generated
            │  ├── argocd-application.yaml          │  ← generated
            │  └── chart/                           │  ← generated Helm chart
            └────────────────────┬──────────────────┘
                                 │
                                 ▼
                        ┌─────────────────┐
                        │ hyperi-ci       │
                        │ Container stage │
                        │ (consumes ci/)  │
                        └─────────────────┘
```

### Tier 1 — Rust apps (rustlib)
- App embeds the Rust `DeploymentContract` struct in source
- Implements `DfeApp::deployment_contract()` returning it
- `<app> generate-artefacts --output-dir ci/` writes everything to `ci/`
- rustlib provides: `generate_dockerfile`, `generate_chart`, `generate_argocd_application`, `generate_container_manifest`, `generate_runtime_stage`, `generate_compose_fragment`

### Tier 2 — Python apps (pylib)
- App embeds the `DeploymentContract` Pydantic model in source
- Implements equivalent `Application.deployment_contract()` method
- `<app> generate-artefacts --output-dir ci/` writes everything to `ci/`
- pylib provides the same generator surface as rustlib (byte-identical output)

### Tier 3 — Anything else (bash, TypeScript, Go, etc.)
- App **manually maintains `ci/deployment-contract.json`** in the repo
- App's CI runs `hyperi-ci emit-artefacts ci/` (a hyperi-ci subcommand) which:
  - Reads `ci/deployment-contract.json`
  - Validates against `deployment-contract.schema.json`
  - Templates Dockerfile/argocd/Helm/etc. and writes back to `ci/`
- The hand-written JSON is the SSoT for non-framework apps; templating is
  centralised in hyperi-ci so bash apps get the same Dockerfile shape Rust
  apps get

---

## Schema versioning policy

The contract is versioned via `schema_version: u32`. Bump in **lockstep**
across rustlib, pylib, and hyperi-ci whenever the JSON shape changes:

| Change | New version? |
|---|---|
| Add an optional field (with serde default) | NO (consumers ignore unknown fields, missing field gets default) |
| Add a required field | YES |
| Remove a field | YES |
| Rename a field | YES |
| Change a field's type | YES |
| Re-shape the structure | YES |

**Producers** stamp the contract they emit with their `schema_version`.
**Consumers** (hyperi-ci CI stages) check `schema_version <= MAX_SUPPORTED`
and fail fast on too-new contracts. This protects against deploying with
a hyperi-ci that doesn't understand a newer contract shape yet.

Current: **schema_version = 2**.

---

## Contract shape (high level)

The full schema is in `deployment-contract.schema.json` (auto-derived from
the Rust struct via `schemars`). High-level fields:

```json
{
  "schema_version": 2,
  "app_name": "dfe-loader",
  "binary_name": "dfe-loader",
  "description": "...",
  "image_registry": "ghcr.io/hyperi-io",
  "base_image": "ubuntu:24.04",
  "image_profile": "production",
  "metrics_port": 9090,
  "health": {
    "liveness_path": "/healthz",
    "readiness_path": "/readyz",
    "metrics_path": "/metrics"
  },
  "env_prefix": "DFE_LOADER",
  "config_mount_path": "/etc/dfe/loader.yaml",
  "extra_ports": [...],
  "secrets": [...],
  "depends_on": ["kafka", "clickhouse"],
  "keda": {...},
  "native_deps": {
    "apt_packages": ["librdkafka1", "libssl3", ...],
    "apt_repos": [...]
  },
  "oci_labels": {...}
}
```

Defaults that should be honoured by all three producers:

| Field | Default | Source |
|---|---|---|
| `image_registry` | `ghcr.io/hyperi-io` | rustlib `DEFAULT_IMAGE_REGISTRY` |
| `base_image` | `ubuntu:24.04` | rustlib `DEFAULT_BASE_IMAGE` |
| `metrics_port` | `9090` | convention |
| `health.liveness_path` | `/healthz` | convention |
| `health.readiness_path` | `/readyz` | convention |
| `health.metrics_path` | `/metrics` | convention |
| ArgoCD `repo_url` | `https://github.com/hyperi-io/{app_name}` | rustlib `argocd_repo_url_from_cascade()` |

Producers may also read these from the YAML config cascade keys
`deployment.image_registry`, `deployment.base_image`,
`deployment.argocd.repo_url` — same fallback chain. This lets ops change
the org-wide default in one config file rather than per-app source.

---

## Generated artefacts (the `ci/` directory)

| File | Producer | Purpose |
|---|---|---|
| `deployment-contract.json` | producer-emitted (or hand-written for Tier 3) | SSoT instance for this app |
| `deployment-contract.schema.json` | rustlib (copied from upstream) | Schema reference |
| `Dockerfile` | producer-emitted | Local `docker build .` works |
| `Dockerfile.runtime` | producer-emitted | Runtime-stage fragment hyperi-ci composes the production image from |
| `container-manifest.json` | producer-emitted | Build config hyperi-ci needs (image name, tag, platforms, build args) |
| `argocd-application.yaml` | producer-emitted | ArgoCD `Application` CR — applied to the argocd cluster |
| `chart/` | producer-emitted | Full Helm chart the ArgoCD Application points at |
| `metrics-manifest.json` | producer-emitted | Metric catalogue (separate, but emitted by same command) |

Every generated file carries an autogenerated header:

```dockerfile
# AUTOGENERATED — do not edit by hand.
# Generated by hyperi-rustlib::deployment::generate_dockerfile()
# Schema version: 2
# Source contract: dfe-loader::deployment::contract()
# Regenerate with: `dfe-loader emit-dockerfile > Dockerfile`
```

---

## Doctrine — hybrid commit-and-regenerate

**Operationally Pattern B:** every CI build regenerates artefacts from the
contract before docker build. The committed copies are *not* trusted by CI.

**Visibly Pattern A:** generated artefacts are still committed to the repo
under `ci/` for human review (PR diffs, ArgoCD ops folks looking at the app
repo, local `docker build .`).

**Drift detection:** Quality stage runs the producer against a temp dir,
diffs against committed `ci/`, fails on diff. Forces dev to re-run
`<app> generate-artefacts --output-dir ci/` and commit when the contract
changes.

This means:
- ✅ One SSoT (the contract in producer source / hand-written JSON for Tier 3)
- ✅ CI never builds with a stale Dockerfile (regen step before Container)
- ✅ Repo visibility (`Dockerfile` is there, ArgoCD config is there)
- ✅ Drift detected at quality stage, not after deployment

---

## CI stage integration (hyperi-ci responsibilities)

### `Quality` stage — drift check
After existing quality checks, run:
```bash
# Tier 1 (Rust)
cargo build --features cli-service,deployment
./target/debug/<app> generate-artefacts --output-dir /tmp/drift/

# Tier 2 (Python)
<app> generate-artefacts --output-dir /tmp/drift/

# Tier 3 (anything else)
hyperi-ci emit-artefacts /tmp/drift/ --from ci/deployment-contract.json

# Common
diff -r /tmp/drift/ ci/ || (
  echo "::error::deployment artefacts drift from contract; re-run generate-artefacts and commit"
  exit 1
)
```

Auto-detect tier:
1. Has `Cargo.toml` with `hyperi-rustlib` dep → Tier 1
2. Has `pyproject.toml` with `hyperi-pylib` dep → Tier 2
3. Has `ci/deployment-contract.json` only → Tier 3

### `Generate` stage — new, between Build and Container
After Build produces the binary in `dist/`:
```bash
mkdir -p ci-tmp/

# Tier 1 (binary already built in Build stage)
dist/<app>-linux-amd64 generate-artefacts --output-dir ci-tmp/

# Tier 2 (Python — installed in venv)
<app> generate-artefacts --output-dir ci-tmp/

# Tier 3
hyperi-ci emit-artefacts ci-tmp/ --from ci/deployment-contract.json
```

The Container stage uses `ci-tmp/Dockerfile.runtime` and
`ci-tmp/container-manifest.json` — NOT the repo's `ci/` files.

### `Container` stage — consumes generated, not committed
```bash
# Read ci-tmp/container-manifest.json for build config
# (image_name, tag, platforms, oci_labels, build_args)

docker buildx build \
  --file ci-tmp/Dockerfile.runtime \
  --tag <registry>/<app_name>:<version> \
  --label <oci_labels from manifest> \
  --platform <platforms from manifest> \
  --push \
  .
```

### Schema version enforcement
At the start of every stage that consumes the contract:
```python
contract = json.load(open("ci/deployment-contract.json"))
if contract["schema_version"] > HYPERCI_MAX_SCHEMA_VERSION:
    fail("contract schema_version=X but hyperi-ci supports up to Y; upgrade hyperi-ci")
```

---

## hyperi-ci `emit-artefacts` subcommand spec

New subcommand for Tier 3 (and used internally for drift checks).

```
hyperi-ci emit-artefacts <output-dir> [--from <contract-json>]

  <output-dir>           Directory to write artefacts to (e.g., ci/, /tmp/drift/)
  --from <path>          Path to deployment-contract.json (default: ci/deployment-contract.json)

Writes:
  <output-dir>/Dockerfile
  <output-dir>/Dockerfile.runtime
  <output-dir>/container-manifest.json
  <output-dir>/argocd-application.yaml
  <output-dir>/chart/...
  <output-dir>/deployment-contract.schema.json  (copied from embedded)

Exits non-zero if:
  - <contract-json> missing or invalid against schema
  - schema_version > MAX_SUPPORTED
  - I/O errors writing output
```

Templates inside hyperi-ci are Python implementations that produce the
**byte-identical** output as rustlib's `generate_dockerfile()` etc. for the
same JSON input. Tested via the cross-language parity suite (below).

---

## Cross-language parity testing

A shared test fixture set lives in `hyperi-rustlib/tests/parity/fixtures/`:

```
fixtures/
  basic/
    contract.json
    expected/
      Dockerfile
      Dockerfile.runtime
      container-manifest.json
      argocd-application.yaml
  with-keda/
    contract.json
    expected/
      ...
  with-secrets/
    ...
  full-dfe-loader/
    ...
```

Each producer (rustlib, pylib, hyperi-ci) runs:

```python
for fixture in fixtures:
    contract = json.load(fixture / "contract.json")
    output = produce_all_artefacts(contract)  # producer-specific
    expected = read_dir(fixture / "expected")
    assert output == expected, f"{producer} drifted on {fixture.name}"
```

CI for **each** of rustlib / pylib / hyperi-ci runs this. Any divergence
(byte-level) in Dockerfile/argocd/etc. fails CI. The fixtures live in
hyperi-rustlib (the source of truth for the contract); pylib and
hyperi-ci git-submodule or vendor them.

When the schema bumps, fixtures are re-generated from the new rustlib and
all three producers must update in lockstep.

---

## Migration path for existing apps

1. **rustlib 2.7.0** publishes (current work) — generators + cascade helpers
   ready, schema_version stays at 2.
2. **DFE apps** (rustlib consumers): implement `DfeApp::deployment_contract()`,
   run `<app> generate-artefacts --output-dir ci/`, commit. Drop the
   manually-curated `Dockerfile` from repo root (it now lives in `ci/`).
3. **hyperi-ci** rolls out:
   - `emit-artefacts` subcommand (Python templater, byte-parity with rustlib)
   - Quality stage drift check (auto-detected per tier)
   - Generate stage (post-Build, pre-Container)
   - Container stage reads `ci-tmp/Dockerfile.runtime`
   - Cross-language parity test suite
4. **pylib** rolls out (parallel to hyperi-ci):
   - `hyperi_pylib.deployment` module mirroring rustlib
   - `Application.deployment_contract()` hook
   - Cross-language parity tests
5. **Non-framework apps** (bash/TS/Go): commit `ci/deployment-contract.json`
   manually; their CI uses `hyperi-ci emit-artefacts ci/` for the rest.

Schema bumps after this are coordinated releases of rustlib + pylib + hyperi-ci.

---

## Reference

- rustlib source: [`src/deployment/`](../src/deployment/)
- rustlib generators: [`src/deployment/generate.rs`](../src/deployment/generate.rs)
- rustlib cascade helpers: [`src/deployment/registry.rs`](../src/deployment/registry.rs)
- DfeApp trait hook: [`src/cli/app.rs`](../src/cli/app.rs) — `fn deployment_contract(&self) -> Option<DeploymentContract>`

JSON Schema (auto-derived from the Rust struct via `schemars`) lives at
`deployment-contract.schema.json` once 2.8.0 ships.
