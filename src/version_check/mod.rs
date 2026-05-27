// Project:   hyperi-rustlib
// File:      src/version_check/mod.rs
// Purpose:   Startup version check against HyperI version API
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Startup version check.
//!
//! Calls the HyperI version API on startup to check if a newer version is
//! available. The check is non-blocking, fire-and-forget, and gracefully
//! handles all failure modes (network errors, timeouts, bad responses).
//!
//! # Usage
//!
//! ```rust,no_run
//! use hyperi_rustlib::version_check::{VersionCheck, VersionCheckConfig};
//!
//! #[tokio::main]
//! async fn main() {
//!     let checker = VersionCheck::new(VersionCheckConfig {
//!         product: "dfe-loader".into(),
//!         current_version: env!("CARGO_PKG_VERSION").into(),
//!         ..Default::default()
//!     });
//!
//!     // Fire-and-forget -- spawns a background task, never blocks startup
//!     checker.check_on_startup();
//! }
//! ```

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Default version check API endpoint.
const DEFAULT_API_URL: &str = "https://releases.hyperi.io/api/v1/check";

/// Default HTTP timeout for the version check.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Configuration for the startup version check.
///
/// When the `config` feature is enabled, this can be loaded from the config
/// cascade under the `version_check` key:
///
/// ```yaml
/// version_check:
///   api_url: "https://releases.hyperi.io/api/v1/check"
///   timeout_secs: 5
///   disabled: false
/// ```
///
/// `product` and `current_version` are always set programmatically -- they
/// come from the binary, not from config files.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VersionCheckConfig {
    /// Product identifier (e.g., "dfe-loader", "dfe-receiver").
    #[serde(default)]
    pub product: String,
    /// Current version of this product (e.g., "1.8.0").
    #[serde(default)]
    pub current_version: String,
    /// Deployment type (e.g., "k8s", "docker", "bare").
    #[serde(default)]
    pub deployment: Option<String>,
    /// API endpoint URL. Defaults to the HyperI version API.
    #[serde(default = "default_api_url")]
    pub api_url: String,
    /// HTTP request timeout in seconds.
    #[serde(default = "default_timeout", with = "duration_secs")]
    pub timeout: Duration,
    /// Disable the version check entirely.
    #[serde(default)]
    pub disabled: bool,
}

fn default_api_url() -> String {
    DEFAULT_API_URL.into()
}

fn default_timeout() -> Duration {
    DEFAULT_TIMEOUT
}

/// Serde helper to serialise `Duration` as seconds (u64).
mod duration_secs {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(Duration::from_secs(secs))
    }
}

impl Default for VersionCheckConfig {
    fn default() -> Self {
        Self {
            product: String::new(),
            current_version: String::new(),
            deployment: None,
            api_url: default_api_url(),
            timeout: DEFAULT_TIMEOUT,
            disabled: false,
        }
    }
}

impl VersionCheckConfig {
    /// Load from the config cascade, then overlay product/version.
    ///
    /// Reads the `version_check` key from the cascade for `api_url`,
    /// `timeout`, and `disabled`. The `product` and `current_version`
    /// fields are always set from the provided arguments (they come
    /// from the binary, not from config files).
    #[must_use]
    pub fn from_cascade(product: &str, current_version: &str) -> Self {
        let mut config = Self::cascade_base();
        config.product = product.into();
        config.current_version = current_version.into();
        config
    }

    /// Load just the cascade portion (api_url, timeout, disabled).
    fn cascade_base() -> Self {
        #[cfg(feature = "config")]
        {
            if let Some(cfg) = crate::config::try_get()
                && let Ok(vc) = cfg.unmarshal_key_registered::<Self>("version_check")
            {
                return vc;
            }
        }
        Self::default()
    }
}

/// Startup version checker.
///
/// Call [`VersionCheck::check_on_startup`] during application init to spawn
/// a background task that checks for newer versions. The check never blocks
/// the main thread and gracefully handles all errors.
#[derive(Debug, Clone)]
pub struct VersionCheck {
    config: VersionCheckConfig,
}

