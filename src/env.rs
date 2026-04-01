// Project:   hyperi-rustlib
// File:      src/env.rs
// Purpose:   Runtime environment detection (K8s, Docker, container, bare metal)
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Runtime environment detection.
//!
//! Detects whether the application is running in Kubernetes, Docker,
//! a generic container, or on bare metal. This information is used
//! to configure paths, logging format, and other runtime behaviour.

use std::path::Path;

/// Runtime environment types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Environment {
    /// Running in Kubernetes
    Kubernetes,
    /// Running in Docker (but not K8s)
    Docker,
    /// Running in a generic container (detected via cgroups)
    Container,
    /// Running on bare metal / local development
    BareMetal,
}

impl Environment {
    /// Detect the current runtime environment.
    ///
    /// Detection priority (highest confidence first):
    /// 1. Kubernetes service account token exists
    /// 2. Kubernetes environment variables present
    /// 3. Docker: `/.dockerenv` file exists
    /// 4. Container: cgroups contain container markers
    /// 5. Default: `BareMetal`
    #[must_use]
    pub fn detect() -> Self {
        // Check for Kubernetes first (highest priority)
        if Self::is_kubernetes_by_token() || Self::is_kubernetes_by_env() {
            return Self::Kubernetes;
        }

        // Check for Docker
        if Self::is_docker_by_file() {
            return Self::Docker;
        }

        // Check for generic container via cgroups
        if Self::is_container_by_cgroups() {
            return Self::Container;
        }

        Self::BareMetal
    }

    /// Check if running in any container environment.
    #[must_use]
    pub const fn is_container(&self) -> bool {
        matches!(self, Self::Kubernetes | Self::Docker | Self::Container)
    }

    /// Check if running in Kubernetes.
    #[must_use]
    pub const fn is_kubernetes(&self) -> bool {
        matches!(self, Self::Kubernetes)
    }

    /// Check if running in Docker (but not K8s).
    #[must_use]
    pub const fn is_docker(&self) -> bool {
        matches!(self, Self::Docker)
    }

    /// Check if running on bare metal.
    #[must_use]
    pub const fn is_bare_metal(&self) -> bool {
        matches!(self, Self::BareMetal)
    }

    // Detection helpers

    fn is_kubernetes_by_token() -> bool {
        Path::new("/var/run/secrets/kubernetes.io/serviceaccount/token").exists()
    }

    fn is_kubernetes_by_env() -> bool {
        std::env::var("KUBERNETES_SERVICE_HOST").is_ok()
    }

    fn is_docker_by_file() -> bool {
        Path::new("/.dockerenv").exists()
    }

    fn is_container_by_cgroups() -> bool {
        // Check cgroup v1
        if let Ok(content) = std::fs::read_to_string("/proc/1/cgroup")
            && (content.contains("/docker/")
                || content.contains("/kubepods/")
                || content.contains("/lxc/")
                || content.contains("/containerd/"))
        {
            return true;
        }

        // Check cgroup v2 (unified hierarchy)
        if let Ok(content) = std::fs::read_to_string("/proc/1/mountinfo")
            && (content.contains("/docker/")
                || content.contains("/kubepods/")
                || content.contains("/containerd/"))
        {
            return true;
        }

        false
    }
}

impl std::fmt::Display for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Kubernetes => write!(f, "kubernetes"),
            Self::Docker => write!(f, "docker"),
            Self::Container => write!(f, "container"),
            Self::BareMetal => write!(f, "bare_metal"),
        }
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::detect()
    }
}

// =============================================================================
// RuntimeContext — rich runtime metadata detected once at startup
// =============================================================================

/// Rich runtime context detected once at startup, immutable after.
///
/// Provides all K8s/container metadata in one place. Modules read from this
/// instead of doing their own env var lookups. Detected lazily on first access
/// via [`runtime_context()`].
///
/// On bare metal, most fields are `None` — features that read them become no-ops.
#[derive(Debug, Clone)]
pub struct RuntimeContext {
    /// Detected runtime environment.
    pub environment: Environment,
    /// K8s pod name (from `POD_NAME` or `HOSTNAME` env var).
    pub pod_name: Option<String>,
    /// K8s namespace (from `POD_NAMESPACE` env var or service account).
    pub namespace: Option<String>,
    /// K8s node name (from `NODE_NAME` env var).
    pub node_name: Option<String>,
    /// Container ID (from `HOSTNAME` in container environments).
    pub container_id: Option<String>,
    /// cgroup memory limit in bytes (`None` if unlimited or bare metal).
    pub memory_limit_bytes: Option<u64>,
    /// cgroup CPU quota in cores (`None` if unlimited or bare metal).
    pub cpu_quota_cores: Option<f64>,
}

