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
//!     // Fire-and-forget — spawns a background task, never blocks startup
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
/// `product` and `current_version` are always set programmatically — they
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
                && let Ok(vc) = cfg.unmarshal_key::<Self>("version_check")
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
#[derive(Debug, Serialize)]
struct CheckPayload {
    product: String,
    current_version: String,
    instance_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    os: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    arch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deployment: Option<String>,
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

/// Perform the HTTP version check.
async fn do_version_check(
    config: &VersionCheckConfig,
) -> Result<VersionCheckResponse, VersionCheckError> {
    let instance_id = get_or_create_instance_id();

    let payload = CheckPayload {
        product: config.product.clone(),
        current_version: config.current_version.clone(),
        instance_id,
        os: Some(std::env::consts::OS.into()),
        arch: Some(std::env::consts::ARCH.into()),
        deployment: config.deployment.clone(),
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

/// Get or create a persistent instance ID.
///
/// Tries to read from `~/.config/hyperi/instance_id`. If it doesn't exist,
/// generates a new UUIDv4 and persists it. Falls back to an ephemeral UUID
/// if the file can't be written.
///
/// The result is cached in-process via `OnceLock` — the file is read at most
/// once per process, eliminating TOCTOU races between parallel callers.
fn get_or_create_instance_id() -> String {
    static INSTANCE_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();

    INSTANCE_ID
        .get_or_init(|| {
            let config_dir = dirs::config_dir().map_or_else(
                || std::path::PathBuf::from("/tmp/hyperi"),
                |d| d.join("hyperi"),
            );

            let id_path = config_dir.join("instance_id");

            // Try to read existing
            if let Ok(id) = std::fs::read_to_string(&id_path) {
                let id = id.trim().to_string();
                if !id.is_empty() {
                    return id;
                }
            }

            // Generate new
            let id = uuid::Uuid::new_v4().to_string();

            // Try to persist (best-effort)
            if std::fs::create_dir_all(&config_dir).is_ok() {
                let _ = std::fs::write(&id_path, &id);
            }

            id
        })
        .clone()
}

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
    fn test_instance_id_stable() {
        // Multiple calls return the same ID
        let id1 = get_or_create_instance_id();
        let id2 = get_or_create_instance_id();
        assert_eq!(id1, id2);
        assert!(!id1.is_empty());
    }

    #[test]
    fn test_instance_id_is_uuid() {
        let id = get_or_create_instance_id();
        assert!(uuid::Uuid::parse_str(&id).is_ok(), "not a valid UUID: {id}");
    }

    #[test]
    fn test_check_payload_serialization() {
        let payload = CheckPayload {
            product: "dfe-loader".into(),
            current_version: "1.8.0".into(),
            instance_id: "test-id".into(),
            os: Some("linux".into()),
            arch: Some("x86_64".into()),
            deployment: None,
        };

        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["product"], "dfe-loader");
        assert_eq!(json["current_version"], "1.8.0");
        // deployment is None, should be absent
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