impl VersionCheck {
    /// Create a new version checker with the given configuration.
    #[must_use]
    pub fn new(config: VersionCheckConfig) -> Self {
        Self { config }
    }

    /// Spawn a background task to check for a newer version.
    ///
    /// This method returns immediately. The check runs asynchronously and
    /// logs the result. Any errors are logged at warn level and swallowed.
    pub fn check_on_startup(&self) {
        if self.config.disabled {
            tracing::debug!("version check disabled");
            return;
        }

        if self.config.product.is_empty() || self.config.current_version.is_empty() {
            tracing::debug!("version check skipped: product or version not set");
            return;
        }

        let config = self.config.clone();
        tokio::spawn(async move {
            match do_version_check(&config).await {
                Ok(resp) => log_version_response(&config, &resp),
                Err(e) => {
                    tracing::warn!(error = %e, "version check failed (non-fatal)");
                }
            }
        });
    }
}

// ============================================================================
// Request / response types
// ============================================================================

/// Payload sent to the version check API.
///
/// Intentionally minimal. Pre-2.7.5 the payload also included:
///   - `instance_id`: a persistent UUID disk-stored in `~/.cache/hyperi/`.
///     Effectively a tracking cookie that survived restarts. Dropped --
///     too aggressive for an OSS library's default behaviour. Operators
///     who want a stable identifier can set one themselves via
///     `VersionCheckConfig` (not currently exposed; can be added if a
///     real need emerges).
///   - `deployment`: free-form string from the operator's config
///     (`"production-east"`, etc.). Operators sometimes embed sensitive
///     names; dropping by default. Field stays on `VersionCheckConfig`
///     for forward-compat but is no longer sent.
///
/// Kept: `product`, `current_version`, `os` (family -- Linux/Darwin/
/// Windows), `arch` (x86_64/aarch64). Enough signal for "which versions
/// are running on which platforms"; zero personal data.
#[derive(Debug, Serialize)]
struct CheckPayload {
    product: String,
    current_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    os: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arch: Option<String>,
}

/// Response from the version check API.
#[derive(Debug, Deserialize)]
pub struct VersionCheckResponse {
    /// Latest available version (e.g., "1.9.0").
    pub latest_version: Option<String>,
    /// Whether an update is available.
    pub update_available: bool,
    /// URL to the release page.
    pub release_url: Option<String>,
    /// When the latest version was published (ISO 8601).
    pub published_at: Option<String>,
    /// Optional message from the server.
    pub message: Option<String>,
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Environment-variable opt-out check. Returns true if the user has
/// disabled telemetry via `HYPERI_TELEMETRY=off|0|false|no`.
fn telemetry_opted_out() -> bool {
    std::env::var("HYPERI_TELEMETRY").is_ok_and(|v| {
        let l = v.to_ascii_lowercase();
        matches!(l.as_str(), "off" | "0" | "false" | "no" | "disabled")
    })
}

/// Once-per-process announcement of the telemetry call. The first
/// time we make a version check, log loudly what gets sent and how
/// to opt out. Subsequent calls stay quiet (this is `info!`, not
/// `warn!`, so log level filtering still applies).
fn announce_once(config: &VersionCheckConfig) {
    static ANNOUNCED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ANNOUNCED.get_or_init(|| {
        tracing::info!(
            endpoint = %config.api_url,
            "version check telemetry: sending anonymous {{product, current_version, os, arch}} to endpoint; \
             set HYPERI_TELEMETRY=off to disable"
        );
    });
}

/// Perform the HTTP version check.
async fn do_version_check(
    config: &VersionCheckConfig,
) -> Result<VersionCheckResponse, VersionCheckError> {
    if telemetry_opted_out() {
        return Err(VersionCheckError::Http(
            "telemetry opted out via HYPERI_TELEMETRY env var".into(),
        ));
    }

    announce_once(config);

    let payload = CheckPayload {
        product: config.product.clone(),
        current_version: config.current_version.clone(),
        os: Some(std::env::consts::OS.into()),
        arch: Some(std::env::consts::ARCH.into()),
    };

    let client = reqwest::Client::builder()
        .timeout(config.timeout)
        .build()
        .map_err(|e| VersionCheckError::Http(e.to_string()))?;

    let resp = client
        .post(&config.api_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| VersionCheckError::Http(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(VersionCheckError::Http(format!("HTTP {}", resp.status())));
    }

    resp.json::<VersionCheckResponse>()
        .await
        .map_err(|e| VersionCheckError::Parse(e.to_string()))
}

/// Log the version check response at the appropriate level.
fn log_version_response(config: &VersionCheckConfig, resp: &VersionCheckResponse) {
    if resp.update_available {
        if let Some(ref latest) = resp.latest_version {
            let age = resp
                .published_at
                .as_deref()
                .and_then(format_age)
                .unwrap_or_default();

            tracing::info!(
                product = %config.product,
                current = %config.current_version,
                latest = %latest,
                age = %age,
                url = resp.release_url.as_deref().unwrap_or(""),
                "new version available"
            );
        }
    } else {
        tracing::debug!(
            product = %config.product,
            version = %config.current_version,
            "running latest version"
        );
    }

    if let Some(ref msg) = resp.message
        && !msg.is_empty()
    {
        tracing::info!(product = %config.product, "{msg}");
    }
}

/// Format an ISO 8601 timestamp into a human-readable age string.
///
/// Returns `None` if the timestamp cannot be parsed.
fn format_age(published_at: &str) -> Option<String> {
    // Parse ISO 8601 with timezone (e.g., "2026-01-15T10:00:00Z")
    // Try with timezone first, then without
    let published = published_at
        .parse::<chrono::DateTime<chrono::Utc>>()
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(published_at, "%Y-%m-%dT%H:%M:%S")
                .map(|dt| dt.and_utc())
        })
        .ok()?;

    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(published);

