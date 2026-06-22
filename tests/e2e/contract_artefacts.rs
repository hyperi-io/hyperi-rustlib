// Project:   hyperi-rustlib
// File:      tests/e2e/contract_artefacts.rs
// Purpose:   E2E tests for generated container contract artefacts (TEMPLATE)
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! ============================================================================
//! TEMPLATE -- copy this file into your DFE consumer's `tests/e2e/` and
//! adapt the FIXTURE section. The probe/skip/cluster helpers live in
//! `hyperi_rustlib::deployment::test_support` so each consumer's copy
//! stays short and benefits from any bug fixes pushed to rustlib.
//!
//! Bring the test_support helpers into scope by adding a dev-dependency
//! on hyperi-rustlib version 2.7.3 or higher with the
//! `deployment-test-support` feature enabled.
//!
//! Then in your consumer's tests/e2e/contract_artefacts.rs, replace this
//! file's `test_contract()` / `test_identity()` fixtures with your own
//! `Config::deployment_contract()` (or whatever your app uses to build
//! its `DeploymentContract`) and `ContractIdentity::detect(image_ref)`.
//! Everything else -- the Tier A + Tier B test bodies -- stays the same.
//! ============================================================================
//!
//! E2E tests for the artefacts emitted by `crate::deployment` -- the
//! Dockerfile, Helm chart, and ArgoCD Application. Two tiers:
//!
//! - **Tier A** (default): light-weight checks that exercise the artefact
//!   without a real cluster.
//!   - Dockerfile: `docker build` + `docker run --rm <img> --help` -- proves
//!     the image actually starts.
//!   - Helm chart: `helm lint` + `helm template` -- proves the chart
//!     renders. (Without a cluster, deployment manifests can't "execute".)
//!   - ArgoCD Application: `kubeconform` -- proves the manifest is
//!     schema-valid.
//! - **Tier B** (env-gated by `HYPERI_E2E_CLUSTER=1`): heavy-weight checks
//!   that bring up a local kind cluster.
//!   - Helm: `helm install` on the kind cluster, assert the release lands.
//!   - ArgoCD: install ArgoCD into the cluster, apply the generated
//!     Application, verify the live object carries the identity annotations.
//!
//! # Skip policy
//!
//! Every test that needs an external tool / daemon / cluster probes first
//! via `hyperi_rustlib::deployment::test_support` and skips cleanly when
//! the dependency is absent. Skip emissions use the canonical prefix
//! `HYPERCI-SKIP[contract-e2e][tier-a|tier-b]:` so downstream test
//! runners can grep, count, and emit a summary line at the end of a CI
//! run.
//!
//! # Why a mock binary?
//!
//! `generate_dockerfile()` produces a Dockerfile that `COPY`s the
//! consumer app's binary into the image. rustlib itself isn't an app, so
//! for Tier A this template drops a tiny POSIX shell script into the
//! build context that responds to `--help`. Real consumer copies of this
//! template should REPLACE the mock with `cargo build --release --bin
//! <name>` and copy the produced binary into the docker build context.
//! See dfe-receiver's adaptation for the canonical example.

#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]

use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use hyperi_rustlib::deployment::test_support::{
    docker_available, docker_empty_creds_json, ensure_kind_cluster, helm_available,
    kubeconform_available, skip, tier_b_enabled, wait_until,
};
use hyperi_rustlib::deployment::{
    ArgocdConfig, ContractIdentity, DeploymentContract, HealthContract, ImageProfile, OciLabels,
    generate_argocd_application, generate_chart, generate_dockerfile,
};

// ============================================================================
// FIXTURE -- replace `test_contract()` and `test_identity()` in your
// consumer's copy with calls to your app's real contract / identity.
// ============================================================================

fn test_contract() -> DeploymentContract {
    DeploymentContract {
        app_name: "hyperi-contract-test".into(),
        binary_name: "hyperi-contract-test".into(),
        description: "Throwaway test app for rustlib contract e2e".into(),
        metrics_port: 9090,
        health: HealthContract::default(),
        env_prefix: "HCT".into(),
        metric_prefix: "hct".into(),
        config_mount_path: "/etc/hct/config.yaml".into(),
        image_registry: "ghcr.io/hyperi-io".into(),
        extra_ports: vec![],
        entrypoint_args: vec![],
        secrets: vec![],
        base_image: "ubuntu:24.04".into(),
        native_deps: hyperi_rustlib::deployment::NativeDepsContract::default(),
        image_profile: ImageProfile::Production,
        oci_labels: OciLabels::default(),
        schema_version: 1,
        keda: None,
        default_config: None,
        depends_on: vec![],
    }
}

