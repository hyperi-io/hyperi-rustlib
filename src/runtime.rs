// Project:   hyperi-rustlib
// File:      src/runtime.rs
// Purpose:   Container-aware runtime path management
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Runtime path management.
//!
//! Provides container-aware path resolution that works identically in
//! Kubernetes, Docker, and local development environments.

use std::path::PathBuf;

use crate::env::Environment;

/// Default container base path.
const CONTAINER_BASE_PATH: &str = "/app";

/// Standard application paths based on runtime environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    /// Read-only configuration directory (ConfigMap in K8s, ~/.config locally)
    pub config_dir: PathBuf,
    /// Read-only secrets directory (Secret in K8s, ~/.{app}/secrets locally)
    pub secrets_dir: PathBuf,
    /// Persistent data directory (PVC in K8s, ~/.local/share locally)
    pub data_dir: PathBuf,
    /// Ephemeral temporary directory (EmptyDir in K8s, /tmp locally)
    pub temp_dir: PathBuf,
    /// Application logs directory
    pub logs_dir: PathBuf,
    /// Cache directory
    pub cache_dir: PathBuf,
    /// Runtime directory (PID files, sockets)
    pub run_dir: PathBuf,
}

impl RuntimePaths {
    /// Discover paths based on auto-detected environment.
    #[must_use]
    pub fn discover() -> Self {
        Self::discover_for(Environment::detect())
    }

    /// Discover paths for a specific environment.
    #[must_use]
    pub fn discover_for(env: Environment) -> Self {
        match env {
            Environment::Kubernetes | Environment::Docker | Environment::Container => {
                Self::container_paths()
            }
            Environment::BareMetal => Self::local_paths(),
        }
    }

    /// Get paths for container environments.
    fn container_paths() -> Self {
        let base = std::env::var("CONTAINER_BASE_PATH")
            .unwrap_or_else(|_| CONTAINER_BASE_PATH.to_string());
        let base_path = PathBuf::from(&base);

        Self {
            config_dir: base_path.join("config"),
            secrets_dir: base_path.join("secrets"),
            data_dir: base_path.join("data"),
            temp_dir: base_path.join("tmp"),
            logs_dir: base_path.join("logs"),
            cache_dir: base_path.join("cache"),
            run_dir: base_path.join("run"),
        }
    }

    /// Get paths for local development (XDG-compliant).
    fn local_paths() -> Self {
        let app_name = std::env::var("APP_NAME").unwrap_or_else(|_| "hs-app".to_string());

        // Use dirs crate for XDG-compliant paths
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join(&app_name);

        let data_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("~/.local/share"))
            .join(&app_name);

        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("~/.cache"))
            .join(&app_name);

        let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));

        Self {
            config_dir,
            secrets_dir: home_dir.join(format!(".{app_name}")).join("secrets"),
            data_dir: data_dir.clone(),
            temp_dir: std::env::temp_dir().join(&app_name),
            logs_dir: data_dir.join("logs"),
            cache_dir,
            run_dir: dirs::runtime_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join(&app_name),
        }
    }

    /// Create all directories if they don't exist.
    ///
    /// # Errors
    ///
    /// Returns an error if directory creation fails.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.config_dir)?;
        std::fs::create_dir_all(&self.secrets_dir)?;
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(&self.temp_dir)?;
        std::fs::create_dir_all(&self.logs_dir)?;
        std::fs::create_dir_all(&self.cache_dir)?;
        std::fs::create_dir_all(&self.run_dir)?;
        Ok(())
    }

    /// Check if all required directories exist.
    #[must_use]
    pub fn all_exist(&self) -> bool {
        self.config_dir.exists()
            && self.secrets_dir.exists()
            && self.data_dir.exists()
            && self.temp_dir.exists()
            && self.logs_dir.exists()
            && self.cache_dir.exists()
            && self.run_dir.exists()
    }
}

impl Default for RuntimePaths {
    fn default() -> Self {
        Self::discover()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_paths() {
        let paths = RuntimePaths::discover_for(Environment::Kubernetes);

        assert_eq!(paths.config_dir, PathBuf::from("/app/config"));
        assert_eq!(paths.secrets_dir, PathBuf::from("/app/secrets"));
        assert_eq!(paths.data_dir, PathBuf::from("/app/data"));
        assert_eq!(paths.temp_dir, PathBuf::from("/app/tmp"));
        assert_eq!(paths.logs_dir, PathBuf::from("/app/logs"));
        assert_eq!(paths.cache_dir, PathBuf::from("/app/cache"));
        assert_eq!(paths.run_dir, PathBuf::from("/app/run"));
    }

    #[test]
    fn test_docker_uses_container_paths() {
        let docker_paths = RuntimePaths::discover_for(Environment::Docker);
        let k8s_paths = RuntimePaths::discover_for(Environment::Kubernetes);

        // Docker and K8s should use same container paths
        assert_eq!(docker_paths, k8s_paths);
    }

    #[test]
    fn test_local_paths_use_xdg() {
        let paths = RuntimePaths::discover_for(Environment::BareMetal);

        // Local paths should be in home directory, not /app
        assert!(!paths.config_dir.starts_with("/app"));
        assert!(!paths.data_dir.starts_with("/app"));
    }

    #[test]
    fn test_custom_container_base_path() {
        std::env::set_var("CONTAINER_BASE_PATH", "/custom");
        let paths = RuntimePaths::discover_for(Environment::Docker);
        std::env::remove_var("CONTAINER_BASE_PATH");

        assert_eq!(paths.config_dir, PathBuf::from("/custom/config"));
    }
}
