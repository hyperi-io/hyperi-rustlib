// Project:   hs-rustlib
// File:      src/license/integrity.rs
// Purpose:   Anti-tampering and integrity verification
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Integrity verification and anti-tampering measures.
//!
//! This module provides:
//! - License signature verification (Ed25519)
//! - Runtime integrity checks
//! - Basic anti-debugging detection (optional)
//!
//! # Security Model
//!
//! These protections are designed to deter casual tampering and make
//! reverse engineering more time-consuming. They are NOT cryptographically
//! unbreakable - a determined attacker with debugging tools can bypass them.
//!
//! The goal is economic: make tampering cost more than a license.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use obfstr::obfstr;
use sha2::{Digest, Sha256};

use super::error::LicenseError;
use super::types::LicenseSettings;

/// The Ed25519 public key for verifying license signatures.
///
/// This key is obfuscated at compile time. The corresponding private key
/// is used by the license generation tool (external to this crate).
///
/// # Production Deployment
///
/// Replace this with your actual public key before deployment.
/// Generate a keypair with: `openssl genpkey -algorithm ed25519`
#[inline(never)]
fn get_public_key_bytes() -> [u8; 32] {
    // Obfuscated placeholder public key - REPLACE FOR PRODUCTION
    // This is a dummy key for development/testing
    // The obfstr! macro returns a temporary, so we need to convert immediately
    let key_b64_str = obfstr!("MCowBQYDK2VwAyEAPlaceholderKeyForDevReplace=").to_string();

    // Decode base64 and extract the 32-byte key
    // Ed25519 public keys in SPKI format have a 12-byte header
    let decoded = BASE64
        .decode(key_b64_str.as_bytes())
        .unwrap_or_else(|_| vec![0u8; 44]);

    if decoded.len() >= 44 {
        // SPKI format: skip 12-byte header
        let mut key = [0u8; 32];
        key.copy_from_slice(&decoded[12..44]);
        key
    } else if decoded.len() == 32 {
        // Raw 32-byte key
        let mut key = [0u8; 32];
        key.copy_from_slice(&decoded);
        key
    } else {
        // Fallback to zeros (will fail verification)
        [0u8; 32]
    }
}

/// Verify the Ed25519 signature on a license.
///
/// The signature is computed over the canonical JSON representation
/// of the license (excluding the signature field itself).
///
/// # Errors
///
/// Returns an error if:
/// - The license has no signature
/// - The signature is malformed
/// - The signature doesn't match the license data
pub(crate) fn verify_signature(settings: &LicenseSettings) -> Result<(), LicenseError> {
    let Some(sig_b64) = &settings.signature else {
        // No signature - allow for default/test licenses
        if settings.is_default {
            return Ok(());
        }
        return Err(LicenseError::SignatureInvalid {
            reason: "license has no signature".into(),
        });
    };

    // Decode the signature
    let sig_bytes = BASE64
        .decode(sig_b64)
        .map_err(|e| LicenseError::SignatureInvalid {
            reason: format!("invalid signature encoding: {e}"),
        })?;

    let signature =
        Signature::from_slice(&sig_bytes).map_err(|e| LicenseError::SignatureInvalid {
            reason: format!("invalid signature format: {e}"),
        })?;

    // Get the public key
    let pk_bytes = get_public_key_bytes();
    let public_key =
        VerifyingKey::from_bytes(&pk_bytes).map_err(|e| LicenseError::SignatureInvalid {
            reason: format!("invalid public key: {e}"),
        })?;

    // Create canonical message (license JSON without signature)
    let message = create_signing_message(settings)?;

    // Verify
    public_key
        .verify(message.as_bytes(), &signature)
        .map_err(|_| LicenseError::SignatureInvalid {
            reason: "signature verification failed".into(),
        })
}

/// Create the canonical message for signing/verification.
///
/// This is the JSON representation of the license with the signature
/// field removed, ensuring deterministic serialization.
fn create_signing_message(settings: &LicenseSettings) -> Result<String, LicenseError> {
    // Clone and remove signature for canonical representation
    let mut signing_data = settings.clone();
    signing_data.signature = None;

    serde_json::to_string(&signing_data).map_err(|e| LicenseError::SignatureInvalid {
        reason: format!("failed to serialize license: {e}"),
    })
}

/// Compute a hash of critical data for integrity checking.
///
/// This can be used to detect if license settings have been modified
/// in memory after loading.
#[must_use]
pub fn compute_settings_hash(settings: &LicenseSettings) -> [u8; 32] {
    let mut hasher = Sha256::new();

    // Hash critical fields
    hasher.update(settings.label.as_bytes());
    hasher.update(settings.max_cores.unwrap_or(0).to_le_bytes());
    hasher.update(settings.max_throughput_mbps.unwrap_or(0).to_le_bytes());
    hasher.update(settings.max_nodes.unwrap_or(0).to_le_bytes());

    if let Some(expires) = &settings.expires_at {
        hasher.update(expires.as_bytes());
    }

    hasher.finalize().into()
}

