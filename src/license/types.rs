// Project:   hs-rustlib
// File:      src/license/types.rs
// Purpose:   License data structures
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! License data types and structures.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// License settings loaded from an encrypted license file.
///
/// This structure contains all licensable parameters that can be
/// dynamically configured via the license system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseSettings {
    /// Human-readable license tier label (e.g., "Community", "Enterprise").
    pub label: String,

    /// Organization name the license is issued to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,

    /// Maximum CPU cores allowed (None = unlimited).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cores: Option<u32>,

    /// Maximum memory in GB (None = unlimited).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_memory_gb: Option<u32>,

    /// Maximum aggregate throughput in Mbps (None = unlimited).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_throughput_mbps: Option<u64>,

    /// Maximum per-container throughput in Mbps (None = unlimited).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_container_throughput_mbps: Option<u64>,

    /// Maximum number of nodes/instances (None = unlimited).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_nodes: Option<u32>,

    /// License expiration timestamp (ISO 8601 format).
    /// None means the license never expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,

    /// License issuance timestamp (ISO 8601 format).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issued_at: Option<String>,

    /// Ed25519 signature over the license data (base64 encoded).
    /// Used to verify the license was issued by HyperSec.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,

    /// Feature flags - extensible key-value pairs for feature gating.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub features: HashMap<String, serde_json::Value>,

    /// Whether this is a default/fallback license (not loaded from file).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_default: bool,
}

impl LicenseSettings {
    /// Check if a feature is enabled.
    ///
    /// Returns `true` if the feature exists and is truthy.
    #[must_use]
    pub fn has_feature(&self, name: &str) -> bool {
        self.features
            .get(name)
            .is_some_and(|v| v.as_bool().unwrap_or(false))
    }

    /// Get a feature value as a string.
    #[must_use]
    pub fn feature_string(&self, name: &str) -> Option<&str> {
        self.features.get(name).and_then(|v| v.as_str())
    }

    /// Get a feature value as an integer.
    #[must_use]
    pub fn feature_int(&self, name: &str) -> Option<i64> {
        self.features.get(name).and_then(|v| v.as_i64())
    }

    /// Check if the license has expired.
    ///
    /// Returns `false` if there's no expiration date.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        let Some(expires_at) = &self.expires_at else {
            return false;
        };

        // Parse ISO 8601 timestamp and compare with current time
        // Using a simple string comparison works for ISO 8601 format
        let now = chrono_lite_now();
        expires_at.as_str() < now.as_str()
    }

    /// Check if this is an unlimited license (enterprise tier).
    #[must_use]
    pub fn is_unlimited(&self) -> bool {
        self.max_cores.is_none()
            && self.max_throughput_mbps.is_none()
            && self.max_nodes.is_none()
    }

    /// Get the effective core limit, with a fallback.
    #[must_use]
    pub fn effective_cores(&self, system_cores: u32) -> u32 {
        self.max_cores.unwrap_or(system_cores)
    }

    /// Get the effective throughput limit in Mbps, with a fallback.
    #[must_use]
    pub fn effective_throughput_mbps(&self, default: u64) -> u64 {
        self.max_throughput_mbps.unwrap_or(default)
    }
}

impl Default for LicenseSettings {
    fn default() -> Self {
        super::defaults::get_default_settings()
    }
}

/// Get current UTC timestamp in ISO 8601 format.
///
/// This is a minimal implementation to avoid pulling in chrono.
fn chrono_lite_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();

    let secs = duration.as_secs();

    // Convert to date components (simplified, doesn't handle leap seconds)
    let days = secs / 86400;
    let time_secs = secs % 86400;

    let hours = time_secs / 3600;
    let mins = (time_secs % 3600) / 60;
    let secs = time_secs % 60;

    // Calculate year, month, day from days since epoch
    // This is a simplified calculation
    let mut year = 1970i32;
    #[allow(clippy::cast_possible_truncation)]
    let mut remaining_days = days as i32; // Safe: we're in year range

    loop {
        let days_in_year = if is_leap_year(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let (month, day) = days_to_month_day(remaining_days, is_leap_year(year));

    format!(
        "{year:04}-{month:02}-{day:02}T{hours:02}:{mins:02}:{secs:02}Z"
    )
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn days_to_month_day(day_of_year: i32, leap: bool) -> (i32, i32) {
    let days_in_months: [i32; 12] = if leap {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut remaining = day_of_year;
    for (i, &days) in days_in_months.iter().enumerate() {
        if remaining < days {
            // Safe: i is at most 11 (12 months)
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            let month = i as i32 + 1;
            return (month, remaining + 1);
        }
        remaining -= days;
    }

    (12, 31) // Fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_license_settings_default() {
        let settings = LicenseSettings::default();
        assert_eq!(settings.label, "Community");
        assert!(settings.is_default);
    }

    #[test]
    fn test_has_feature() {
        let mut settings = LicenseSettings::default();
        settings
            .features
            .insert("test_feature".to_string(), serde_json::json!(true));
        settings
            .features
            .insert("disabled_feature".to_string(), serde_json::json!(false));

        assert!(settings.has_feature("test_feature"));
        assert!(!settings.has_feature("disabled_feature"));
        assert!(!settings.has_feature("nonexistent"));
    }

    #[test]
    fn test_is_expired_no_expiry() {
        let settings = LicenseSettings::default();
        assert!(!settings.is_expired());
    }

    #[test]
    fn test_is_expired_future_date() {
        let mut settings = LicenseSettings::default();
        settings.expires_at = Some("2099-12-31T23:59:59Z".to_string());
        assert!(!settings.is_expired());
    }

    #[test]
    fn test_is_expired_past_date() {
        let mut settings = LicenseSettings::default();
        settings.expires_at = Some("2020-01-01T00:00:00Z".to_string());
        assert!(settings.is_expired());
    }

    #[test]
    fn test_is_unlimited() {
        let mut settings = LicenseSettings::default();

        // Default has limits
        assert!(!settings.is_unlimited());

        // Remove all limits
        settings.max_cores = None;
        settings.max_throughput_mbps = None;
        settings.max_nodes = None;
        assert!(settings.is_unlimited());
    }

    #[test]
    fn test_effective_cores() {
        let mut settings = LicenseSettings::default();
        settings.max_cores = Some(4);

        assert_eq!(settings.effective_cores(16), 4);

        settings.max_cores = None;
        assert_eq!(settings.effective_cores(16), 16);
    }

    #[test]
    fn test_chrono_lite_now_format() {
        let now = chrono_lite_now();
        // Should be in ISO 8601 format: YYYY-MM-DDTHH:MM:SSZ
        assert!(now.contains('T'));
        assert!(now.ends_with('Z'));
        assert_eq!(now.len(), 20);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let settings = LicenseSettings::default();
        let json = serde_json::to_string(&settings).expect("serialize");
        let parsed: LicenseSettings = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(settings.label, parsed.label);
        assert_eq!(settings.max_cores, parsed.max_cores);
    }
}