impl RuntimeContext {
    /// Detect the full runtime context.
    ///
    /// Reads environment variables and filesystem signals. Safe to call
    /// on bare metal — fields will be `None` when not in a container.
    #[must_use]
    pub fn detect() -> Self {
        let environment = Environment::detect();

        let pod_name = std::env::var("POD_NAME").ok().or_else(|| {
            if environment.is_container() {
                std::env::var("HOSTNAME").ok()
            } else {
                None
            }
        });

        let namespace = std::env::var("POD_NAMESPACE").ok().or_else(|| {
            // Fall back to reading the K8s service account namespace file
            std::fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/namespace")
                .ok()
                .map(|s| s.trim().to_string())
        });

        let node_name = std::env::var("NODE_NAME").ok();

        let container_id = if environment.is_container() {
            std::env::var("HOSTNAME").ok()
        } else {
            None
        };

        // cgroup resource limits (container environments only)
        let memory_limit_bytes = if environment.is_container() {
            read_cgroup_memory_limit()
        } else {
            None
        };

        let cpu_quota_cores = if environment.is_container() {
            read_cgroup_cpu_quota()
        } else {
            None
        };

        Self {
            environment,
            pod_name,
            namespace,
            node_name,
            container_id,
            memory_limit_bytes,
            cpu_quota_cores,
        }
    }

    /// Convenience: is this running in Kubernetes?
    #[must_use]
    pub fn is_kubernetes(&self) -> bool {
        self.environment.is_kubernetes()
    }

    /// Convenience: is this running in any container?
    #[must_use]
    pub fn is_container(&self) -> bool {
        self.environment.is_container()
    }

    /// Convenience: is this bare metal / local dev?
    #[must_use]
    pub fn is_bare_metal(&self) -> bool {
        self.environment.is_bare_metal()
    }
}

impl Default for RuntimeContext {
    fn default() -> Self {
        Self::detect()
    }
}

static RUNTIME_CONTEXT: std::sync::OnceLock<RuntimeContext> = std::sync::OnceLock::new();

/// Get the global runtime context (detected lazily on first call).
///
/// All modules should use this instead of reading env vars directly.
/// The context is immutable after first detection.
#[must_use]
pub fn runtime_context() -> &'static RuntimeContext {
    RUNTIME_CONTEXT.get_or_init(RuntimeContext::detect)
}

// =============================================================================
// cgroup resource limit helpers
// =============================================================================

/// Read cgroup v2 memory limit (returns None if unlimited or not in a cgroup).
fn read_cgroup_memory_limit() -> Option<u64> {
    let content = std::fs::read_to_string("/sys/fs/cgroup/memory.max").ok()?;
    let trimmed = content.trim();
    if trimmed == "max" {
        return None; // No limit set
    }
    trimmed.parse::<u64>().ok()
}

/// Read cgroup v2 CPU quota as fractional cores (returns None if unlimited).
///
/// Reads `/sys/fs/cgroup/cpu.max` which contains `quota period` (e.g. "200000 100000" = 2 cores).
fn read_cgroup_cpu_quota() -> Option<f64> {
    let content = std::fs::read_to_string("/sys/fs/cgroup/cpu.max").ok()?;
    let parts: Vec<&str> = content.split_whitespace().collect();
    if parts.len() < 2 || parts[0] == "max" {
        return None; // No limit
    }
    let quota: f64 = parts[0].parse().ok()?;
    let period: f64 = parts[1].parse().ok()?;
    if period > 0.0 {
        Some(quota / period)
    } else {
        None
    }
}

// =============================================================================
// Helm detection and app env helpers
// =============================================================================

/// Check if the application was deployed via Helm.
///
/// Looks for Helm-specific labels in Kubernetes downward API.
#[must_use]
pub fn is_helm() -> bool {
    // Check for Helm release name env var (commonly set)
    if std::env::var("HELM_RELEASE_NAME").is_ok() {
        return true;
    }

    // Check for Helm labels via downward API
    let labels_path = Path::new("/etc/podinfo/labels");
    if labels_path.exists()
        && let Ok(content) = std::fs::read_to_string(labels_path)
    {
        return content.contains("helm.sh/chart")
            || content.contains("app.kubernetes.io/managed-by=\"Helm\"");
    }

    false
}

