// Project:   hyperi-rustlib
// File:      src/deployment/native_deps.rs
// Purpose:   Runtime native dependency declarations for container images
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Runtime native dependency contracts for Dockerfile generation.
//!
//! Maps hyperi-rustlib Cargo features to the system packages needed at runtime
//! in the container image. The Dockerfile generator uses this to emit APT repo
//! setup and `apt-get install` commands automatically.

use serde::{Deserialize, Serialize};

/// Runtime native dependencies for a container image.
///
/// Populated via [`NativeDepsContract::for_rustlib_features`] — pass the list
/// of hyperi-rustlib features your app enables, get back the runtime packages
/// and any custom APT repos needed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NativeDepsContract {
    /// Custom APT repositories to add before installing packages.
    #[serde(default)]
    pub apt_repos: Vec<AptRepoContract>,

    /// APT packages to install from default repos.
    #[serde(default)]
    pub apt_packages: Vec<String>,
}

/// A custom APT repository (e.g., Confluent for librdkafka).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AptRepoContract {
    /// GPG key URL for the repo.
    pub key_url: String,

    /// Local keyring file path (e.g., `/usr/share/keyrings/confluent-clients.gpg`).
    pub keyring: String,

    /// Repository base URL (e.g., `https://packages.confluent.io/clients/deb`).
    pub url: String,

    /// Distribution codename (e.g., `noble`, `bookworm`).
    /// If empty, derived from the base image at generation time.
    #[serde(default)]
    pub codename: String,

    /// APT packages to install from this specific repo.
    pub packages: Vec<String>,
}

/// Confluent APT repository for librdkafka.
fn confluent_repo(codename: &str) -> AptRepoContract {
    AptRepoContract {
        key_url: "https://packages.confluent.io/clients/deb/archive.key".into(),
        keyring: "/usr/share/keyrings/confluent-clients.gpg".into(),
        url: "https://packages.confluent.io/clients/deb".into(),
        codename: codename.into(),
        packages: vec!["librdkafka1".into()],
    }
}

/// Derive the APT codename from a base image string.
///
/// Maps common base images to their codenames. Falls back to `noble` if
/// the image is not recognised.
fn codename_from_base_image(base_image: &str) -> &'static str {
    if base_image.contains("bookworm") {
        "bookworm"
    } else if base_image.contains("jammy") {
        "jammy"
    } else if base_image.contains("focal") {
        "focal"
    } else {
        // ubuntu:24.04 and anything else → noble
        "noble"
    }
}

impl NativeDepsContract {
    /// Build runtime native deps from a list of hyperi-rustlib feature names.
    ///
    /// Pass the same feature strings you use in `Cargo.toml` (e.g.,
    /// `"transport-kafka"`, `"spool"`, `"secrets-aws"`). The base image is
    /// used to derive the APT codename for custom repos.
    ///
    /// # Example
    ///
    /// ```rust
    /// use hyperi_rustlib::deployment::NativeDepsContract;
    ///
    /// let deps = NativeDepsContract::for_rustlib_features(
    ///     &["transport-kafka", "spool", "tiered-sink", "secrets"],
    ///     "ubuntu:24.04",
    /// );
    /// assert!(!deps.apt_packages.is_empty());
    /// assert!(!deps.apt_repos.is_empty());
    /// ```
    #[must_use]
    pub fn for_rustlib_features(features: &[&str], base_image: &str) -> Self {
        let codename = codename_from_base_image(base_image);
        let mut apt_repos = Vec::new();
        let mut packages: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        let mut add = |pkg: &str| {
            if seen.insert(pkg.to_string()) {
                packages.push(pkg.into());
            }
        };

        let needs_kafka = features
            .iter()
            .any(|f| *f == "transport-kafka" || f.starts_with("dlq-kafka"));

        if needs_kafka {
            apt_repos.push(confluent_repo(codename));
            add("libssl3");
            add("zlib1g");
        }

        let needs_zstd = features
            .iter()
            .any(|f| *f == "spool" || *f == "tiered-sink");
        if needs_zstd {
            add("libzstd1");
        }

        // openssl is a transitive dep for many features (http, secrets, transport)
        let needs_ssl = features.iter().any(|f| {
            *f == "http"
                || f.starts_with("secrets")
                || f.starts_with("transport")
                || *f == "config-postgres"
                || f.starts_with("otel")
        });
        if needs_ssl {
            add("libssl3");
            add("zlib1g");
        }

        // directory-config-git needs libgit2
        let needs_git2 = features.contains(&"directory-config-git");
        if needs_git2 {
            add("libgit2-1.7");
        }

        Self {
            apt_repos,
            apt_packages: packages,
        }
    }

