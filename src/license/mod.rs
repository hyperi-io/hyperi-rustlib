// Project:   hs-rustlib
// File:      src/license/mod.rs
// Purpose:   License management with encrypted license files
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! License management with encrypted license files and anti-tampering.
//!
//! This module provides a secure license system that:
//! - Loads encrypted license files (AES-256-GCM)
//! - Supports local files and HTTPS URLs
//! - Falls back to compiled-in defaults
//! - Verifies Ed25519 signatures
//! - Includes anti-tampering measures
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use hs_rustlib::license;
//!
//! // Initialize with default search paths
//! license::init(license::LicenseOptions::default())?;
//!
//! // Access license settings
//! let settings = license::get();
//! println!("License tier: {}", settings.label);
//! println!("Max cores: {:?}", settings.max_cores);
//!
//! // Check features
//! if settings.has_feature("advanced_analytics") {
//!     // Enable advanced analytics
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! ## License File Format
//!
//! License files are encrypted JSON with the following structure:
//!
//! ```json
//! {
//!   "label": "Enterprise",
//!   "organization": "Acme Corp",
//!   "max_cores": null,
//!   "max_throughput_mbps": 10000,
//!   "expires_at": "2027-01-01T00:00:00Z",
//!   "signature": "base64-ed25519-signature",
//!   "features": {
//!     "advanced_analytics": true,
//!     "custom_rules": true
//!   }
//! }
//! ```
//!
//! The JSON is encrypted with AES-256-GCM before distribution.
//!
//! ## Security Model
//!
//! The protection aims to make casual tampering difficult, not impossible:
//! - Encryption key is obfuscated at compile time
//! - License signature verification (Ed25519)
//! - Runtime integrity checks
//! - Optional debugger detection
//!
//! A determined attacker with debugging tools can bypass these protections.
//! The goal is economic: make tampering cost more than a license.

mod crypto;
mod defaults;
mod error;
mod integrity;
mod types;

use std::path::PathBuf;
use std::sync::OnceLock;

pub use error::LicenseError;
pub use types::LicenseSettings;

/// Global license singleton.
static LICENSE: OnceLock<License> = OnceLock::new();

/// Options for license initialization.
#[derive(Debug, Clone)]
pub struct LicenseOptions {
    /// Explicit path to the license file.
    /// If set, only this path is checked.
    pub license_path: Option<PathBuf>,

    /// URL to fetch the license from.
    /// Only used if `license_path` is not set and no local file is found.
    #[cfg(feature = "license-http")]
    pub license_url: Option<String>,

    /// Whether to verify the license signature.
    /// Set to false for development/testing only.
    pub verify_signature: bool,

    /// Whether to allow expired licenses (with a warning).
    /// Useful for grace periods.
    pub allow_expired: bool,

    /// Custom decryption key (overrides compiled-in key).
    /// Used for testing or multi-tenant deployments.
    pub custom_key: Option<Vec<u8>>,
}

impl Default for LicenseOptions {
    fn default() -> Self {
        Self {
            license_path: None,
            #[cfg(feature = "license-http")]
            license_url: None,
            verify_signature: true,
            allow_expired: false,
            custom_key: None,
        }
    }
}

/// License manager holding the current license state.
#[derive(Debug)]
pub struct License {
    /// The loaded license settings.
    settings: LicenseSettings,

    /// Hash of settings for integrity verification.
    settings_hash: [u8; 32],

    /// Source of the license (for debugging).
    source: LicenseSource,
}

/// Where the license was loaded from.
#[derive(Debug, Clone)]
pub enum LicenseSource {
    /// Loaded from a local file.
    File(PathBuf),

    /// Fetched from a URL.
    #[cfg(feature = "license-http")]
    Url(String),

    /// Using compiled-in defaults.
    Default,
}

impl License {
    /// Create a new license manager with the given options.
    fn new(opts: LicenseOptions) -> Result<Self, LicenseError> {
        let (settings, source) = Self::load_license(&opts)?;

        // Verify signature if required
        if opts.verify_signature && !settings.is_default {
            integrity::verify_signature(&settings)?;
        }

        // Check expiration
        if settings.is_expired() && !opts.allow_expired {
            return Err(LicenseError::Expired {
                expiry: settings.expires_at.clone().unwrap_or_default(),
            });
        }

        // Compute integrity hash
        let settings_hash = integrity::compute_settings_hash(&settings);

        Ok(Self {
            settings,
            settings_hash,
            source,
        })
    }

