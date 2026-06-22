// Project:   hyperi-rustlib
// File:      src/tls.rs
// Purpose:   Unified TLS trust + client-config construction (private-CA first)
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! # Unified TLS trust
//!
//! One place to build a rustls [`ClientConfig`] for every HyperI transport that
//! speaks TLS (HTTP, gRPC, Vault, Redis; Kafka maps the same vocabulary onto
//! librdkafka file paths). Collapses the previously ad-hoc per-transport TLS
//! handling into a single trust model with first-class **private-CA** support
//! -- the common HyperI deployment shape (an internal CA, not the public web
//! PKI). Mirrors the design used in the HyperI clickhouse-rs fork.
//!
//! ## Crypto provider
//!
//! Client configs are built with an **explicit aws-lc-rs provider** rather than
//! relying on the rustls process-default. The dependency graph already pulls
//! BOTH aws-lc-rs (via reqwest / aws-smithy) and ring (via sqlx), so there is
//! no unambiguous process-default provider -- `ClientConfig::builder()` would
//! panic at runtime ("no process-level CryptoProvider"). Selecting aws-lc-rs
//! explicitly matches the reqwest / AWS path and removes that footgun.
//!
//! ## Trust model
//!
//! [`TlsTrust`] composes roots from up to three sources:
//! - **native** OS roots (`rustls-native-certs`),
//! - **webpki** Mozilla roots (`webpki-roots`),
//! - **extra** PEM files (`extra_roots` + `extra_intermediates`) -- the private
//!   CA, accepted as a single bundle file or several.
//!
//! `exclusive = true` trusts ONLY the extra PEM files (native + webpki ignored)
//! -- pin to your private CA and nothing else. An empty resulting store is an
//! error, never a silent "trust nothing / trust everything".
//!
//! ## No `InsecureSkipVerify`
//!
//! This module offers no certificate-verification bypass. Transports that still
//! expose a dev-only `skip_verify` flag gate it to non-production (see
//! `KafkaConfig::validate` / `OpenBaoConfig::validate`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::{ClientConfig, RootCertStore};
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;

/// Errors building a TLS trust store or client config.
#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    /// A certificate file could not be opened or read as PEM.
    #[error("TLS: cannot read certificate file {path}: {reason}")]
    PemRead {
        /// The offending path.
        path: PathBuf,
        /// Human-readable reason.
        reason: String,
    },

    /// A file was read but contained no usable certificates.
    #[error("TLS: no certificates parsed from {path}")]
    NoCertsFound {
        /// The offending path.
        path: PathBuf,
    },

    /// The resulting trust store has no roots (would trust nothing).
    #[error(
        "TLS: trust store is empty -- no roots from native/webpki/extra sources \
         (check `native_roots`/`webpki_roots`/`extra_roots`, or `exclusive` with no files)"
    )]
    EmptyTrustStore,

    /// rustls rejected the assembled client configuration.
    #[error("TLS: failed to build client config: {0}")]
    Build(String),
}

/// Where a transport's TLS roots come from.
///
/// `native` + `webpki` are convenience root sources; `extra_*` are explicit
/// PEM files (the private CA). See the module docs for `exclusive`.
#[derive(Debug, Clone)]
pub struct TlsTrust {
    /// Trust the OS native root store.
    pub native_roots: bool,
    /// Trust the bundled Mozilla (webpki) root store.
    pub webpki_roots: bool,
    /// Extra root-CA PEM files to trust (e.g. a private CA bundle).
    pub extra_roots: Vec<PathBuf>,
    /// Extra intermediate-CA PEM files to add as trust anchors. Useful when a
    /// private CA ships an intermediate the server does not present.
    pub extra_intermediates: Vec<PathBuf>,
    /// Trust ONLY `extra_*` -- ignore native and webpki regardless of their
    /// flags. Pin to the private CA and nothing else.
    pub exclusive: bool,
}

impl Default for TlsTrust {
    /// Native OS roots, no webpki, no extras, non-exclusive -- the
    /// public-internet default.
    fn default() -> Self {
        Self {
            native_roots: true,
            webpki_roots: false,
            extra_roots: Vec::new(),
            extra_intermediates: Vec::new(),
            exclusive: false,
        }
    }
}