    /// Returns true if there are no native deps to install.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.apt_repos.is_empty() && self.apt_packages.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kafka_features_add_confluent_repo() {
        let deps = NativeDepsContract::for_rustlib_features(&["transport-kafka"], "ubuntu:24.04");
        assert_eq!(deps.apt_repos.len(), 1);
        assert!(deps.apt_repos[0].url.contains("confluent"));
        assert!(deps.apt_repos[0].packages.contains(&"librdkafka1".into()));
        assert_eq!(deps.apt_repos[0].codename, "noble");
        assert!(deps.apt_packages.contains(&"libssl3".into()));
        assert!(deps.apt_packages.contains(&"zlib1g".into()));
    }

    #[test]
    fn test_spool_adds_zstd() {
        let deps = NativeDepsContract::for_rustlib_features(&["spool"], "ubuntu:24.04");
        assert!(deps.apt_packages.contains(&"libzstd1".into()));
    }

    #[test]
    fn test_tiered_sink_adds_zstd() {
        let deps = NativeDepsContract::for_rustlib_features(&["tiered-sink"], "ubuntu:24.04");
        assert!(deps.apt_packages.contains(&"libzstd1".into()));
    }

    #[test]
    fn test_no_features_empty() {
        let deps = NativeDepsContract::for_rustlib_features(&[], "ubuntu:24.04");
        assert!(deps.is_empty());
    }

    #[test]
    fn test_pure_rust_features_empty() {
        let deps = NativeDepsContract::for_rustlib_features(
            &["cli", "deployment", "logger"],
            "ubuntu:24.04",
        );
        assert!(deps.is_empty());
    }

    #[test]
    fn test_bookworm_codename() {
        let deps =
            NativeDepsContract::for_rustlib_features(&["transport-kafka"], "debian:bookworm-slim");
        assert_eq!(deps.apt_repos[0].codename, "bookworm");
    }

    #[test]
    fn test_no_duplicate_packages() {
        let deps = NativeDepsContract::for_rustlib_features(
            &["transport-kafka", "http", "secrets"],
            "ubuntu:24.04",
        );
        let ssl_count = deps.apt_packages.iter().filter(|p| *p == "libssl3").count();
        assert_eq!(ssl_count, 1);
    }

    #[test]
    fn test_dlq_kafka_adds_confluent() {
        let deps = NativeDepsContract::for_rustlib_features(&["dlq-kafka"], "ubuntu:24.04");
        assert_eq!(deps.apt_repos.len(), 1);
    }

    #[test]
    fn test_git2_feature() {
        let deps =
            NativeDepsContract::for_rustlib_features(&["directory-config-git"], "ubuntu:24.04");
        assert!(deps.apt_packages.contains(&"libgit2-1.7".into()));
    }

    #[test]
    fn test_full_receiver_features() {
        let deps = NativeDepsContract::for_rustlib_features(
            &[
                "config",
                "config-reload",
                "logger",
                "metrics",
                "http-server",
                "transport-kafka",
                "transport-grpc",
                "dlq-kafka",
                "spool",
                "tiered-sink",
                "runtime",
                "secrets",
                "scaling",
                "cli",
                "deployment",
            ],
            "ubuntu:24.04",
        );
        assert_eq!(deps.apt_repos.len(), 1); // confluent only
        assert!(!deps.apt_packages.contains(&"librdkafka1".to_string())); // in repo packages
        assert!(deps.apt_repos[0].packages.contains(&"librdkafka1".into()));
        assert!(deps.apt_packages.contains(&"libssl3".into()));
        assert!(deps.apt_packages.contains(&"libzstd1".into()));
        assert!(deps.apt_packages.contains(&"zlib1g".into()));
    }
}