/// Verify that settings haven't been tampered with in memory.
///
/// Compare the current hash against a previously computed hash.
#[must_use]
pub fn verify_settings_integrity(settings: &LicenseSettings, expected_hash: &[u8; 32]) -> bool {
    let current_hash = compute_settings_hash(settings);

    // Constant-time comparison to prevent timing attacks
    constant_time_compare(&current_hash, expected_hash)
}

/// Constant-time comparison of two byte slices.
///
/// Prevents timing side-channel attacks.
#[inline(never)]
fn constant_time_compare(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }

    result == 0
}

/// Basic environment check for debugging.
///
/// Returns true if a debugger might be attached.
/// This is a best-effort detection - sophisticated debuggers can evade it.
#[cfg(target_os = "linux")]
#[must_use]
pub fn is_debugger_present() -> bool {
    // Check /proc/self/status for TracerPid
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if let Some(tracer) = line.strip_prefix("TracerPid:") {
                let pid: i32 = tracer.trim().parse().unwrap_or(0);
                if pid != 0 {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn is_debugger_present() -> bool {
    // No detection on non-Linux platforms
    false
}

/// Perform basic integrity checks.
///
/// Call this periodically to detect tampering.
/// Returns `Ok(())` if all checks pass.
pub fn run_integrity_checks(
    settings: &LicenseSettings,
    expected_hash: &[u8; 32],
) -> Result<(), LicenseError> {
    // Check 1: Settings hash
    if !verify_settings_integrity(settings, expected_hash) {
        return Err(LicenseError::IntegrityFailed {
            reason: "license settings modified in memory".into(),
        });
    }

    // Check 2: Expiration (in case system clock was manipulated)
    if settings.is_expired() {
        return Err(LicenseError::Expired {
            expiry: settings.expires_at.clone().unwrap_or_default(),
        });
    }

    // Check 3: Debugger detection (warning only, doesn't fail)
    // Uncomment if you want to fail on debugger detection:
    // if is_debugger_present() {
    //     return Err(LicenseError::IntegrityFailed {
    //         reason: "debugging detected".into(),
    //     });
    // }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_settings_hash_deterministic() {
        let settings = LicenseSettings::default();
        let hash1 = compute_settings_hash(&settings);
        let hash2 = compute_settings_hash(&settings);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_compute_settings_hash_changes_with_data() {
        let mut settings1 = LicenseSettings::default();
        let mut settings2 = LicenseSettings::default();
        settings2.max_cores = Some(99);

        let hash1 = compute_settings_hash(&settings1);
        let hash2 = compute_settings_hash(&settings2);
        assert_ne!(hash1, hash2);

        // Also test label change
        settings1.label = "Modified".to_string();
        let hash3 = compute_settings_hash(&settings1);
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn test_verify_settings_integrity_valid() {
        let settings = LicenseSettings::default();
        let hash = compute_settings_hash(&settings);
        assert!(verify_settings_integrity(&settings, &hash));
    }

    #[test]
    fn test_verify_settings_integrity_tampered() {
        let mut settings = LicenseSettings::default();
        let hash = compute_settings_hash(&settings);

        // Tamper with settings
        settings.max_cores = Some(9999);

        assert!(!verify_settings_integrity(&settings, &hash));
    }

    #[test]
    fn test_constant_time_compare_equal() {
        let a = [1u8, 2, 3, 4];
        let b = [1u8, 2, 3, 4];
        assert!(constant_time_compare(&a, &b));
    }

    #[test]
    fn test_constant_time_compare_not_equal() {
        let a = [1u8, 2, 3, 4];
        let b = [1u8, 2, 3, 5];
        assert!(!constant_time_compare(&a, &b));
    }

    #[test]
    fn test_constant_time_compare_different_length() {
        let a = [1u8, 2, 3];
        let b = [1u8, 2, 3, 4];
        assert!(!constant_time_compare(&a, &b));
    }

    #[test]
    fn test_run_integrity_checks_valid() {
        let settings = LicenseSettings::default();
        let hash = compute_settings_hash(&settings);
        assert!(run_integrity_checks(&settings, &hash).is_ok());
    }

    #[test]
    fn test_run_integrity_checks_tampered() {
        let mut settings = LicenseSettings::default();
        let hash = compute_settings_hash(&settings);

        settings.max_cores = Some(9999);

        let result = run_integrity_checks(&settings, &hash);
        assert!(result.is_err());
    }

    #[test]
    fn test_run_integrity_checks_expired() {
        let mut settings = LicenseSettings::default();
        settings.expires_at = Some("2020-01-01T00:00:00Z".to_string());
        let hash = compute_settings_hash(&settings);

        let result = run_integrity_checks(&settings, &hash);
        assert!(matches!(result, Err(LicenseError::Expired { .. })));
    }

    #[test]
    fn test_verify_signature_no_signature_default() {
        let settings = LicenseSettings::default();
        // Default licenses without signatures should pass
        assert!(verify_signature(&settings).is_ok());
    }

    #[test]
    fn test_verify_signature_no_signature_non_default() {
        let mut settings = LicenseSettings::default();
        settings.is_default = false;

        let result = verify_signature(&settings);
        assert!(matches!(result, Err(LicenseError::SignatureInvalid { .. })));
    }
}
