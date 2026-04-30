// Project:   hyperi-rustlib
// File:      src/deployment/registry.rs
// Purpose:   Config-cascade-driven container registry resolution
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Container registry resolution for deployment contracts.
//!
//! The publish-target registry (where the built image is pushed) and the
//! base image (the `FROM` line) are org-wide decisions, not per-app. This
//! module reads them from the config cascade so they live in YAML config
//! rather than being hardcoded in each app's contract source.
//!
//! # Cascade keys
//!
//! ```yaml
//! deployment:
//!   image_registry: ghcr.io/hyperi-io        # default: ghcr.io/hyperi-io
//!   base_image: ubuntu:24.04                 # default: ubuntu:24.04
//! ```
//!
//! # Defaults
//!
//! - [`DEFAULT_IMAGE_REGISTRY`] = `ghcr.io/hyperi-io` — where built images go
//! - [`DEFAULT_BASE_IMAGE`] = `ubuntu:24.04` — what the runtime stage builds on
//!
//! When (eventually) a curated GHCR base image lands at
//! `ghcr.io/hyperi-io/dfe-base:ubuntu-24.04`, ops can override
//! `deployment.base_image` in the cascade without rebuilding the apps.

/// Default publish-target registry for HyperI org.
///
/// Combined with the contract's `app_name` to produce
/// `<DEFAULT_IMAGE_REGISTRY>/<app_name>:<version>`.
pub const DEFAULT_IMAGE_REGISTRY: &str = "ghcr.io/hyperi-io";

/// Default base image for the runtime stage.
///
/// Pulled from Docker Hub (no registry prefix). To use a curated GHCR base,
/// set `deployment.base_image` in the YAML cascade to e.g.
/// `ghcr.io/hyperi-io/dfe-base:ubuntu-24.04` once that image exists.
pub const DEFAULT_BASE_IMAGE: &str = "ubuntu:24.04";

/// Read the publish-target image registry from the config cascade.
///
/// Reads `deployment.image_registry` from the YAML cascade. Falls back to
/// [`DEFAULT_IMAGE_REGISTRY`] when not set, when config isn't loaded, or
/// when the `config` feature is disabled.
///
/// # Example
///
/// ```rust,no_run
/// use hyperi_rustlib::deployment::{DeploymentContract, image_registry_from_cascade};
/// # fn dummy() -> DeploymentContract { unimplemented!() }
/// let mut contract = dummy();
/// contract.image_registry = image_registry_from_cascade();
/// ```
#[must_use]
pub fn image_registry_from_cascade() -> String {
    #[cfg(feature = "config")]
    {
        if let Some(cfg) = crate::config::try_get()
            && let Some(s) = cfg.get_string("deployment.image_registry")
            && !s.is_empty()
        {
            return s;
        }
    }
    DEFAULT_IMAGE_REGISTRY.to_string()
}

/// Read the runtime base image from the config cascade.
///
/// Reads `deployment.base_image` from the YAML cascade. Falls back to
/// [`DEFAULT_BASE_IMAGE`] when not set.
#[must_use]
pub fn base_image_from_cascade() -> String {
    #[cfg(feature = "config")]
    {
        if let Some(cfg) = crate::config::try_get()
            && let Some(s) = cfg.get_string("deployment.base_image")
            && !s.is_empty()
        {
            return s;
        }
    }
    DEFAULT_BASE_IMAGE.to_string()
}

/// Read the git repo URL for ArgoCD generation from the config cascade.
///
/// Reads `deployment.argocd.repo_url` from the YAML cascade. Falls back to
/// `https://github.com/hyperi-io/{app_name}` if not set — matches the org
/// convention.
#[must_use]
pub fn argocd_repo_url_from_cascade(app_name: &str) -> String {
    #[cfg(feature = "config")]
    {
        if let Some(cfg) = crate::config::try_get()
            && let Some(s) = cfg.get_string("deployment.argocd.repo_url")
            && !s.is_empty()
        {
            return s;
        }
    }
    format!("https://github.com/hyperi-io/{app_name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_ghcr_friendly() {
        assert_eq!(DEFAULT_IMAGE_REGISTRY, "ghcr.io/hyperi-io");
        assert_eq!(DEFAULT_BASE_IMAGE, "ubuntu:24.04");
    }

    #[test]
    fn cascade_falls_back_to_defaults_when_no_config() {
        // No config setup → returns defaults.
        assert_eq!(image_registry_from_cascade(), DEFAULT_IMAGE_REGISTRY);
        assert_eq!(base_image_from_cascade(), DEFAULT_BASE_IMAGE);
    }
}