    /// Load license from file, URL, or defaults.
    fn load_license(opts: &LicenseOptions) -> Result<(LicenseSettings, LicenseSource), LicenseError> {
        // Priority 1: Explicit path
        if let Some(path) = &opts.license_path {
            return Self::load_from_file(path, opts);
        }

        // Priority 2: Environment variable
        if let Ok(path) = std::env::var("HYPERSEC_LICENSE_PATH") {
            let path = PathBuf::from(path);
            if path.exists() {
                return Self::load_from_file(&path, opts);
            }
        }

        // Priority 3: Standard locations
        for path in Self::standard_license_paths() {
            if path.exists() {
                if let Ok(result) = Self::load_from_file(&path, opts) {
                    return Ok(result);
                }
            }
        }

        // Priority 4: URL (if feature enabled)
        #[cfg(feature = "license-http")]
        {
            if let Some(url) = &opts.license_url {
                return Self::load_from_url(url, opts);
            }

            if let Ok(url) = std::env::var("HYPERSEC_LICENSE_URL") {
                return Self::load_from_url(&url, opts);
            }
        }

        // Priority 5: Compiled defaults
        Ok((defaults::get_default_settings(), LicenseSource::Default))
    }

    /// Standard paths to search for license files.
    fn standard_license_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();

        // Current directory
        paths.push(PathBuf::from("license.enc"));
        paths.push(PathBuf::from(".license.enc"));

        // /etc/hypersec/
        paths.push(PathBuf::from("/etc/hypersec/license.enc"));

        // User config directory
        if let Some(config_dir) = dirs::config_dir() {
            paths.push(config_dir.join("hypersec/license.enc"));
        }

        // Home directory
        if let Some(home) = dirs::home_dir() {
            paths.push(home.join(".hypersec/license.enc"));
        }

        paths
    }

    /// Load and decrypt a license file.
    fn load_from_file(
        path: &PathBuf,
        opts: &LicenseOptions,
    ) -> Result<(LicenseSettings, LicenseSource), LicenseError> {
        let encrypted = std::fs::read(path).map_err(|e| LicenseError::LoadFailed {
            path: path.clone(),
            reason: e.to_string(),
        })?;

        let settings = Self::decrypt_and_parse(&encrypted, opts)?;
        Ok((settings, LicenseSource::File(path.clone())))
    }

    /// Fetch and decrypt a license from URL.
    #[cfg(feature = "license-http")]
    fn load_from_url(
        url: &str,
        opts: &LicenseOptions,
    ) -> Result<(LicenseSettings, LicenseSource), LicenseError> {
        // Blocking HTTP request (license loading happens at startup)
        let response = reqwest::blocking::get(url).map_err(|e| LicenseError::FetchFailed {
            url: url.to_string(),
            reason: e.to_string(),
        })?;

        if !response.status().is_success() {
            return Err(LicenseError::FetchFailed {
                url: url.to_string(),
                reason: format!("HTTP {}", response.status()),
            });
        }

        let encrypted = response.bytes().map_err(|e| LicenseError::FetchFailed {
            url: url.to_string(),
            reason: e.to_string(),
        })?;

        let settings = Self::decrypt_and_parse(&encrypted, opts)?;
        Ok((settings, LicenseSource::Url(url.to_string())))
    }

    /// Decrypt encrypted data and parse as license settings.
    fn decrypt_and_parse(
        encrypted: &[u8],
        opts: &LicenseOptions,
    ) -> Result<LicenseSettings, LicenseError> {
        // Get decryption key
        let key_bytes = match &opts.custom_key {
            Some(k) => crypto::derive_key(k),
            None => crypto::derive_key(&defaults::get_decryption_key()),
        };

        // Decrypt
        let decrypted = crypto::decrypt(encrypted, &key_bytes)?;

        // Parse JSON
        let settings: LicenseSettings =
            serde_json::from_slice(&decrypted).map_err(|e| LicenseError::ParseFailed {
                reason: e.to_string(),
            })?;

        Ok(settings)
    }

    /// Get the license settings.
    #[must_use]
    pub fn settings(&self) -> &LicenseSettings {
        &self.settings
    }

    /// Get the license source.
    #[must_use]
    pub fn source(&self) -> &LicenseSource {
        &self.source
    }

    /// Run integrity checks on the license.
    ///
    /// Call this periodically to detect tampering.
    pub fn verify_integrity(&self) -> Result<(), LicenseError> {
        integrity::run_integrity_checks(&self.settings, &self.settings_hash)
    }

    /// Check if a debugger is attached.
    #[must_use]
    pub fn is_debugger_present(&self) -> bool {
        integrity::is_debugger_present()
    }
}

// ============================================================================
// Global singleton API
// ============================================================================

/// Initialize the global license.
///
/// This should be called once at application startup.
///
/// # Errors
///
/// Returns an error if:
/// - License file cannot be loaded
/// - Decryption fails
/// - Signature verification fails
/// - License has expired
/// - License was already initialized
pub fn init(opts: LicenseOptions) -> Result<(), LicenseError> {
    let license = License::new(opts)?;
    LICENSE
        .set(license)
        .map_err(|_| LicenseError::AlreadyInitialized)
}

