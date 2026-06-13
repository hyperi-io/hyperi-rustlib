// Project:   hyperi-rustlib
// File:      src/secrets/crypto.rs
// Purpose:   At-rest encryption helpers for the secrets disk cache
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! At-rest encryption for cached secret payloads.
//!
//! `CacheConfig.encryption_key` (when set) drives AES-256-GCM
//! encryption of every disk-cache entry. The user-supplied key string
//! is passed through HKDF-SHA256 (with a fixed library-domain salt)
//! to produce the 32-byte AES key -- this means the user can supply
//! any-length string, including a passphrase, without us assuming
//! they already have 32 bytes of key material.
//!
//! ## Envelope format
//!
//! Stored on disk as compact JSON:
//!
//! ```json
//! {
//!     "v": 1,
//!     "nonce": "<base64-url-no-pad 12 bytes>",
//!     "ct":    "<base64-url-no-pad ciphertext+tag>"
//! }
//! ```
//!
//! Version 1 is the only format. A future v2 would gain a header byte
//! up front (e.g. AAD-bound version + cipher selector) but until we
//! need to migrate, the JSON envelope is enough.
//!
//! ## Nonce strategy
//!
//! Random 96-bit nonce per entry. The 96-bit space tolerates
//! `~2^32` writes before the collision risk crosses the 2^-32 threshold;
//! a secrets cache writing once per refresh interval (minutes-hours)
//! lives well below this.
//!
//! ## Misuse resistance
//!
//! - Tampering with `ct` or `nonce` breaks GCM auth -> `open()`
//!   returns an error.
//! - Wrong key, wrong AAD -- same path, same error (no oracle).
//! - Truncated nonce / bad base64 -> malformed envelope error.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine;
use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use super::error::{SecretsError, SecretsResult};

/// HKDF salt -- fixed per-library so two services with the same
/// `encryption_key` produce the same derived key (the user's intent
/// is "same key = same cache"). Not secret. 32 bytes of high-entropy
/// constant.
const HKDF_SALT: &[u8] = b"hyperi-rustlib::secrets::cache::v1::hkdf-salt-32bytes!";
const HKDF_INFO: &[u8] = b"hyperi-rustlib secrets disk cache AES-256-GCM key";

const ENVELOPE_VERSION: u8 = 1;
const NONCE_LEN: usize = 12; // 96 bits -- standard for AES-GCM

/// AAD prefix. Domain-separates the cache AAD namespace so a future
/// reuse of `seal`/`open` for a different purpose can't share AAD
/// values with the cache by accident.
const AAD_DOMAIN: &[u8] = b"hyperi-rustlib:secrets-cache:v1:";

/// Compose AAD for a cache slot: domain prefix + cache-key bytes.
pub(super) fn aad_for(cache_key: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(AAD_DOMAIN.len() + cache_key.len());
    v.extend_from_slice(AAD_DOMAIN);
    v.extend_from_slice(cache_key.as_bytes());
    v
}

/// Encrypted cache envelope. JSON-serialised to disk.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct Envelope {
    /// Format version. Currently always `1`.
    pub v: u8,
    /// Base64-url-no-pad encoded 12-byte nonce.
    pub nonce: String,
    /// Base64-url-no-pad encoded ciphertext (includes 16-byte GCM tag).
    pub ct: String,
}

impl Envelope {
    pub fn looks_like(raw: &[u8]) -> bool {
        // Cheap check: starts with `{`, contains the version key.
        // Avoids attempting full JSON parse on legacy plaintext entries.
        raw.first() == Some(&b'{') && raw.windows(4).any(|w| w == br#""v":"#)
    }
}

/// Derive a 32-byte AES-256-GCM key from a user-supplied string.
///
/// HKDF-SHA256 with a fixed library-domain salt + info. The user's
/// `encryption_key` can be any length (passphrase or pre-existing
/// 32-byte key encoded as text); HKDF normalises it.
fn derive_key(user_key: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    // HKDF-Extract + HKDF-Expand. Both calls are infallible for our
    // sizes (32-byte output well below the 8160-byte limit).
    let h = Hkdf::<Sha256>::new(Some(HKDF_SALT), user_key.as_bytes());
    h.expand(HKDF_INFO, &mut out)
        .expect("HKDF expand: 32-byte output within HKDF's 8160-byte ceiling");
    out
}

/// Seal `plaintext` under `user_key`. `aad` (cache-key name) is
/// authenticated, not encrypted; mismatch on open fails auth.
///
/// # Errors
///
/// `SecretsError::CacheError` on encrypt or serialise failure.
pub(super) fn seal(user_key: &str, plaintext: &[u8], aad: &[u8]) -> SecretsResult<String> {
    let key_bytes = derive_key(user_key);
    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);

    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ct = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|e| SecretsError::CacheError(format!("encrypt failed: {e}")))?;

    let envelope = Envelope {
        v: ENVELOPE_VERSION,
        nonce: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(nonce_bytes),
        ct: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&ct),
    };

    serde_json::to_string(&envelope)
        .map_err(|e| SecretsError::CacheError(format!("envelope serialise: {e}")))
}

