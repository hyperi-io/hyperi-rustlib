// Project:   hyperi-rustlib
// File:      src/deployment/contract_identity.rs
// Purpose:   Contract Identity Annotation Scheme v1
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Contract Identity Annotation Scheme v1.
//!
//! Stamps every deployment artefact (OCI image, Helm chart, ArgoCD Application)
//! with three uniform, greppable identity annotations:
//!
//! | Key                                | Meaning                              | Format                            |
//! |------------------------------------|--------------------------------------|-----------------------------------|
//! | `io.hyperi.contract.version`       | Contract schema version              | Literal string `v1`               |
//! | `io.hyperi.contract.source-commit` | Git SHA of the consumer app's HEAD   | 40-char lowercase hex             |
//! | `io.hyperi.contract.image-ref`     | Intended pull reference for the image | `<reg>/<repo>:<tag>` or `@<digest>` |
//!
//! Same key string on every surface (image label, Chart.yaml annotation,
//! Application annotation). The grep payoff: `grep -r 'io.hyperi.contract' .`
//! finds every contract-emitted artefact regardless of format.
//!
//! # Pre-push vs post-push image_ref
//!
//! At Dockerfile-emit time the image isn't built yet, let alone pushed, so
//! its content digest is unknown. The image label therefore carries the
//! TAG form (`<reg>/<repo>:<tag>`). The push step is responsible for
//! re-rendering `Chart.yaml` and the ArgoCD `Application` with the DIGEST
//! form (`<reg>/<repo>@sha256:<hex>`) returned by the registry, because
//! those manifests are written after the push completes and benefit from
//! digest-pinning for reproducibility.
//!
//! # Rollout phase
//!
//! Phase 1 (this commit): generators accept `Option<&ContractIdentity>`.
//! When `Some(id)`, the three annotations are emitted; when `None`, the
//! generator is silent (backwards-compat for consumers that haven't
//! adopted yet). Phase 2 flips the parameter to required once all six
//! DFE consumers pass identity at every call site. Phase 3 removes the
//! Option<> wrapper and the silent-on-None branch.

use std::env;
use std::process::Command;

/// Annotation key prefix shared across all three keys.
pub const KEY_PREFIX: &str = "io.hyperi.contract";

/// Schema version literal. Bumps only when the contract format itself
/// breaks, NOT when the consumer's app version moves.
pub const VERSION: &str = "v1";

/// Errors from constructing or detecting a `ContractIdentity`.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    /// `source_commit` was not 40-char lowercase hex.
    #[error("source_commit must be 40-char lowercase hex (got {got:?})")]
    InvalidSourceCommit {
        /// The rejected value (echoed so the caller can log it).
        got: String,
    },

    /// `image_ref` was empty or lacked an explicit registry host.
    ///
    /// Bare names like `nginx:1.25` are rejected per spec -- we never
    /// allow the implicit `docker.io/library/` prefix. The registry host
    /// must be present explicitly and look like an FQDN
    /// (contains `.`), a `host:port`, or the literal `localhost`.
    #[error(
        "image_ref must include an explicit registry host (got {got:?}); \
         no implicit docker.io/library/ prefix allowed"
    )]
    InvalidImageRef {
        /// The rejected value (echoed so the caller can log it).
        got: String,
    },

    /// `source_commit` couldn't be auto-detected.
    #[error(
        "could not auto-detect source_commit: GITHUB_SHA env var is unset \
         or invalid, and `git rev-parse HEAD` failed: {reason}"
    )]
    DetectFailed {
        /// What the detection attempt reported.
        reason: String,
    },
}

/// Three-key identity stamped on every deployment artefact.
///
/// Construct via [`ContractIdentity::new`] when both inputs are known
/// up front (typical for the push step that has the registry digest),
/// or via [`ContractIdentity::detect`] to auto-resolve `source_commit`
/// from CI env or local git when only `image_ref` is in hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractIdentity {
    source_commit: String,
    image_ref: String,
}