    let days = duration.num_days();
    if days < 0 {
        return Some("just released".into());
    }
    if days == 0 {
        return Some("released today".into());
    }
    if days == 1 {
        return Some("released 1 day ago".into());
    }
    if days < 30 {
        return Some(format!("released {days} days ago"));
    }
    let months = days / 30;
    if months == 1 {
        return Some("released 1 month ago".into());
    }
    if months < 12 {
        return Some(format!("released {months} months ago"));
    }
    let years = months / 12;
    let remaining_months = months % 12;
    if remaining_months == 0 {
        Some(format!("released {years}y ago"))
    } else {
        Some(format!("released {years}y {remaining_months}m ago"))
    }
}

// Persistent-instance-id helpers removed in 2.7.5.
//
// Previously this file maintained a UUID at
// `~/.cache/hyperi/instance_id` and included it in every telemetry
// payload. The persistent identifier survived restarts, IP changes,
// and process recycles -- functionally indistinguishable from a
// long-lived tracking cookie. For an open-source library the friction
// it created for SOC2 / regulated consumers far exceeded the fleet-
// uniqueness signal value. Removed in the pre-GA hardening pass.

/// Errors during version check (internal, never exposed to caller).
#[derive(Debug)]
enum VersionCheckError {
    Http(String),
    Parse(String),
}