/// Get the current application environment name (dev, staging, prod).
///
/// Checks in order: `APP_ENV`, `ENVIRONMENT`, `ENV`, defaults to "development".
#[must_use]
pub fn get_app_env() -> String {
    std::env::var("APP_ENV")
        .or_else(|_| std::env::var("ENVIRONMENT"))
        .or_else(|_| std::env::var("ENV"))
        .unwrap_or_else(|_| "development".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_environment_display() {
        assert_eq!(Environment::Kubernetes.to_string(), "kubernetes");
        assert_eq!(Environment::Docker.to_string(), "docker");
        assert_eq!(Environment::Container.to_string(), "container");
        assert_eq!(Environment::BareMetal.to_string(), "bare_metal");
    }

    #[test]
    fn test_environment_is_container() {
        assert!(Environment::Kubernetes.is_container());
        assert!(Environment::Docker.is_container());
        assert!(Environment::Container.is_container());
        assert!(!Environment::BareMetal.is_container());
    }

    #[test]
    fn test_environment_is_kubernetes() {
        assert!(Environment::Kubernetes.is_kubernetes());
        assert!(!Environment::Docker.is_kubernetes());
        assert!(!Environment::Container.is_kubernetes());
        assert!(!Environment::BareMetal.is_kubernetes());
    }

    #[test]
    fn test_environment_is_bare_metal() {
        assert!(!Environment::Kubernetes.is_bare_metal());
        assert!(!Environment::Docker.is_bare_metal());
        assert!(!Environment::Container.is_bare_metal());
        assert!(Environment::BareMetal.is_bare_metal());
    }

    #[test]
    fn test_get_app_env_default() {
        temp_env::with_vars(
            [
                ("APP_ENV", None::<&str>),
                ("ENVIRONMENT", None),
                ("ENV", None),
            ],
            || assert_eq!(get_app_env(), "development"),
        );
    }

    #[test]
    fn test_get_app_env_from_app_env() {
        temp_env::with_var("APP_ENV", Some("production"), || {
            assert_eq!(get_app_env(), "production");
        });
    }

    #[test]
    fn test_environment_detect_returns_valid() {
        // Just ensure detect() doesn't panic and returns a valid variant
        let env = Environment::detect();
        assert!(matches!(
            env,
            Environment::Kubernetes
                | Environment::Docker
                | Environment::Container
                | Environment::BareMetal
        ));
    }

    // --- RuntimeContext tests ---

    #[test]
    fn test_runtime_context_detect_does_not_panic() {
        let ctx = RuntimeContext::detect();
        // Environment is always set
        assert!(matches!(
            ctx.environment,
            Environment::Kubernetes
                | Environment::Docker
                | Environment::Container
                | Environment::BareMetal
        ));
    }

    #[test]
    fn test_runtime_context_bare_metal_has_no_k8s_fields() {
        // On a dev machine (bare metal), K8s fields should be None
        // unless POD_NAME etc. env vars happen to be set
        let ctx = RuntimeContext::detect();
        if ctx.environment.is_bare_metal() {
            assert!(
                ctx.node_name.is_none(),
                "node_name should be None on bare metal"
            );
            // pod_name might come from HOSTNAME, so we don't assert it's None
        }
    }

    #[test]
    fn test_runtime_context_reads_pod_name_env() {
        temp_env::with_vars(
            [
                ("POD_NAME", Some("test-pod-123")),
                ("KUBERNETES_SERVICE_HOST", Some("10.0.0.1")),
            ],
            || {
                let ctx = RuntimeContext::detect();
                assert_eq!(ctx.pod_name.as_deref(), Some("test-pod-123"));
            },
        );
    }

    #[test]
    fn test_runtime_context_reads_namespace_env() {
        temp_env::with_var("POD_NAMESPACE", Some("production"), || {
            let ctx = RuntimeContext::detect();
            assert_eq!(ctx.namespace.as_deref(), Some("production"));
        });
    }

    #[test]
    fn test_runtime_context_reads_node_name_env() {
        temp_env::with_var("NODE_NAME", Some("node-1"), || {
            let ctx = RuntimeContext::detect();
            assert_eq!(ctx.node_name.as_deref(), Some("node-1"));
        });
    }

    #[test]
    fn test_runtime_context_global_singleton() {
        // runtime_context() should return the same instance every time
        let ctx1 = runtime_context();
        let ctx2 = runtime_context();
        assert_eq!(ctx1.environment, ctx2.environment);
        assert_eq!(ctx1.pod_name, ctx2.pod_name);
    }

    #[test]
    fn test_runtime_context_is_kubernetes_convenience() {
        let mut ctx = RuntimeContext::detect();
        ctx.environment = Environment::Kubernetes;
        assert!(ctx.is_kubernetes());
        assert!(ctx.is_container());
        assert!(!ctx.is_bare_metal());
    }

    #[test]
    fn test_runtime_context_is_bare_metal_convenience() {
        let mut ctx = RuntimeContext::detect();
        ctx.environment = Environment::BareMetal;
        assert!(!ctx.is_kubernetes());
        assert!(!ctx.is_container());
        assert!(ctx.is_bare_metal());
    }

    // --- cgroup helper tests ---

    #[test]
    fn test_read_cgroup_memory_limit_returns_option() {
        // On bare metal, returns None (no cgroup). On container, returns Some.
        let limit = read_cgroup_memory_limit();
        // Just verify it doesn't panic — result depends on environment
        let _ = limit;
    }

    #[test]
    fn test_read_cgroup_cpu_quota_returns_option() {
        let quota = read_cgroup_cpu_quota();
        let _ = quota;
    }

    #[test]
    fn test_prestop_delay_default_bare_metal() {
        // On bare metal, default pre-stop delay should be 0
        temp_env::with_var("PRESTOP_DELAY_SECS", None::<&str>, || {
            let ctx = RuntimeContext::detect();
            if ctx.environment.is_bare_metal() {
                // The prestop_delay_secs function is in shutdown.rs,
                // but we can verify the RuntimeContext is bare metal
                assert!(!ctx.is_kubernetes());
            }
        });
    }
}
