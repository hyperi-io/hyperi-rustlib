// Project:   hyperi-rustlib
// File:      src/deployment/test_support.rs
// Purpose:   Reusable test helpers for contract-artefact e2e tests
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Test helpers for contract-artefact end-to-end tests, shared by rustlib
//! itself and every downstream DFE consumer (`dfe-loader`, `dfe-receiver`,
//! `dfe-archiver`, `dfe-fetcher`, `dfe-transform-vrl`, `dfe-transform-vector`).
//!
//! # What this module provides
//!
//! * **Tool probes** -- thin wrappers over `Command::new(<tool>).output()`
//!   that cache result via `OnceLock` so a slow probe runs at most once
//!   per test binary. Probes: [`docker_available`], [`helm_available`],
//!   [`kubeconform_available`], [`kind_available`], [`kubectl_available`].
//! * **Skip emission** -- [`skip`] writes the canonical
//!   `HYPERCI-SKIP[contract-e2e][<tier>]: <test>: <reason>` line to BOTH
//!   stderr and a side-channel file at
//!   `$CARGO_TARGET_TMPDIR/contract-e2e-skips.log` (or
//!   `~/.cache/hyperi-ai/contract-e2e-skips.log` outside a cargo run).
//!   The CI test runner is expected to grep + count these lines and
//!   emit a top-of-stage summary; tests themselves don't need to do
//!   anything fancier than call this helper and `return`.
//! * **Tier-B gate** -- [`tier_b_enabled`] returns true iff
//!   `HYPERI_E2E_CLUSTER=1`. Cluster-based tests gate on this.
//! * **Kind cluster lifecycle** -- [`KindClusterGuard`] +
//!   [`ensure_kind_cluster`] bring up a uniquely-named local kind
//!   cluster and tear it down on Drop. Each test gets its own cluster
//!   so parallel test runs never collide on cluster name.
//!
//! # Usage from a consumer's `tests/e2e/`
//!
//! Add `hyperi-rustlib` with the `deployment` feature in your
//! dev-dependencies (this module always ships with `deployment`):
//!
//! ```ignore
//! use hyperi_rustlib::deployment::test_support::{
//!     docker_available, ensure_kind_cluster, skip, tier_b_enabled,
//! };
//!
//! #[test]
//! fn my_consumer_dockerfile_actually_starts() {
//!     if !docker_available() {
//!         skip(
//!             "tier-a",
//!             "my_consumer_dockerfile_actually_starts",
//!             "docker daemon not reachable",
//!         );
//!         return;
//!     }
//!     // ... build + run image, etc.
//! }
//! ```
//!
//! # Why no `tempfile` dependency here?
//!
//! rustlib `src/` stays std-only -- `tempfile` is a dev-dep, available
//! to integration tests but not pulled into the runtime crate. Callers
//! that need a tempdir for kubeconfig storage create their own and pass
//! the path in via [`ensure_kind_cluster_in`]. The shorthand
//! [`ensure_kind_cluster`] uses `~/.cache/hyperi-ai/contract-test/<cluster>/`
//! and cleans the directory on Drop -- works without any dev-dep.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

// ============================================================================
// Tool probes -- thin, cached, no panic.
// ============================================================================

/// Returns true iff `docker info` succeeds. Cached for the test-binary
/// lifetime.
#[must_use]
pub fn docker_available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        Command::new("docker")
            .args(["info", "--format", "{{.ServerVersion}}"])
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// Returns true iff `helm version` succeeds.
#[must_use]
pub fn helm_available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        Command::new("helm")
            .arg("version")
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// Returns true iff `kubeconform -v` succeeds.
#[must_use]
pub fn kubeconform_available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        Command::new("kubeconform")
            .arg("-v")
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// Returns true iff `kind version` succeeds.
#[must_use]
pub fn kind_available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        Command::new("kind")
            .arg("version")
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// Returns true iff `kubectl version --client=true` succeeds.
#[must_use]
pub fn kubectl_available() -> bool {
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| {
        Command::new("kubectl")
            .args(["version", "--client=true"])
            .output()
            .is_ok_and(|o| o.status.success())
    })
}