impl TlsTrust {
    /// Trust only the given private-CA PEM bundle (exclusive pin).
    #[must_use]
    pub fn private_ca(pem_path: impl Into<PathBuf>) -> Self {
        Self {
            native_roots: false,
            webpki_roots: false,
            extra_roots: vec![pem_path.into()],
            extra_intermediates: Vec::new(),
            exclusive: true,
        }
    }
}

/// Source for a client `ClientConfig`: either fully pre-built, or assembled
/// from a [`TlsTrust`].
pub enum TlsConfigSource {
    /// Use a caller-supplied config verbatim.
    Explicit(Arc<ClientConfig>),
    /// Build from a trust specification.
    Trust(TlsTrust),
}

/// Append certificates from a PEM file into `store`, leniently.
///
/// Skips unparseable PEM blocks and non-certificate DER, so a bundle mixing
/// junk with valid certs still loads the valid ones. Returns the number of
/// certificates added. Errors only if the path is not a readable file, or if
/// zero certificates were added.
///
/// # Errors
///
/// [`TlsError::PemRead`] if the path is not a readable file;
/// [`TlsError::NoCertsFound`] if no certificate parsed from it.
pub fn add_pem_file_certs(store: &mut RootCertStore, path: &Path) -> Result<usize, TlsError> {
    // Guard the known footgun: `pem_file_iter` loops forever on a directory
    // (rustls/pki-types#98). Also gives a clean error for missing files.
    if !path.is_file() {
        return Err(TlsError::PemRead {
            path: path.to_path_buf(),
            reason: "not a readable file".to_string(),
        });
    }

    let iter = CertificateDer::pem_file_iter(path).map_err(|e| TlsError::PemRead {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;

    // Lenient: drop unreadable PEM blocks (filter_map ok), then let
    // add_parsable_certificates drop any DER that is not a valid certificate.
    let certs: Vec<CertificateDer<'static>> = iter.filter_map(Result::ok).collect();
    let (added, _ignored) = store.add_parsable_certificates(certs);

    if added == 0 {
        return Err(TlsError::NoCertsFound {
            path: path.to_path_buf(),
        });
    }
    Ok(added)
}

/// Assemble a [`RootCertStore`] from a [`TlsTrust`].
///
/// # Errors
///
/// Propagates [`add_pem_file_certs`] errors for any `extra_*` file, and returns
/// [`TlsError::EmptyTrustStore`] if the assembled store has no roots.
pub fn build_root_store(trust: &TlsTrust) -> Result<RootCertStore, TlsError> {
    let mut store = RootCertStore::empty();

    if !trust.exclusive {
        if trust.native_roots {
            let result = rustls_native_certs::load_native_certs();
            let (_added, _ignored) = store.add_parsable_certificates(result.certs);
            for err in result.errors {
                tracing::warn!(error = %err, "TLS: error loading a native root (continuing)");
            }
        }
        if trust.webpki_roots {
            store
                .roots
                .extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        }
    }

    for path in &trust.extra_roots {
        add_pem_file_certs(&mut store, path)?;
    }
    for path in &trust.extra_intermediates {
        add_pem_file_certs(&mut store, path)?;
    }

    if store.is_empty() {
        return Err(TlsError::EmptyTrustStore);
    }
    Ok(store)
}

/// Build a rustls [`ClientConfig`] from a [`TlsConfigSource`].
///
/// Uses an explicit aws-lc-rs crypto provider, safe default protocol versions,
/// and no client authentication. See the module docs for why the provider is
/// explicit.
///
/// # Errors
///
/// Propagates [`build_root_store`] errors, and [`TlsError::Build`] if rustls
/// rejects the protocol-version selection.
pub fn build_client_config(source: TlsConfigSource) -> Result<Arc<ClientConfig>, TlsError> {
    match source {
        TlsConfigSource::Explicit(cfg) => Ok(cfg),
        TlsConfigSource::Trust(trust) => {
            let roots = build_root_store(&trust)?;
            let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
            let cfg = ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| TlsError::Build(e.to_string()))?
                .with_root_certificates(roots)
                .with_no_client_auth();
            Ok(Arc::new(cfg))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Generate `n` independent self-signed CA cert PEMs, concatenated.
    fn gen_ca_bundle(n: usize) -> String {
        let mut bundle = String::new();
        for i in 0..n {
            let cert = rcgen::generate_simple_self_signed(vec![format!("ca-{i}.test")])
                .expect("rcgen self-signed");
            bundle.push_str(&cert.cert.pem());
        }
        bundle
    }

    fn write_temp(contents: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        f.write_all(contents.as_bytes()).expect("write");
        f.flush().expect("flush");
        f
    }

    #[test]
    fn add_pem_counts_multi_cert_bundle() {
        let f = write_temp(&gen_ca_bundle(3));
        let mut store = RootCertStore::empty();
        let added = add_pem_file_certs(&mut store, f.path()).unwrap();
        assert_eq!(added, 3, "all three certs in the bundle are added");
    }

    #[test]
    fn add_pem_is_lenient_with_junk_plus_valid() {
        let mut contents = String::from("this is not a PEM block\ngarbage line\n");
        contents.push_str(&gen_ca_bundle(1));
        contents.push_str("\ntrailing junk\n");
        let f = write_temp(&contents);
        let mut store = RootCertStore::empty();
        let added = add_pem_file_certs(&mut store, f.path()).unwrap();
        assert_eq!(added, 1, "junk is skipped, the valid cert still loads");
    }

    #[test]
    fn add_pem_zero_certs_is_error() {
        let f = write_temp("no certificates here at all\n");
        let mut store = RootCertStore::empty();
        let err = add_pem_file_certs(&mut store, f.path()).unwrap_err();
        assert!(matches!(err, TlsError::NoCertsFound { .. }));
    }

    #[test]
    fn add_pem_unreadable_path_is_error() {
        let mut store = RootCertStore::empty();
        let err = add_pem_file_certs(&mut store, Path::new("/nonexistent/nope.pem")).unwrap_err();
        assert!(matches!(err, TlsError::PemRead { .. }));
    }

    #[test]
    fn build_store_augments_native_with_extra() {
        let f = write_temp(&gen_ca_bundle(1));
        let trust = TlsTrust {
            native_roots: true,
            webpki_roots: false,
            extra_roots: vec![f.path().to_path_buf()],
            extra_intermediates: Vec::new(),
            exclusive: false,
        };
        let store = build_root_store(&trust).unwrap();
        // Native roots vary by host, but the store must be non-empty and at
        // least contain our extra cert.
        assert!(!store.is_empty());
    }

    #[test]
    fn build_store_exclusive_uses_only_extra() {
        let f = write_temp(&gen_ca_bundle(2));
        let trust = TlsTrust::private_ca(f.path());
        let store = build_root_store(&trust).unwrap();
        assert_eq!(
            store.roots.len(),
            2,
            "exclusive store holds exactly the private-CA certs, no native roots"
        );
    }

    #[test]
    fn build_store_exclusive_with_no_files_is_error() {
        let trust = TlsTrust {
            native_roots: true, // ignored under exclusive
            webpki_roots: true, // ignored under exclusive
            extra_roots: Vec::new(),
            extra_intermediates: Vec::new(),
            exclusive: true,
        };
        let err = build_root_store(&trust).unwrap_err();
        assert!(matches!(err, TlsError::EmptyTrustStore));
    }

    #[test]
    fn build_store_no_sources_is_empty_error() {
        let trust = TlsTrust {
            native_roots: false,
            webpki_roots: false,
            extra_roots: Vec::new(),
            extra_intermediates: Vec::new(),
            exclusive: false,
        };
        let err = build_root_store(&trust).unwrap_err();
        assert!(matches!(err, TlsError::EmptyTrustStore));
    }

    #[test]
    fn build_client_config_from_private_ca() {
        let f = write_temp(&gen_ca_bundle(1));
        let cfg =
            build_client_config(TlsConfigSource::Trust(TlsTrust::private_ca(f.path()))).unwrap();
        // A usable client config was produced (no client auth, has roots).
        assert!(Arc::strong_count(&cfg) >= 1);
    }
}