/// Open an envelope from [`seal`]. Same `aad` required.
///
/// # Errors
///
/// `SecretsError::CacheError` on malformed envelope, wrong version,
/// bad base64, or auth failure (wrong key / AAD / tampered ct).
pub(super) fn open(user_key: &str, envelope_bytes: &[u8], aad: &[u8]) -> SecretsResult<Vec<u8>> {
    let envelope: Envelope = serde_json::from_slice(envelope_bytes)
        .map_err(|e| SecretsError::CacheError(format!("envelope parse: {e}")))?;

    if envelope.v != ENVELOPE_VERSION {
        return Err(SecretsError::CacheError(format!(
            "unsupported envelope version {} (expected {ENVELOPE_VERSION})",
            envelope.v
        )));
    }

    let nonce_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(envelope.nonce.as_bytes())
        .map_err(|e| SecretsError::CacheError(format!("nonce base64: {e}")))?;
    if nonce_bytes.len() != NONCE_LEN {
        return Err(SecretsError::CacheError(format!(
            "nonce length {} (expected {NONCE_LEN})",
            nonce_bytes.len()
        )));
    }
    let ct = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(envelope.ct.as_bytes())
        .map_err(|e| SecretsError::CacheError(format!("ct base64: {e}")))?;

    let key_bytes = derive_key(user_key);
    let key = Key::<Aes256Gcm>::from_slice(&key_bytes);
    let cipher = Aes256Gcm::new(key);
    let nonce = Nonce::from_slice(&nonce_bytes);

    cipher
        .decrypt(
            nonce,
            Payload {
                msg: ct.as_ref(),
                aad,
            },
        )
        .map_err(|_| {
            SecretsError::CacheError(
                "decrypt failed: wrong key, wrong AAD, or tampered data".into(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const AAD: &[u8] = b"test-aad";

    #[test]
    fn round_trip_recovers_plaintext() {
        let key = "operator-provided-passphrase";
        let pt = b"my secret value";
        let env = seal(key, pt, AAD).unwrap();
        let recovered = open(key, env.as_bytes(), AAD).unwrap();
        assert_eq!(recovered, pt);
    }

    #[test]
    fn wrong_key_fails_authentication() {
        let env = seal("right-key", b"data", AAD).unwrap();
        let err = open("wrong-key", env.as_bytes(), AAD).unwrap_err();
        assert!(matches!(err, SecretsError::CacheError(_)));
    }

    #[test]
    fn tampered_ciphertext_fails_authentication() {
        let mut env = seal("k", b"data", AAD).unwrap();
        // Flip a byte in the middle of the ct field.
        let needle = "\"ct\":\"";
        let start = env.find(needle).unwrap() + needle.len();
        let mut bytes = env.into_bytes();
        bytes[start] = if bytes[start] == b'A' { b'B' } else { b'A' };
        env = String::from_utf8(bytes).unwrap();
        let err = open("k", env.as_bytes(), AAD).unwrap_err();
        assert!(matches!(err, SecretsError::CacheError(_)));
    }

    /// AAD prevents cross-slot file swaps.
    #[test]
    fn aad_mismatch_fails_authentication() {
        let env = seal("k", b"db-password", b"db_password").unwrap();
        let err = open("k", env.as_bytes(), b"kafka_password").unwrap_err();
        assert!(matches!(err, SecretsError::CacheError(_)));
    }

    /// Domain-separated AAD. A future caller using the
    /// same cache_key under a different domain MUST fail auth.
    #[test]
    fn aad_domain_separation_blocks_cross_module_reuse() {
        let env = seal("k", b"db-password", &aad_for("db_password")).unwrap();
        // Same cache_key bytes, no domain prefix → must fail.
        let err = open("k", env.as_bytes(), b"db_password").unwrap_err();
        assert!(matches!(err, SecretsError::CacheError(_)));
    }

    #[test]
    fn nonces_are_unique_across_seals() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1000 {
            let env = seal("k", b"same plaintext", AAD).unwrap();
            assert!(seen.insert(env), "duplicate envelope (nonce collision)");
        }
    }

    #[test]
    fn looks_like_envelope_detects_v1_form() {
        let env = seal("k", b"x", AAD).unwrap();
        assert!(Envelope::looks_like(env.as_bytes()));
    }

    #[test]
    fn looks_like_envelope_rejects_legacy_plaintext() {
        let legacy = br#"{"data":"abc","fetched_at_secs":1234,"metadata":{}}"#;
        assert!(!Envelope::looks_like(legacy));
    }

    #[test]
    fn malformed_envelope_returns_error() {
        let err = open("k", b"not json", AAD).unwrap_err();
        assert!(matches!(err, SecretsError::CacheError(_)));
    }
}
