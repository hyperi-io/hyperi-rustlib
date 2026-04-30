// Project:   hyperi-rustlib
// File:      src/deployment/smoke.rs
// Purpose:   Smoke-test helper for generated Dockerfiles
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Smoke-test helper for [`generate_dockerfile`](super::generate_dockerfile)
//! output.
//!
//! Builds a docker image from the contract-generated Dockerfile + a binary,
//! then runs the container with a probe command (`version` by default) and
//! asserts the container exits cleanly. Catches:
//!
//! - Bad APT package names (`docker build` fails)
//! - Binary linking issues against the runtime base image
//! - Wrong entrypoint args
//! - Healthcheck command broken at startup
//!
//! # Requirements
//!
//! - Docker daemon reachable (`docker info` succeeds)
//! - Local binary already built (`cargo build` ran)
//!
//! # Example
//!
//! ```rust,ignore
//! #[test]
//! fn smoke_test_dockerfile_builds_and_runs() {
//!     // Skip if no docker on CI runner / dev machine
//!     if !hyperi_rustlib::deployment::smoke::docker_available() {
//!         return;
//!     }
//!     let contract = my_app::deployment::contract();
//!     let r = hyperi_rustlib::deployment::smoke::smoke_test_build(
//!         &contract,
//!         std::path::Path::new("target/debug/my-app"),
//!         &["version"],
//!     ).unwrap();
//!     assert_eq!(r.exit_code, 0, "container exited non-zero: {}", r.stdout);
//! }
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

use super::contract::DeploymentContract;
use super::generate::generate_dockerfile;

/// Result of a smoke-test build + run.
#[derive(Debug, Clone)]
pub struct SmokeResult {
    /// Tag of the built image.
    pub image_tag: String,
    /// stdout captured from `docker run`.
    pub stdout: String,
    /// stderr captured from `docker run`.
    pub stderr: String,
    /// Exit code of the container.
    pub exit_code: i32,
    /// Path to the tempdir used for the build context.
    pub build_context: PathBuf,
}

/// Errors during smoke-testing.
#[derive(Debug, thiserror::Error)]
pub enum SmokeError {
    /// Docker daemon not reachable.
    #[error("docker daemon not available — `docker info` failed: {0}")]
    DockerUnavailable(String),

    /// Source binary missing.
    #[error("source binary not found at {path}")]
    BinaryMissing {
        /// The path that was checked.
        path: PathBuf,
    },

    /// I/O error during context setup.
    #[error("I/O error during smoke test: {0}")]
    Io(#[from] std::io::Error),

    /// `docker build` failed (image didn't build).
    #[error("docker build failed (exit {exit_code}): {stderr}")]
    BuildFailed {
        /// Build exit code.
        exit_code: i32,
        /// Build stderr.
        stderr: String,
    },
}

/// Returns true if `docker info` succeeds (daemon reachable).
///
/// Use this in test prologues to skip when Docker isn't available
/// (developer machines without Docker, CI without docker-in-docker, etc.).
#[must_use]
pub fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Build + run a smoke-test container from the contract.
///
/// 1. Creates a tempdir as the docker build context.
/// 2. Copies `binary_path` into the context as `<contract.binary()>`.
/// 3. Writes the contract's [`generate_dockerfile`] output to `Dockerfile`.
/// 4. Runs `docker build -t <tag> <context>`.
/// 5. Runs `docker run --rm <tag> <run_args>` (or just `<tag>` if empty).
/// 6. Captures stdout / stderr / exit code.
/// 7. Cleans up the image.
///
/// The tempdir is left in place if the build fails (for debugging) and
/// cleaned up otherwise. The path is returned in [`SmokeResult::build_context`].
///
/// # Errors
///
/// - [`SmokeError::DockerUnavailable`] if the daemon isn't reachable
/// - [`SmokeError::BinaryMissing`] if `binary_path` doesn't exist
/// - [`SmokeError::BuildFailed`] if `docker build` fails
/// - [`SmokeError::Io`] for filesystem errors during context setup
pub fn smoke_test_build(
    contract: &DeploymentContract,
    binary_path: &Path,
    run_args: &[&str],
) -> Result<SmokeResult, SmokeError> {
    if !docker_available() {
        return Err(SmokeError::DockerUnavailable(
            "`docker info` did not exit 0".into(),
        ));
    }
    if !binary_path.exists() {
        return Err(SmokeError::BinaryMissing {
            path: binary_path.to_path_buf(),
        });
    }

    // Build context tempdir.
    let context = tempdir_for_smoke(contract)?;

    // Copy binary into context with the name the Dockerfile expects.
    let dst = context.join(contract.binary());
    std::fs::copy(binary_path, &dst)?;
    set_executable(&dst)?;

    // Write Dockerfile.
    let dockerfile = generate_dockerfile(contract);
    std::fs::write(context.join("Dockerfile"), dockerfile)?;

    // Build.
    let image_tag = format!("{}:smoke-test", contract.app_name);
    let build = Command::new("docker")
        .arg("build")
        .arg("-t")
        .arg(&image_tag)
        .arg(&context)
        .output()?;

    if !build.status.success() {
        return Err(SmokeError::BuildFailed {
            exit_code: build.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&build.stderr).into_owned(),
        });
    }

    // Run.
    let mut run = Command::new("docker");
    run.arg("run").arg("--rm").arg(&image_tag);
    for arg in run_args {
        run.arg(arg);
    }
    let output = run.output()?;

    // Cleanup image.
    let _ = Command::new("docker")
        .arg("image")
        .arg("rm")
        .arg("-f")
        .arg(&image_tag)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    Ok(SmokeResult {
        image_tag,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code().unwrap_or(-1),
        build_context: context,
    })
}

fn tempdir_for_smoke(contract: &DeploymentContract) -> std::io::Result<PathBuf> {
    let base = std::env::temp_dir().join(format!("hyperi-smoke-{}", contract.app_name));
    if base.exists() {
        std::fs::remove_dir_all(&base)?;
    }
    std::fs::create_dir_all(&base)?;
    Ok(base)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_available_returns_bool() {
        // Just exercises the function; result depends on environment.
        let _ = docker_available();
    }

    #[test]
    fn binary_missing_error() {
        let contract = make_test_contract();
        let r = smoke_test_build(
            &contract,
            Path::new("/nonexistent/binary/that/does/not/exist"),
            &[],
        );
        // Either DockerUnavailable (no daemon) or BinaryMissing — both are
        // expected non-success outcomes. We just assert it errored.
        assert!(r.is_err());
    }

    fn make_test_contract() -> DeploymentContract {
        DeploymentContract {
            schema_version: 2,
            app_name: "smoke-test".into(),
            binary_name: "smoke-test".into(),
            description: "Smoke test".into(),
            metrics_port: 9090,
            health: super::super::HealthContract::default(),
            env_prefix: "SMOKE".into(),
            metric_prefix: "smoke".into(),
            config_mount_path: "/etc/smoke/config.yaml".into(),
            image_registry: super::super::DEFAULT_IMAGE_REGISTRY.to_string(),
            extra_ports: vec![],
            entrypoint_args: vec![],
            secrets: vec![],
            default_config: None,
            depends_on: vec![],
            keda: None,
            base_image: super::super::DEFAULT_BASE_IMAGE.to_string(),
            native_deps: super::super::NativeDepsContract::default(),
            image_profile: super::super::ImageProfile::Production,
            oci_labels: super::super::OciLabels::default(),
        }
    }
}