/// Returns true iff `HYPERI_E2E_CLUSTER` env var is set to `1` or `true`.
///
/// Cluster-based (Tier B) tests must check this before bringing up kind
/// because cluster spin-up is slow (60-120 s typical) and the harness
/// shouldn't run by default in `cargo test`.
#[must_use]
pub fn tier_b_enabled() -> bool {
    std::env::var("HYPERI_E2E_CLUSTER").is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

// ============================================================================
// Skip emission with canonical prefix.
// ============================================================================

/// Write a skip notice with the canonical
/// `HYPERCI-SKIP[contract-e2e][<tier>]:` prefix to stderr AND to a
/// side-channel log file the test runner can grep after the test stage.
///
/// `tier` is `"tier-a"` or `"tier-b"`. Anything else works but the test
/// runner's grep won't match.
///
/// # Side-channel path
///
/// Tries `$CARGO_TARGET_TMPDIR/contract-e2e-skips.log` first (cargo's
/// official integration-test scratch dir). Falls back to
/// `$HOME/.cache/hyperi-ai/contract-e2e-skips.log` if the env var is
/// unset (e.g. when the test binary is invoked outside `cargo test`).
/// File-write failures are silently ignored -- the test must not fail
/// because the skip-log infrastructure had a hiccup.
pub fn skip(tier: &str, test_name: &str, reason: &str) {
    let line = format!("HYPERCI-SKIP[contract-e2e][{tier}]: {test_name}: {reason}");
    eprintln!("{line}");
    if let Some(path) = skip_log_path() {
        let _ = append_line(&path, &line);
    }
}

fn skip_log_path() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("CARGO_TARGET_TMPDIR") {
        return Some(Path::new(&dir).join("contract-e2e-skips.log"));
    }
    // Fallback: ~/.cache/hyperi-ai/. Never /tmp per AGENT-RULES Rule 4.
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    Some(Path::new(&home).join(".cache/hyperi-ai/contract-e2e-skips.log"))
}

fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{line}")
}

// ============================================================================
// Docker config helper -- empty creds JSON to avoid credential-helper lookup.
// ============================================================================

/// Empty-creds JSON suitable for `<docker_config_dir>/config.json`.
///
/// Set `DOCKER_CONFIG=<dir>` and write this to `<dir>/config.json` to
/// prevent docker from invoking credential helpers like
/// `docker-credential-secretservice` that may not be on the host. Public
/// registry pulls (Docker Hub, ghcr.io public images) don't need auth, so
/// the empty config is sufficient for build + run tests.
#[must_use]
pub fn docker_empty_creds_json() -> &'static str {
    r#"{"auths": {}}"#
}

// ============================================================================
// Kind cluster lifecycle -- per-test cluster, dropped on test exit.
// ============================================================================

/// Owns a kind cluster + its kubeconfig path. Dropping the guard deletes
/// the cluster and (if owned) removes the kubeconfig parent directory.
#[derive(Debug)]
pub struct KindClusterGuard {
    /// Kind cluster name (e.g. `hctest-deadbeef`).
    pub name: String,
    /// Path to the kubeconfig file. Tests pass this via `KUBECONFIG` env
    /// var so they don't disturb the user's default context.
    pub kubeconfig: PathBuf,
    /// Directory that holds `kubeconfig`. When the guard owns it (`true`),
    /// the directory is removed on Drop. When `false`, the caller manages
    /// the lifetime (e.g. via their own `tempfile::TempDir`).
    cleanup_dir: bool,
}