impl std::fmt::Display for VersionCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "http: {e}"),
            Self::Parse(e) => write!(f, "parse: {e}"),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = VersionCheckConfig::default();
        assert_eq!(config.api_url, DEFAULT_API_URL);
        assert_eq!(config.timeout, Duration::from_secs(5));
        assert!(!config.disabled);
        assert!(config.product.is_empty());
    }

    #[test]
    fn telemetry_opt_out_recognises_common_values() {
        // `temp_env::with_var` scopes env mutation to the closure and
        // restores the previous value on drop -- required because the
        // crate has `#![deny(unsafe_code)]` and edition 2024 forbids
        // direct `std::env::set_var` without `unsafe { }`.
        for v in ["off", "Off", "OFF", "0", "false", "False", "no", "disabled"] {
            temp_env::with_var("HYPERI_TELEMETRY", Some(v), || {
                assert!(telemetry_opted_out(), "value `{v}` should opt out");
            });
        }
        for v in ["on", "1", "true", ""] {
            temp_env::with_var("HYPERI_TELEMETRY", Some(v), || {
                assert!(!telemetry_opted_out(), "value `{v}` should NOT opt out");
            });
        }
        temp_env::with_var_unset("HYPERI_TELEMETRY", || {
            assert!(!telemetry_opted_out(), "absent var should NOT opt out");
        });
    }

    #[test]
    fn check_payload_omits_dropped_fields() {
        // The payload struct itself no longer has instance_id or deployment
        // -- this test enforces that by serialising and checking the JSON
        // shape. A future change that re-adds them will fail here.
        let payload = CheckPayload {
            product: "dfe-loader".into(),
            current_version: "1.0.0".into(),
            os: Some("linux".into()),
            arch: Some("x86_64".into()),
        };
        let json = serde_json::to_string(&payload).unwrap();
        assert!(!json.contains("instance_id"));
        assert!(!json.contains("deployment"));
        assert!(json.contains("product"));
        assert!(json.contains("current_version"));
        assert!(json.contains("\"os\":\"linux\""));
        assert!(json.contains("\"arch\":\"x86_64\""));
    }

    #[test]
    fn test_check_payload_serialization() {
        let payload = CheckPayload {
            product: "dfe-loader".into(),
            current_version: "1.8.0".into(),
            os: Some("linux".into()),
            arch: Some("x86_64".into()),
        };

        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["product"], "dfe-loader");
        assert_eq!(json["current_version"], "1.8.0");
        assert_eq!(json["os"], "linux");
        assert_eq!(json["arch"], "x86_64");
        // Dropped fields must not reappear.
        assert!(json.get("instance_id").is_none());
        assert!(json.get("deployment").is_none());
    }

    #[test]
    fn test_response_deserialization() {
        let json = r#"{
            "latest_version": "1.9.0",
            "update_available": true,
            "release_url": "https://github.com/hyperi-io/dfe-loader/releases/tag/v1.9.0",
            "published_at": "2026-02-15T10:00:00Z",
            "message": null
        }"#;

        let resp: VersionCheckResponse = serde_json::from_str(json).unwrap();
        assert!(resp.update_available);
        assert_eq!(resp.latest_version.as_deref(), Some("1.9.0"));
        assert_eq!(resp.published_at.as_deref(), Some("2026-02-15T10:00:00Z"));
        assert!(resp.message.is_none());
    }

    #[test]
    fn test_response_no_update() {
        let json = r#"{
            "latest_version": "1.8.0",
            "update_available": false,
            "release_url": null,
            "published_at": null,
            "message": null
        }"#;

        let resp: VersionCheckResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.update_available);
    }

    #[test]
    fn test_format_age_today() {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let age = format_age(&now).unwrap();
        assert_eq!(age, "released today");
    }

    #[test]
    fn test_format_age_days() {
        let ten_days_ago = (chrono::Utc::now() - chrono::Duration::days(10))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        let age = format_age(&ten_days_ago).unwrap();
        assert_eq!(age, "released 10 days ago");
    }

    #[test]
    fn test_format_age_months() {
        let three_months_ago = (chrono::Utc::now() - chrono::Duration::days(90))
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        let age = format_age(&three_months_ago).unwrap();
        assert_eq!(age, "released 3 months ago");
    }

    #[test]
    fn test_format_age_invalid() {
        assert!(format_age("not-a-date").is_none());
    }

    #[test]
    fn test_disabled_does_not_spawn() {
        let checker = VersionCheck::new(VersionCheckConfig {
            disabled: true,
            ..Default::default()
        });
        // Should return immediately without panic (no tokio runtime needed)
        checker.check_on_startup();
    }

    #[test]
    fn test_empty_product_does_not_spawn() {
        let checker = VersionCheck::new(VersionCheckConfig::default());
        // Should return immediately without panic
        checker.check_on_startup();
    }
}
