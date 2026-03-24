// Project:   hyperi-rustlib
// File:      src/config/registry.rs
// Purpose:   Auto-registering config registry for reflection and admin endpoints
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Reflectable configuration registry.
//!
//! Modules that read config via [`Config::unmarshal_key_registered`] are
//! automatically recorded in a global registry. This enables:
//!
//! - Listing all config sections an application uses
//! - Dumping the effective config (with redaction) for debugging
//! - Dumping defaults for documentation
//! - Future: admin `/config` endpoint, change notifications
//!
//! # Auto-registration
//!
//! ```rust,no_run
//! use hyperi_rustlib::config;
//!
//! // This automatically registers "expression" in the registry:
//! let cfg = config::get();
//! let profile: MyConfig = cfg.unmarshal_key_registered("expression").unwrap_or_default();
//!
//! // Later, reflect on all registered sections:
//! for section in config::registry::sections() {
//!     println!("{}: {}", section.key, section.type_name);
//! }
//! ```

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde_json::Value as JsonValue;

/// Global config registry singleton.
static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

/// A registered config section.
#[derive(Debug, Clone)]
pub struct ConfigSection {
    /// The config key (e.g., "expression", "memory", "version_check").
    pub key: String,
    /// The Rust type name (e.g., "ProfileConfig").
    pub type_name: String,
    /// Default values as JSON (from `T::default()`).
    pub defaults: JsonValue,
    /// Effective values as JSON (from the cascade).
    pub effective: JsonValue,
}

/// The config registry — stores all registered sections.
#[derive(Debug, Clone, Default)]
struct Registry {
    sections: BTreeMap<String, ConfigSection>,
}

/// Register a config section in the global registry.
///
/// Called automatically by [`Config::unmarshal_key_registered`]. Can also
/// be called manually for sections that don't go through the cascade.
///
/// Requires `T: Serialize + Default` so we can capture both the default
/// and effective values as JSON for reflection.
pub fn register<T>(key: &str, effective: &T)
where
    T: serde::Serialize + Default + 'static,
{
    let section = ConfigSection {
        key: key.to_string(),
        type_name: std::any::type_name::<T>().to_string(),
        defaults: serde_json::to_value(T::default()).unwrap_or(JsonValue::Null),
        effective: serde_json::to_value(effective).unwrap_or(JsonValue::Null),
    };

    if let Ok(mut guard) = REGISTRY.lock() {
        let registry = guard.get_or_insert_with(Registry::default);
        registry.sections.insert(key.to_string(), section);
    }
}

/// List all registered config sections, sorted by key.
#[must_use]
pub fn sections() -> Vec<ConfigSection> {
    REGISTRY
        .lock()
        .ok()
        .and_then(|guard| {
            guard
                .as_ref()
                .map(|r| r.sections.values().cloned().collect())
        })
        .unwrap_or_default()
}

/// Dump all effective config values as a JSON object.
///
/// Keys are config section names, values are the effective config.
/// Sensitive fields should use `#[serde(skip_serializing)]` on the
/// struct to be excluded automatically.
#[must_use]
pub fn dump_effective() -> JsonValue {
    let map: serde_json::Map<String, JsonValue> = sections()
        .into_iter()
        .map(|s| (s.key, s.effective))
        .collect();
    JsonValue::Object(map)
}

/// Dump all default config values as a JSON object.
///
/// Keys are config section names, values are the defaults from
/// `T::default()`.
#[must_use]
pub fn dump_defaults() -> JsonValue {
    let map: serde_json::Map<String, JsonValue> = sections()
        .into_iter()
        .map(|s| (s.key, s.defaults))
        .collect();
    JsonValue::Object(map)
}

/// Check if a specific key is registered.
#[must_use]
pub fn is_registered(key: &str) -> bool {
    REGISTRY
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|r| r.sections.contains_key(key)))
        .unwrap_or(false)
}

/// Get a single registered section by key.
#[must_use]
pub fn get_section(key: &str) -> Option<ConfigSection> {
    REGISTRY
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().and_then(|r| r.sections.get(key).cloned()))
}

/// Clear the registry (for testing only).
#[cfg(test)]
pub(crate) fn clear() {
    if let Ok(mut guard) = REGISTRY.lock() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default, PartialEq)]
    struct TestConfig {
        enabled: bool,
        threshold: f64,
        #[serde(skip_serializing)]
        secret_token: String,
    }

    #[test]
    fn register_and_retrieve() {
        clear();

        let config = TestConfig {
            enabled: true,
            threshold: 0.75,
            secret_token: "hunter2".into(),
        };
        register::<TestConfig>("test_module", &config);

        assert!(is_registered("test_module"));
        assert!(!is_registered("nonexistent"));

        let section = get_section("test_module").unwrap();
        assert_eq!(section.key, "test_module");
        assert!(section.type_name.contains("TestConfig"));

        // Effective should have the values we set (secret redacted by skip_serializing)
        assert_eq!(section.effective["enabled"], true);
        assert_eq!(section.effective["threshold"], 0.75);
        assert!(section.effective.get("secret_token").is_none());

        // Defaults should have Default::default() values
        assert_eq!(section.defaults["enabled"], false);
        assert_eq!(section.defaults["threshold"], 0.0);
    }

    #[test]
    fn sections_returns_sorted() {
        clear();

        register::<TestConfig>("zebra", &TestConfig::default());
        register::<TestConfig>("alpha", &TestConfig::default());
        register::<TestConfig>("middle", &TestConfig::default());

        let keys: Vec<String> = sections().iter().map(|s| s.key.clone()).collect();
        assert_eq!(keys, vec!["alpha", "middle", "zebra"]);
    }

    #[test]
    fn dump_effective_returns_json_object() {
        clear();

        let config = TestConfig {
            enabled: true,
            threshold: 0.9,
            secret_token: "secret".into(),
        };
        register::<TestConfig>("my_module", &config);

        let dump = dump_effective();
        assert!(dump.is_object());
        assert_eq!(dump["my_module"]["enabled"], true);
        assert_eq!(dump["my_module"]["threshold"], 0.9);
        // secret_token skipped via #[serde(skip_serializing)]
        assert!(dump["my_module"].get("secret_token").is_none());
    }

    #[test]
    fn dump_defaults_returns_default_values() {
        clear();

        register::<TestConfig>(
            "my_module",
            &TestConfig {
                enabled: true,
                threshold: 0.9,
                secret_token: String::new(),
            },
        );

        let dump = dump_defaults();
        assert_eq!(dump["my_module"]["enabled"], false);
        assert_eq!(dump["my_module"]["threshold"], 0.0);
    }

    #[test]
    fn re_register_overwrites() {
        clear();

        let v1 = TestConfig {
            enabled: false,
            threshold: 0.5,
            secret_token: String::new(),
        };
        register::<TestConfig>("module", &v1);
        assert_eq!(get_section("module").unwrap().effective["threshold"], 0.5);

        let v2 = TestConfig {
            enabled: true,
            threshold: 0.9,
            secret_token: String::new(),
        };
        register::<TestConfig>("module", &v2);
        assert_eq!(get_section("module").unwrap().effective["threshold"], 0.9);
    }

    #[test]
    fn empty_registry() {
        clear();

        assert!(sections().is_empty());
        assert_eq!(dump_effective(), JsonValue::Object(serde_json::Map::new()));
        assert_eq!(dump_defaults(), JsonValue::Object(serde_json::Map::new()));
        assert!(!is_registered("anything"));
        assert!(get_section("anything").is_none());
    }
}