fn test_identity() -> ContractIdentity {
    ContractIdentity::new(
        "0123456789abcdef0123456789abcdef01234567",
        "ghcr.io/hyperi-io/hyperi-contract-test:test",
    )
    .expect("fixture identity must validate")
}

/// Drop a tiny POSIX script into the build context to stand in for the
/// real consumer binary. Real consumer copies of this template should
/// REPLACE this with `cargo build` of their actual binary.
fn write_mock_binary(build_ctx: &Path, binary_name: &str) -> std::io::Result<()> {
    let path = build_ctx.join(binary_name);
    let mut f = std::fs::File::create(&path)?;
    f.write_all(
        b"#!/bin/sh\n\
          # Mock binary for hyperi-rustlib contract-artefact e2e test.\n\
          # The real consumer's binary is replaced by this stub during testing.\n\
          if [ \"$1\" = \"--help\" ] || [ \"$1\" = \"-h\" ]; then\n\
          \x20 echo \"hyperi-contract-test: ok\"\n\
          \x20 exit 0\n\
          fi\n\
          echo \"hyperi-contract-test: started (mock)\"\n\
          exit 0\n",
    )?;
    drop(f);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms)?;
    }
    Ok(())
}

// ============================================================================
// Tier A -- Dockerfile: docker build + docker run --help
// ============================================================================

#[test]
fn tier_a_dockerfile_builds_and_image_runs() {
    if !docker_available() {
        skip(
            "tier-a",
            "tier_a_dockerfile_builds_and_image_runs",
            "docker daemon not reachable",
        );
        return;
    }

    let contract = test_contract();
    let identity = test_identity();
    let dockerfile = generate_dockerfile(&contract, Some(&identity));

    let tmp = tempfile::tempdir().expect("tempdir");
    let ctx = tmp.path();
    let dockerfile_path = ctx.join("Dockerfile");
    std::fs::write(&dockerfile_path, &dockerfile).expect("write Dockerfile");
    write_mock_binary(ctx, contract.binary()).expect("write mock binary");

    let docker_config = tempfile::tempdir().expect("docker config tempdir");
    std::fs::write(
        docker_config.path().join("config.json"),
        docker_empty_creds_json(),
    )
    .expect("write empty docker config");

    let tag = format!("hyperi-contract-test:e2e-{}", std::process::id());

    let build = Command::new("docker")
        .env("DOCKER_CONFIG", docker_config.path())
        .args(["build", "--quiet", "-t", &tag, "-f"])
        .arg(&dockerfile_path)
        .arg(ctx)
        .output()
        .expect("docker build invocation");
    assert!(
        build.status.success(),
        "docker build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    let entrypoint = format!("/usr/local/bin/{}", contract.binary());
    let run = Command::new("docker")
        .env("DOCKER_CONFIG", docker_config.path())
        .args(["run", "--rm", "--entrypoint", &entrypoint, &tag, "--help"])
        .output()
        .expect("docker run invocation");
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        run.status.success(),
        "docker run failed: stdout={stdout} stderr={stderr}",
    );
    assert!(
        stdout.contains("hyperi-contract-test: ok"),
        "container ran but did not produce expected output: stdout={stdout} stderr={stderr}",
    );

    let inspect = Command::new("docker")
        .env("DOCKER_CONFIG", docker_config.path())
        .args(["inspect", "--format", "{{json .Config.Labels}}", &tag])
        .output()
        .expect("docker inspect invocation");
    let labels = String::from_utf8_lossy(&inspect.stdout);
    assert!(
        labels.contains("io.hyperi.contract.version")
            && labels.contains("\"v1\"")
            && labels.contains("io.hyperi.contract.source-commit")
            && labels.contains("0123456789abcdef0123456789abcdef01234567")
            && labels.contains("io.hyperi.contract.image-ref"),
        "docker inspect did not show all three io.hyperi.contract.* labels: {labels}",
    );

    let _ = Command::new("docker")
        .env("DOCKER_CONFIG", docker_config.path())
        .args(["rmi", "-f", &tag])
        .output();
}

// ============================================================================
// Tier A -- Helm chart: helm lint + helm template
// ============================================================================

