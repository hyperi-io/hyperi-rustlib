// Project:   hs-rustlib
// File:      src/license/crypto.rs
// Purpose:   AES-256-GCM encryption/decryption for license files
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Cryptographic operations for license file encryption/decryption.
//!
//! Uses AES-256-GCM for authenticated encryption. The encryption key is
//! derived from an obfuscated compile-time secret.

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use sha2::{Digest, Sha256};

use super::error::LicenseError;

/// Nonce size for AES-GCM (96 bits / 12 bytes).
pub const NONCE_SIZE: usize = 12;

/// Authentication tag size (128 bits / 16 bytes).
pub const TAG_SIZE: usize = 16;

/// Minimum encrypted payload size (nonce + tag).
pub const MIN_ENCRYPTED_SIZE: usize = NONCE_SIZE + TAG_SIZE;

/// Derive a 256-bit key from the obfuscated secret.
///
/// Uses SHA-256 to derive a fixed-size key from the secret.
/// The secret itself is obfuscated at compile time.
#[inline]
pub(crate) fn derive_key(secret: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(secret);
    hasher.update(b"hs-rustlib-license-v1"); // Domain separation
    hasher.finalize().into()
}

/// Decrypt an AES-256-GCM encrypted payload.
///
/// # Format
///
/// The encrypted data format is:
/// ```text
/// [12-byte nonce][ciphertext][16-byte auth tag]
/// ```
///
/// # Errors
///
/// Returns an error if:
/// - The data is too short (less than nonce + tag)
/// - Decryption fails (wrong key or tampered data)
pub(crate) fn decrypt(encrypted: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, LicenseError> {
    if encrypted.len() < MIN_ENCRYPTED_SIZE {
        return Err(LicenseError::DecryptionFailed {
            reason: "encrypted data too short".into(),
        });
    }

    let (nonce_bytes, ciphertext) = encrypted.split_at(NONCE_SIZE);
    let nonce = Nonce::from_slice(nonce_bytes);

    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| LicenseError::DecryptionFailed {
        reason: format!("invalid key: {e}"),
    })?;

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| LicenseError::DecryptionFailed {
            reason: "decryption failed - invalid key or tampered data".into(),
        })
}

/// Encrypt data using AES-256-GCM.
///
/// This is used for creating license files (external tooling).
/// The nonce is randomly generated and prepended to the ciphertext.
///
/// # Errors
///
/// Returns an error if encryption fails.
#[cfg(feature = "license")]
pub fn encrypt(plaintext: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, LicenseError> {
    use rand::RngCore;

    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| LicenseError::EncryptionFailed {
        reason: format!("invalid key: {e}"),
    })?;

    // Generate random nonce
    let mut nonce_bytes = [0u8; NONCE_SIZE];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| LicenseError::EncryptionFailed {
            reason: format!("encryption failed: {e}"),
        })?;

    // Prepend nonce to ciphertext
    let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_key_deterministic() {
        let key1 = derive_key(b"test-secret");
        let key2 = derive_key(b"test-secret");
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_derive_key_different_inputs() {
        let key1 = derive_key(b"secret-a");
        let key2 = derive_key(b"secret-b");
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = derive_key(b"test-key-for-roundtrip");
        let plaintext = b"Hello, this is a test license!";

        let encrypted = encrypt(plaintext, &key).expect("encryption should succeed");
        let decrypted = decrypt(&encrypted, &key).expect("decryption should succeed");

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_wrong_key_fails() {
        let key1 = derive_key(b"correct-key");
        let key2 = derive_key(b"wrong-key");
        let plaintext = b"secret data";

        let encrypted = encrypt(plaintext, &key1).expect("encryption should succeed");
        let result = decrypt(&encrypted, &key2);

        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_tampered_data_fails() {
        let key = derive_key(b"test-key");
        let plaintext = b"original data";

        let mut encrypted = encrypt(plaintext, &key).expect("encryption should succeed");

        // Tamper with the ciphertext (not the nonce)
        if encrypted.len() > NONCE_SIZE + 5 {
            encrypted[NONCE_SIZE + 5] ^= 0xFF;
        }

        let result = decrypt(&encrypted, &key);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_too_short_fails() {
        let key = derive_key(b"test-key");
        let short_data = vec![0u8; MIN_ENCRYPTED_SIZE - 1];

        let result = decrypt(&short_data, &key);
        assert!(result.is_err());
    }

    #[test]
    fn test_encrypted_data_has_correct_structure() {
        let key = derive_key(b"test-key");
        let plaintext = b"test";

        let encrypted = encrypt(plaintext, &key).expect("encryption should succeed");

        // Should have nonce + ciphertext + tag
        // Ciphertext is same length as plaintext for GCM
        assert!(encrypted.len() >= NONCE_SIZE + plaintext.len() + TAG_SIZE);
    }
}