impl ContractIdentity {
    /// Construct from explicit inputs.
    ///
    /// `source_commit` must be 40-char lowercase hex (no `sha256:` prefix --
    /// it's a git SHA, not a content hash). `image_ref` must be non-empty
    /// and include an explicit registry host (no implicit `docker.io`).
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::InvalidSourceCommit`] or
    /// [`IdentityError::InvalidImageRef`] when validation fails.
    pub fn new(
        source_commit: impl Into<String>,
        image_ref: impl Into<String>,
    ) -> Result<Self, IdentityError> {
        let source_commit = source_commit.into();
        let image_ref = image_ref.into();
        validate_source_commit(&source_commit)?;
        validate_image_ref(&image_ref)?;
        Ok(Self {
            source_commit,
            image_ref,
        })
    }

    /// Auto-detect `source_commit` from the CI env (`GITHUB_SHA`) or
    /// `git rev-parse HEAD` and combine with the caller-supplied
    /// `image_ref`.
    ///
    /// `image_ref` always comes from the caller because it derives from
    /// the contract (`<image_registry>/<app_name>:<tag>`) and the
    /// detection layer has no business reading the contract.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::DetectFailed`] when neither source works,
    /// or [`IdentityError::InvalidImageRef`] when the caller-supplied
    /// ref fails validation.
    pub fn detect(image_ref: impl Into<String>) -> Result<Self, IdentityError> {
        let source_commit = detect_source_commit()?;
        Self::new(source_commit, image_ref)
    }

    /// Schema version literal (`"v1"` in this build).
    #[must_use]
    pub fn version(&self) -> &'static str {
        VERSION
    }

    /// 40-char lowercase hex git SHA of the consumer app's HEAD.
    #[must_use]
    pub fn source_commit(&self) -> &str {
        &self.source_commit
    }

    /// Intended pull reference for the container image.
    #[must_use]
    pub fn image_ref(&self) -> &str {
        &self.image_ref
    }

    /// Render as three Dockerfile `LABEL` lines, in canonical order
    /// (version, source-commit, image-ref). No trailing newline; the
    /// caller is responsible for any separator.
    #[must_use]
    pub fn as_dockerfile_labels(&self) -> String {
        format!(
            "LABEL {KEY_PREFIX}.version=\"{VERSION}\"\n\
             LABEL {KEY_PREFIX}.source-commit=\"{c}\"\n\
             LABEL {KEY_PREFIX}.image-ref=\"{r}\"",
            c = self.source_commit,
            r = self.image_ref,
        )
    }

    /// Render as three YAML annotation lines, indented by `indent`
    /// spaces. Suitable for inclusion under `metadata.annotations:`
    /// (ArgoCD Application) or top-level `annotations:` (Helm
    /// `Chart.yaml`). No trailing newline.
    #[must_use]
    pub fn as_yaml_annotations(&self, indent: usize) -> String {
        let pad = " ".repeat(indent);
        format!(
            "{pad}{KEY_PREFIX}.version: \"{VERSION}\"\n\
             {pad}{KEY_PREFIX}.source-commit: \"{c}\"\n\
             {pad}{KEY_PREFIX}.image-ref: \"{r}\"",
            c = self.source_commit,
            r = self.image_ref,
        )
    }
}

fn validate_source_commit(s: &str) -> Result<(), IdentityError> {
    if s.len() != 40 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(IdentityError::InvalidSourceCommit { got: s.to_string() });
    }
    Ok(())
}

fn validate_image_ref(s: &str) -> Result<(), IdentityError> {
    if s.is_empty() {
        return Err(IdentityError::InvalidImageRef { got: s.to_string() });
    }
    // Must contain '/' separating host from repo path.
    let Some((host, _rest)) = s.split_once('/') else {
        return Err(IdentityError::InvalidImageRef { got: s.to_string() });
    };
    // Host must look like an FQDN (contains '.'), a host:port (contains
    // ':'), or the literal 'localhost'. Bare repo segments like 'library'
    // would mean "default docker.io" -- forbidden per spec.
    if host == "localhost" || host.contains('.') || host.contains(':') {
        Ok(())
    } else {
        Err(IdentityError::InvalidImageRef { got: s.to_string() })
    }
}