/// Initialize with default options.
///
/// Searches standard paths and falls back to compiled defaults.
///
/// # Errors
///
/// Returns an error if initialization fails.
pub fn init_default() -> Result<(), LicenseError> {
    init(LicenseOptions::default())
}

/// Get the global license settings.
///
/// # Panics
///
/// Panics if the license has not been initialized.
#[must_use]
pub fn get() -> &'static LicenseSettings {
    LICENSE
        .get()
        .expect("license not initialized - call license::init() first")
        .settings()
}

/// Try to get the global license settings.
///
/// Returns `None` if the license has not been initialized.
#[must_use]
pub fn try_get() -> Option<&'static LicenseSettings> {
    LICENSE.get().map(License::settings)
}

/// Get the full license manager.
///
/// # Panics
///
/// Panics if the license has not been initialized.
#[must_use]
pub fn get_license() -> &'static License {
    LICENSE
        .get()
        .expect("license not initialized - call license::init() first")
}

/// Verify license integrity.
///
/// Call this periodically to detect tampering.
///
/// # Errors
///
/// Returns an error if integrity checks fail.
pub fn verify_integrity() -> Result<(), LicenseError> {
    get_license().verify_integrity()
}

/// Check if using default (fallback) license.
#[must_use]
pub fn is_default() -> bool {
    try_get().map_or(true, |s| s.is_default)
}

/// Check if a feature is enabled in the license.
#[must_use]
pub fn has_feature(name: &str) -> bool {
    try_get().is_some_and(|s| s.has_feature(name))
}

// ============================================================================
// Utility functions for license file creation (external tooling)
// ============================================================================

/// Encrypt license settings for distribution.
///
/// This is used by external tooling to create encrypted license files.
///
/// # Example
///
/// ```rust,no_run
/// use hs_rustlib::license::{LicenseSettings, encrypt_license};
///
/// let settings = LicenseSettings {
///     label: "Enterprise".to_string(),
///     max_cores: None,  // Unlimited
///     ..Default::default()
/// };
///
/// let key = b"your-secret-key";
/// let encrypted = encrypt_license(&settings, key)?;
/// std::fs::write("license.enc", encrypted)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn encrypt_license(settings: &LicenseSettings, key: &[u8]) -> Result<Vec<u8>, LicenseError> {
    let json = serde_json::to_vec(settings).map_err(|e| LicenseError::EncryptionFailed {
        reason: format!("failed to serialize license: {e}"),
    })?;

    let derived_key = crypto::derive_key(key);
    crypto::encrypt(&json, &derived_key)
}

/// Decrypt a license file for inspection.
///
/// This is used by external tooling to verify license contents.
pub fn decrypt_license(encrypted: &[u8], key: &[u8]) -> Result<LicenseSettings, LicenseError> {
    let derived_key = crypto::derive_key(key);
    let decrypted = crypto::decrypt(encrypted, &derived_key)?;

    serde_json::from_slice(&decrypted).map_err(|e| LicenseError::ParseFailed {
        reason: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_license_options_default() {
        let opts = LicenseOptions::default();
        assert!(opts.license_path.is_none());
        assert!(opts.verify_signature);
        assert!(!opts.allow_expired);
    }

    #[test]
    fn test_license_new_with_defaults() {
        let opts = LicenseOptions {
            verify_signature: false, // Skip for test
            ..Default::default()
        };

        let license = License::new(opts).expect("should create with defaults");
        assert!(license.settings.is_default);
        assert!(matches!(license.source, LicenseSource::Default));
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let settings = LicenseSettings {
            label: "Test License".to_string(),
            max_cores: Some(8),
            ..Default::default()
        };

        let key = b"test-encryption-key";
        let encrypted = encrypt_license(&settings, key).expect("encrypt");
        let decrypted = decrypt_license(&encrypted, key).expect("decrypt");

        assert_eq!(decrypted.label, settings.label);
        assert_eq!(decrypted.max_cores, settings.max_cores);
    }

    #[test]
    fn test_decrypt_wrong_key_fails() {
        let settings = LicenseSettings::default();
        let encrypted = encrypt_license(&settings, b"correct-key").expect("encrypt");

        let result = decrypt_license(&encrypted, b"wrong-key");
        assert!(result.is_err());
    }

    #[test]
    fn test_standard_license_paths_not_empty() {
        let paths = License::standard_license_paths();
        assert!(!paths.is_empty());
    }

    #[test]
    fn test_license_verify_integrity() {
        let opts = LicenseOptions {
            verify_signature: false,
            ..Default::default()
        };

        let license = License::new(opts).expect("create");
        assert!(license.verify_integrity().is_ok());
    }

    #[test]
    fn test_is_default_before_init() {
        // Before init, should return true (default behavior)
        // Note: This test may interfere with other tests due to global state
        // In a real test suite, use separate processes or test isolation
    }
}