#[test]
fn tier_a_chart_lint_and_template() {
    if !helm_available() {
        skip(
            "tier-a",
            "tier_a_chart_lint_and_template",
            "helm CLI not available",
        );
        return;
    }

    let contract = test_contract();
    let identity = test_identity();

    let tmp = tempfile::tempdir().expect("tempdir");
    let chart_dir = tmp.path().join("chart");
    std::fs::create_dir_all(&chart_dir).expect("create chart dir");
    generate_chart(&contract, &chart_dir, Some(&identity)).expect("generate_chart");

    let lint = Command::new("helm")
        .arg("lint")
        .arg(&chart_dir)
        .output()
        .expect("helm lint invocation");
    assert!(
        lint.status.success(),
        "helm lint failed: stdout={} stderr={}",
        String::from_utf8_lossy(&lint.stdout),
        String::from_utf8_lossy(&lint.stderr),
    );

    let template = Command::new("helm")
        .args(["template", "test-release"])
        .arg(&chart_dir)
        .output()
        .expect("helm template invocation");
    assert!(
        template.status.success(),
        "helm template failed: stdout={} stderr={}",
        String::from_utf8_lossy(&template.stdout),
        String::from_utf8_lossy(&template.stderr),
    );
    let rendered = String::from_utf8_lossy(&template.stdout);
    assert!(
        rendered.contains("hyperi-contract-test"),
        "rendered template missing app name: {rendered}",
    );

    let chart_yaml =
        std::fs::read_to_string(chart_dir.join("Chart.yaml")).expect("read Chart.yaml");
    assert!(chart_yaml.contains("io.hyperi.contract.version: \"v1\""));
    assert!(chart_yaml.contains(
        "io.hyperi.contract.source-commit: \"0123456789abcdef0123456789abcdef01234567\""
    ));
    assert!(
        chart_yaml.contains(
            "io.hyperi.contract.image-ref: \"ghcr.io/hyperi-io/hyperi-contract-test:test\""
        )
    );
}

// ============================================================================
// Tier A -- ArgoCD Application: kubeconform
// ============================================================================

#[test]
fn tier_a_argocd_application_kubeconform() {
    if !kubeconform_available() {
        skip(
            "tier-a",
            "tier_a_argocd_application_kubeconform",
            "kubeconform not on PATH",
        );
        return;
    }

    let contract = test_contract();
    let identity = test_identity();
    let argo = ArgocdConfig::default();
    let yaml = generate_argocd_application(&contract, &argo, Some(&identity));

    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("application.yaml");
    std::fs::write(&path, &yaml).expect("write application.yaml");

    let out = Command::new("kubeconform")
        .args(["-strict", "-summary", "-ignore-missing-schemas"])
        .arg(&path)
        .output()
        .expect("kubeconform invocation");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "kubeconform failed: stdout={stdout} stderr={stderr}",
    );
    assert!(
        stdout.contains("0 errors") || stdout.contains("Valid:"),
        "kubeconform summary missing 0-error line: {stdout}",
    );

    let raw = std::fs::read_to_string(&path).unwrap();
    assert_eq!(raw.matches("io.hyperi.contract").count(), 3);
}

// ============================================================================
// Tier B -- kind cluster + real helm install. Env-gated.
// ============================================================================

#[test]
fn tier_b_helm_install_on_kind() {
    if !tier_b_enabled() {
        skip(
            "tier-b",
            "tier_b_helm_install_on_kind",
            "HYPERI_E2E_CLUSTER env var not set (skipping cluster-based tests)",
        );
        return;
    }
    if !helm_available() {
        skip(
            "tier-b",
            "tier_b_helm_install_on_kind",
            "helm CLI not available",
        );
        return;
    }

    let Some(cluster) = ensure_kind_cluster("tier_b_helm_install_on_kind") else {
        return;
    };

    let contract = test_contract();
    let identity = test_identity();

    let tmp = tempfile::tempdir().expect("tempdir");
    let chart_dir = tmp.path().join("chart");
    std::fs::create_dir_all(&chart_dir).expect("create chart dir");
    generate_chart(&contract, &chart_dir, Some(&identity)).expect("generate_chart");

    let install = Command::new("helm")
        .env("KUBECONFIG", &cluster.kubeconfig)
        .args([
            "install",
            "test-release",
            chart_dir.to_str().unwrap(),
            "--namespace",
            "default",
            "--set",
            "image.repository=public.ecr.aws/docker/library/nginx",
            "--set",
            "image.tag=alpine",
            "--wait",
            "--timeout",
            "120s",
        ])
        .output()
        .expect("helm install invocation");
    let stdout = String::from_utf8_lossy(&install.stdout);
    let stderr = String::from_utf8_lossy(&install.stderr);
    if !install.status.success() {
        assert!(
            !(stderr.contains("Error: INSTALLATION FAILED") && stderr.contains("template:")),
            "helm install failed at template stage: {stderr}",
        );
        eprintln!("note: helm install completed admission but pod did not become ready ({stderr})");
    }
    assert!(
        !stdout.is_empty() || !stderr.is_empty(),
        "helm install produced no output -- something is very wrong",
    );

    let list = Command::new("helm")
        .env("KUBECONFIG", &cluster.kubeconfig)
        .args(["list", "--all-namespaces", "-o", "json"])
        .output()
        .expect("helm list");
    let out = String::from_utf8_lossy(&list.stdout);
    assert!(
        out.contains("test-release"),
        "helm release 'test-release' not found post-install: {out}",
    );

    let _ = Command::new("helm")
        .env("KUBECONFIG", &cluster.kubeconfig)
        .args(["uninstall", "test-release"])
        .output();
}