fn detect_source_commit() -> Result<String, IdentityError> {
    // 1. Prefer GITHUB_SHA env var -- standard in GitHub Actions runners.
    if let Ok(sha) = env::var("GITHUB_SHA") {
        let sha = sha.trim().to_lowercase();
        if validate_source_commit(&sha).is_ok() {
            return Ok(sha);
        }
    }
    // 2. Fall back to `git rev-parse HEAD` from cwd.
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|e| IdentityError::DetectFailed {
            reason: e.to_string(),
        })?;
    if !output.status.success() {
        return Err(IdentityError::DetectFailed {
            reason: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let sha = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase();
    validate_source_commit(&sha)?;
    Ok(sha)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn new_accepts_valid_inputs() {
        let id = ContractIdentity::new(VALID_SHA, "ghcr.io/hyperi-io/dfe-loader:v2.7.2").unwrap();
        assert_eq!(id.version(), "v1");
        assert_eq!(id.source_commit(), VALID_SHA);
        assert_eq!(id.image_ref(), "ghcr.io/hyperi-io/dfe-loader:v2.7.2");
    }

    #[test]
    fn new_rejects_short_source_commit() {
        let err = ContractIdentity::new("abc123", "ghcr.io/x/y:v1").unwrap_err();
        assert!(matches!(err, IdentityError::InvalidSourceCommit { .. }));
    }

    #[test]
    fn new_rejects_uppercase_source_commit() {
        let upper = "0123456789ABCDEF0123456789ABCDEF01234567";
        let err = ContractIdentity::new(upper, "ghcr.io/x/y:v1").unwrap_err();
        assert!(matches!(err, IdentityError::InvalidSourceCommit { .. }));
    }

    #[test]
    fn new_rejects_source_commit_with_sha256_prefix() {
        let prefixed = format!("sha256:{VALID_SHA}");
        let err = ContractIdentity::new(prefixed, "ghcr.io/x/y:v1").unwrap_err();
        assert!(matches!(err, IdentityError::InvalidSourceCommit { .. }));
    }

    #[test]
    fn new_rejects_empty_image_ref() {
        let err = ContractIdentity::new(VALID_SHA, "").unwrap_err();
        assert!(matches!(err, IdentityError::InvalidImageRef { .. }));
    }

    #[test]
    fn new_rejects_bare_docker_hub_shortcut() {
        // No registry host -- this would mean implicit docker.io/library/.
        let err = ContractIdentity::new(VALID_SHA, "nginx:1.25").unwrap_err();
        assert!(matches!(err, IdentityError::InvalidImageRef { .. }));
    }

    #[test]
    fn new_rejects_bare_repo_path_without_host() {
        // "library/nginx" has a '/' but the prefix isn't a hostname.
        let err = ContractIdentity::new(VALID_SHA, "library/nginx:1.25").unwrap_err();
        assert!(matches!(err, IdentityError::InvalidImageRef { .. }));
    }

    #[test]
    fn new_accepts_localhost_registry() {
        // The local-registry pattern (kind+registry:2) commonly uses
        // localhost:5000/<repo>.
        let id = ContractIdentity::new(VALID_SHA, "localhost:5000/dfe-loader:test").unwrap();
        assert_eq!(id.image_ref(), "localhost:5000/dfe-loader:test");
    }

    #[test]
    fn new_accepts_digest_form() {
        let digest_ref = format!("ghcr.io/hyperi-io/dfe-loader@sha256:{VALID_SHA}");
        let id = ContractIdentity::new(VALID_SHA, digest_ref.clone()).unwrap();
        assert_eq!(id.image_ref(), digest_ref);
    }

    #[test]
    fn dockerfile_labels_canonical_order_and_quoting() {
        let id = ContractIdentity::new(VALID_SHA, "ghcr.io/hyperi-io/dfe-loader:v2.7.2").unwrap();
        let out = id.as_dockerfile_labels();
        assert_eq!(
            out,
            "LABEL io.hyperi.contract.version=\"v1\"\n\
             LABEL io.hyperi.contract.source-commit=\"0123456789abcdef0123456789abcdef01234567\"\n\
             LABEL io.hyperi.contract.image-ref=\"ghcr.io/hyperi-io/dfe-loader:v2.7.2\""
        );
    }

    #[test]
    fn yaml_annotations_canonical_order_and_quoting() {
        let id = ContractIdentity::new(VALID_SHA, "ghcr.io/hyperi-io/dfe-loader:v2.7.2").unwrap();
        let out = id.as_yaml_annotations(4);
        assert_eq!(
            out,
            "    io.hyperi.contract.version: \"v1\"\n    \
             io.hyperi.contract.source-commit: \"0123456789abcdef0123456789abcdef01234567\"\n    \
             io.hyperi.contract.image-ref: \"ghcr.io/hyperi-io/dfe-loader:v2.7.2\""
        );
    }

    #[test]
    fn yaml_annotations_zero_indent() {
        let id = ContractIdentity::new(VALID_SHA, "ghcr.io/x/y:v1").unwrap();
        let out = id.as_yaml_annotations(0);
        assert!(out.starts_with("io.hyperi.contract.version: \"v1\""));
    }

    #[test]
    fn key_prefix_is_grep_target() {
        // Sanity check the documented grep payoff:
        //   grep -r 'io.hyperi.contract' .
        // -- which only works if every output line literally contains
        // that string.
        let id = ContractIdentity::new(VALID_SHA, "ghcr.io/x/y:v1").unwrap();
        let dockerfile = id.as_dockerfile_labels();
        let yaml = id.as_yaml_annotations(2);
        assert_eq!(dockerfile.matches(KEY_PREFIX).count(), 3);
        assert_eq!(yaml.matches(KEY_PREFIX).count(), 3);
    }
}
