# Dependency duplication report

> At-scale hardening Task 4.3 (finding P2). Snapshot 2026-06-03.
> Regenerate with `scripts/dep-dup-check.sh` (warning-only).

`cargo tree -d --features full -e normal` reports **56 duplicated crate
versions**. Every one is **transitive** -- hyperi-rustlib's *direct*
dependencies pin a single version each. Duplicates fall into three buckets.

## 1. Actionable (transitive, upgrade path exists)

| Duplicate | Pulled by | Action |
|-----------|-----------|--------|
| `reqwest 0.12` (alongside our `0.13`) | `opentelemetry-otlp 0.31` -> `opentelemetry-http 0.31` | Bump OTLP to 0.32 when it lands on reqwest 0.13 (we pin `>=0.31, <0.32`). Until then unavoidable -- OTLP owns the old reqwest. |
| `sysinfo 0.28` (alongside our `0.39`) | `yaque 0.6.6` (spool/disk-queue) | yaque pins old sysinfo. Track yaque updates, or revisit the spool backend. Low impact (compiled once, not on the hot path). |

## 2. Ecosystem transitional duplicates (unavoidable, no action)

These are mid-migration splits across the whole Rust ecosystem; pinning is not
in our control and forcing a single version is infeasible:

- `thiserror 1 / 2`, `thiserror-impl 1 / 2`
- `syn 1 / 2`, `synstructure 0.12 / 0.13`
- `http 0.2 / 1.0`, `http-body 0.4 / 1.0`
- `mio 0.8 / 1.0`
- `rand 0.8 / 0.9 / 0.10`, `rand_chacha`, `rand_core 0.6 / 0.9 / 0.10`
- `hashbrown 0.14 / 0.16 / 0.17`, `bitflags 1 / 2`
- `getrandom 0.2 / 0.3 / 0.4`
- `toml_datetime`, `toml_edit`, `winnow 0.7 / 1.0`
- crypto family mid-bump: `digest 0.10 / 0.11`, `block-buffer`, `crypto-common`,
  `hmac 0.12 / 0.13`, `sha2 0.10 / 0.11`, `cpufeatures`
- `openssl-probe 0.1 / 0.2`

## 3. Notes / watch items

- **Yanked `metrics 0.24.5`:** RESOLVED -- `cargo update -p metrics` lands on
  `0.24.6` (off the yank). Cargo.lock is gitignored for this library, so fresh
  CI/consumer resolution selects 0.24.6 automatically (cargo never picks a
  yanked version unless pinned in a committed lock). No action needed.
- **Minimal feature builds:** the AWS, Vault, TUI, and git stacks are behind
  `secrets-aws` / `secrets-vault` / `cli-service` / `directory-config-git` and
  must NOT appear in a default or transport-only build. Spot-check with
  `cargo tree --no-default-features --features transport-kafka`.

## CI

`scripts/dep-dup-check.sh` runs `cargo tree -d` and prints the count
**warning-only** (never fails the build) -- duplication is tracked, not gated,
because most entries are ecosystem-transitional and outside our control.