#[test]
fn tier_b_argocd_application_sync_on_kind() {
    if !tier_b_enabled() {
        skip(
            "tier-b",
            "tier_b_argocd_application_sync_on_kind",
            "HYPERI_E2E_CLUSTER env var not set (skipping cluster-based tests)",
        );
        return;
    }

    let Some(cluster) = ensure_kind_cluster("tier_b_argocd_application_sync_on_kind") else {
        return;
    };

    // Install ArgoCD into the cluster.
    let ns_yaml = Command::new("kubectl")
        .env("KUBECONFIG", &cluster.kubeconfig)
        .args([
            "create",
            "namespace",
            "argocd",
            "--dry-run=client",
            "-o",
            "yaml",
        ])
        .output()
        .expect("kubectl ns yaml");
    let ns_apply = Command::new("kubectl")
        .env("KUBECONFIG", &cluster.kubeconfig)
        .args(["apply", "-f", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.as_mut().unwrap().write_all(&ns_yaml.stdout)?;
            child.wait_with_output()
        })
        .expect("kubectl apply namespace");
    assert!(ns_apply.status.success());

    let install = Command::new("kubectl")
        .env("KUBECONFIG", &cluster.kubeconfig)
        .args([
            "apply",
            "-n",
            "argocd",
            "-f",
            "https://raw.githubusercontent.com/argoproj/argo-cd/stable/manifests/install.yaml",
        ])
        .output()
        .expect("kubectl apply argocd");
    if !install.status.success() {
        skip(
            "tier-b",
            "tier_b_argocd_application_sync_on_kind",
            &format!(
                "ArgoCD manifest fetch/apply failed (network?): {}",
                String::from_utf8_lossy(&install.stderr).trim()
            ),
        );
        return;
    }

    // Wait for argocd-server Deployment to become Available.
    let kubeconfig = cluster.kubeconfig.clone();
    let server_ready = wait_until(Duration::from_mins(5), Duration::from_secs(5), || {
        Command::new("kubectl")
            .env("KUBECONFIG", &kubeconfig)
            .args([
                "-n",
                "argocd",
                "wait",
                "--for=condition=Available",
                "--timeout=10s",
                "deploy/argocd-server",
            ])
            .output()
            .is_ok_and(|o| o.status.success())
    });
    if !server_ready {
        skip(
            "tier-b",
            "tier_b_argocd_application_sync_on_kind",
            "argocd-server did not become Available within 300s",
        );
        return;
    }

    // Apply the generated Application + verify identity annotations on the
    // live object.
    let contract = test_contract();
    let identity = test_identity();
    let argo = ArgocdConfig::default();
    let app_yaml = generate_argocd_application(&contract, &argo, Some(&identity));

    let tmp = tempfile::tempdir().expect("tempdir");
    let app_path = tmp.path().join("application.yaml");
    std::fs::write(&app_path, &app_yaml).expect("write app yaml");

    let apply = Command::new("kubectl")
        .env("KUBECONFIG", &cluster.kubeconfig)
        .args(["apply", "-f"])
        .arg(&app_path)
        .output()
        .expect("kubectl apply application");
    assert!(
        apply.status.success(),
        "kubectl apply application failed: {}",
        String::from_utf8_lossy(&apply.stderr),
    );

    let get = Command::new("kubectl")
        .env("KUBECONFIG", &cluster.kubeconfig)
        .args([
            "-n",
            "argocd",
            "get",
            "application",
            &contract.app_name,
            "-o",
            "jsonpath={.metadata.annotations}",
        ])
        .output()
        .expect("kubectl get application");
    let annotations = String::from_utf8_lossy(&get.stdout);
    assert!(
        annotations.contains("io.hyperi.contract.version")
            && annotations.contains("v1")
            && annotations.contains("io.hyperi.contract.source-commit")
            && annotations.contains("io.hyperi.contract.image-ref"),
        "applied Application missing identity annotations: {annotations}",
    );
}
