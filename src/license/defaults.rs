// Project:   hs-rustlib
// File:      src/license/defaults.rs
// Purpose:   Obfuscated compile-time default license settings
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Compile-time obfuscated default license settings.
//!
//! These defaults are used when no license file is available.
//! All strings are obfuscated at compile time to prevent trivial
//! extraction from the binary.
//!
//! # Security Note
//!
//! The obfuscation here is NOT encryption - it's designed to make
//! casual inspection harder, not to provide cryptographic security.
//! A determined attacker with debugging tools can still extract these.

use obfstr::obfstr;

use super::types::LicenseSettings;

/// The obfuscated encryption key used to decrypt license files.
///
/// This key is XOR-obfuscated at compile time. The actual key value
/// should be changed for production deployments.
///
/// # Security
///
/// - Change this key before production deployment
/// - The key is obfuscated but not truly hidden from determined attackers
/// - Consider using hardware-bound keys for higher security
#[inline(never)]
pub(crate) fn get_decryption_key() -> Vec<u8> {
    // The obfstr! macro encrypts this at compile time and decrypts at runtime
    // Change this value for your deployment!
    obfstr!("hypersec-default-license-key-v1-change-me")
        .as_bytes()
        .to_vec()
}

/// Get the default license settings (Community tier).
///
/// These settings are compiled into the binary and used when:
/// - No license file is found
/// - License file decryption fails
/// - License has expired (fallback)
#[inline(never)]
pub(crate) fn get_default_settings() -> LicenseSettings {
    LicenseSettings {
        // Obfuscated string literals
        label: obfstr!("Community").to_string(),
        organization: None,

        // Resource limits for community tier
        max_cores: Some(4),
        max_memory_gb: Some(8),
        max_throughput_mbps: Some(100),
        max_container_throughput_mbps: Some(25),
        max_nodes: Some(1),

        // No expiry for defaults (perpetual community license)
        expires_at: None,
        issued_at: None,

        // No signature for compiled defaults
        signature: None,

        // Empty feature flags
        features: std::collections::HashMap::new(),

        // Mark as default/fallback
        is_default: true,
    }
}

/// Enterprise defaults (used for testing/development only).
///
/// NOT compiled into production builds - only available in tests.
#[cfg(test)]
pub(crate) fn get_test_enterprise_settings() -> LicenseSettings {
    use std::collections::HashMap;

    let mut features = HashMap::new();
    features.insert("advanced_analytics".to_string(), serde_json::json!(true));
    features.insert("custom_rules".to_string(), serde_json::json!(true));
    features.insert("multi_tenant".to_string(), serde_json::json!(true));

    LicenseSettings {
        label: "Enterprise".to_string(),
        organization: Some("Test Organization".to_string()),
        max_cores: None,           // Unlimited
        max_memory_gb: None,       // Unlimited
        max_throughput_mbps: None, // Unlimited
        max_container_throughput_mbps: None,
        max_nodes: None, // Unlimited
        expires_at: None,
        issued_at: Some("2025-01-01T00:00:00Z".to_string()),
        signature: None,
        features,
        is_default: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings_has_limits() {
        let settings = get_default_settings();

        assert_eq!(settings.label, "Community");
        assert_eq!(settings.max_cores, Some(4));
        assert_eq!(settings.max_throughput_mbps, Some(100));
        assert!(settings.is_default);
    }

    #[test]
    fn test_decryption_key_not_empty() {
        let key = get_decryption_key();
        assert!(!key.is_empty());
        assert!(key.len() >= 16); // At least 128 bits
    }

    #[test]
    fn test_enterprise_settings_unlimited() {
        let settings = get_test_enterprise_settings();

        assert_eq!(settings.label, "Enterprise");
        assert!(settings.max_cores.is_none());
        assert!(settings.max_throughput_mbps.is_none());
        assert!(!settings.is_default);
        assert!(settings.features.contains_key("advanced_analytics"));
    }
}