impl Drop for KindClusterGuard {
    fn drop(&mut self) {
        // Best-effort: delete the cluster.
        let _ = Command::new("kind")
            .args(["delete", "cluster", "--name", &self.name])
            .output();
        // Best-effort: remove the kubeconfig directory if we own it.
        if self.cleanup_dir
            && let Some(parent) = self.kubeconfig.parent()
        {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}

/// Bring up a uniquely-named kind cluster for the caller, storing
/// `kubeconfig` under `kubeconfig_dir` (which the caller manages, e.g.
/// via their own `tempfile::TempDir`).
///
/// Returns `None` if any prerequisite is missing (kind/kubectl/docker)
/// or cluster creation fails -- the underlying reason is emitted via
/// [`skip`] so the test runner sees it.
///
/// The cluster name is derived from `test_name` via a small hash so each
/// test in a parallel suite gets its own cluster.
#[must_use]
pub fn ensure_kind_cluster_in(test_name: &str, kubeconfig_dir: &Path) -> Option<KindClusterGuard> {
    let cluster = prepare_cluster_or_skip(test_name)?;
    let kubeconfig = kubeconfig_dir.join("kubeconfig");
    let kc = Command::new("kind")
        .args(["get", "kubeconfig", "--name", &cluster])
        .output()
        .ok()?;
    std::fs::write(&kubeconfig, &kc.stdout).ok()?;
    Some(KindClusterGuard {
        name: cluster,
        kubeconfig,
        cleanup_dir: false,
    })
}

/// Convenience: like [`ensure_kind_cluster_in`] but uses an owned
/// directory under `~/.cache/hyperi-ai/contract-test/<cluster>/` so the
/// caller doesn't need `tempfile`. The directory is removed on Drop.
#[must_use]
pub fn ensure_kind_cluster(test_name: &str) -> Option<KindClusterGuard> {
    let cluster = prepare_cluster_or_skip(test_name)?;
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    let dir = Path::new(&home)
        .join(".cache/hyperi-ai/contract-test")
        .join(&cluster);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        skip(
            "tier-b",
            test_name,
            &format!(
                "could not create cluster scratch dir {}: {e}",
                dir.display()
            ),
        );
        // Best-effort cleanup of the cluster we just created.
        let _ = Command::new("kind")
            .args(["delete", "cluster", "--name", &cluster])
            .output();
        return None;
    }
    let kubeconfig = dir.join("kubeconfig");
    let kc = Command::new("kind")
        .args(["get", "kubeconfig", "--name", &cluster])
        .output()
        .ok()?;
    std::fs::write(&kubeconfig, &kc.stdout).ok()?;
    Some(KindClusterGuard {
        name: cluster,
        kubeconfig,
        cleanup_dir: true,
    })
}

fn prepare_cluster_or_skip(test_name: &str) -> Option<String> {
    if !kind_available() {
        skip(
            "tier-b",
            test_name,
            "kind CLI not on PATH (install: https://kind.sigs.k8s.io/)",
        );
        return None;
    }
    if !kubectl_available() {
        skip("tier-b", test_name, "kubectl not on PATH");
        return None;
    }
    if !docker_available() {
        skip(
            "tier-b",
            test_name,
            "docker daemon not reachable (kind requires docker)",
        );
        return None;
    }

    let suffix = test_name.bytes().fold(0u32, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(u32::from(b))
    });
    let cluster = format!("hctest-{suffix:08x}");

    let create = Command::new("kind")
        .args(["create", "cluster", "--name", &cluster, "--wait", "120s"])
        .output()
        .ok()?;
    if !create.status.success() {
        skip(
            "tier-b",
            test_name,
            &format!(
                "kind create cluster failed: {}",
                String::from_utf8_lossy(&create.stderr).trim()
            ),
        );
        return None;
    }
    Some(cluster)
}

// ============================================================================
// Wait helpers -- usable from cluster tests for "wait until X happens".
// ============================================================================

/// Poll `f` every `interval` until it returns `true` or `deadline` elapses.
/// Returns `true` if `f` returned `true` before deadline, `false` otherwise.
///
/// Used by cluster tests to wait for pods/deployments to become Ready.
pub fn wait_until(deadline: Duration, interval: Duration, mut f: impl FnMut() -> bool) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        if f() {
            return true;
        }
        std::thread::sleep(interval);
    }
    f()
}
