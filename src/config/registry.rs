// Project:   hyperi-rustlib
// File:      src/config/registry.rs
// Purpose:   Auto-registering config registry for reflection and admin endpoints
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Reflectable configuration registry.
//!
//! Modules that read config via [`Config::unmarshal_key_registered`](crate::config::Config::unmarshal_key_registered) are
//! automatically recorded in a global registry. This enables:
//!
//! - Listing all config sections an application uses
//! - Dumping the effective config (with redaction) for debugging
//! - Dumping defaults for documentation
//! - Future: admin `/config` endpoint, change notifications
//!
//! # Auto-registration
//!
//! ```rust,ignore
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

/// A boxed change listener callback.
type ChangeCallback = Box<dyn Fn(&JsonValue) + Send>;

/// Change listener storage.
static LISTENERS: Mutex<Option<BTreeMap<String, Vec<ChangeCallback>>>> = Mutex::new(None);

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
/// Called automatically by [`Config::unmarshal_key_registered`](crate::config::Config::unmarshal_key_registered). Can also
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

/// Dump all effective config values as a JSON object (redacted).
///
/// Applies heuristic redaction to fields whose names contain sensitive
/// patterns (password, secret, token, key, credential, auth, private,
/// cert, encryption). Fields with `#[serde(skip_serializing)]` are
/// already excluded at serialisation time — this is the safety net for
/// fields that weren't annotated.
#[must_use]
pub fn dump_effective() -> JsonValue {
    let mut map: serde_json::Map<String, JsonValue> = sections()
        .into_iter()
        .map(|s| (s.key, s.effective))
        .collect();
    for value in map.values_mut() {
        if let JsonValue::Object(obj) = value {
            redact_sensitive_fields(obj);
        }
    }
    JsonValue::Object(map)
}

/// Dump effective config WITHOUT redaction (for internal/debug use only).
///
/// Do NOT expose this on any endpoint. Use `dump_effective()` for safe output.
#[must_use]
pub fn dump_effective_unredacted() -> JsonValue {
    let map: serde_json::Map<String, JsonValue> = sections()
        .into_iter()
        .map(|s| (s.key, s.effective))
        .collect();
    JsonValue::Object(map)
}

/// Dump all default config values as a JSON object (redacted).
#[must_use]
pub fn dump_defaults() -> JsonValue {
    let mut map: serde_json::Map<String, JsonValue> = sections()
        .into_iter()
        .map(|s| (s.key, s.defaults))
        .collect();
    for value in map.values_mut() {
        if let JsonValue::Object(obj) = value {
            redact_sensitive_fields(obj);
        }
    }
    JsonValue::Object(map)
}

/// Field name patterns that trigger automatic redaction.
///
/// Any JSON field whose name (lowercased) contains one of these
/// substrings will have its value replaced with `"***REDACTED***"`.
/// Field name patterns that trigger automatic redaction.
///
/// Any JSON field whose name (lowercased) contains one of these
/// substrings will have its value replaced with `"***REDACTED***"`.
///
/// This is a safety net — the primary protection is [`SensitiveString`]
/// on the field type (compile-time safe). This heuristic catches fields
/// that developers forgot to mark as sensitive.
const SENSITIVE_PATTERNS: &[&str] = &[
    "password",
    "secret",
    "token",
    "key",
    "credential",
    "auth",
    "private",
    "cert",
    "encryption",
    "connection_string",
    "dsn",
];

const REDACTED: &str = "***REDACTED***";

/// Recursively redact fields with sensitive names.
fn redact_sensitive_fields(obj: &mut serde_json::Map<String, JsonValue>) {
    for (key, value) in obj.iter_mut() {
        let lower = key.to_lowercase();
        if SENSITIVE_PATTERNS.iter().any(|p| lower.contains(p)) {
            *value = JsonValue::String(REDACTED.into());
            continue;
        }
        match value {
            JsonValue::Object(nested) => redact_sensitive_fields(nested),
            JsonValue::Array(arr) => {
                for item in arr.iter_mut() {
                    if let JsonValue::Object(nested) = item {
                        redact_sensitive_fields(nested);
                    }
                }
            }
            _ => {}
        }
    }
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

/// Subscribe to changes for a specific config key (opt-in).
///
/// The callback fires when [`update`] is called for the given key.
/// Modules that need hot-reload subscribe at init; modules that don't
/// simply use the `OnceLock` pattern and ignore change events.
///
/// The callback receives the new effective value as JSON.
pub fn on_change(key: &str, callback: impl Fn(&JsonValue) + Send + 'static) {
    if let Ok(mut guard) = LISTENERS.lock() {
        let listeners = guard.get_or_insert_with(BTreeMap::new);
        listeners
            .entry(key.to_string())
            .or_default()
            .push(Box::new(callback));
    }
}

/// Re-register a config section and notify listeners.
///
/// Call this when config is reloaded (e.g., from `ConfigReloader`).
/// Listeners registered via [`on_change`] are notified with the new
/// effective value.
pub fn update<T>(key: &str, effective: &T)
where
    T: serde::Serialize + Default + 'static,
{
    let effective_json = serde_json::to_value(effective).unwrap_or(JsonValue::Null);

    // Update the registry entry
    let section = ConfigSection {
        key: key.to_string(),
        type_name: std::any::type_name::<T>().to_string(),
        defaults: serde_json::to_value(T::default()).unwrap_or(JsonValue::Null),
        effective: effective_json.clone(),
    };

    if let Ok(mut guard) = REGISTRY.lock() {
        let registry = guard.get_or_insert_with(Registry::default);
        registry.sections.insert(key.to_string(), section);
    }

    // Notify listeners
    if let Ok(guard) = LISTENERS.lock()
        && let Some(listeners) = &*guard
        && let Some(callbacks) = listeners.get(key)
    {
        for cb in callbacks {
            cb(&effective_json);
        }
    }
}

/// Clear the registry (for testing only).
#[cfg(test)]
pub(crate) fn clear() {
    if let Ok(mut guard) = REGISTRY.lock() {
        *guard = None;
    }
    if let Ok(mut guard) = LISTENERS.lock() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    /// Tests share global statics — serialise them.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    macro_rules! serial_test {
        () => {
            let _guard = TEST_LOCK.lock().unwrap();
            clear();
        };
    }

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default, PartialEq)]
    struct TestConfig {
        enabled: bool,
        threshold: f64,
        #[serde(skip_serializing)]
        secret_token: String,
    }

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
    struct SensitiveConfig {
        host: String,
        password: String,
        api_token: String,
        encryption_key: String,
        normal_field: u32,
        nested: NestedSensitive,
    }

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
    struct NestedSensitive {
        db_password: String,
        port: u16,
    }

    #[test]
    fn register_and_retrieve() {
        serial_test!();

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

        assert_eq!(section.effective["enabled"], true);
        assert_eq!(section.effective["threshold"], 0.75);
        // skip_serializing excludes it entirely
        assert!(section.effective.get("secret_token").is_none());

        assert_eq!(section.defaults["enabled"], false);
        assert_eq!(section.defaults["threshold"], 0.0);
    }

    #[test]
    fn sections_returns_sorted() {
        serial_test!();

        register::<TestConfig>("zebra", &TestConfig::default());
        register::<TestConfig>("alpha", &TestConfig::default());
        register::<TestConfig>("middle", &TestConfig::default());

        let keys: Vec<String> = sections().iter().map(|s| s.key.clone()).collect();
        assert_eq!(keys, vec!["alpha", "middle", "zebra"]);
    }

    #[test]
    fn dump_effective_redacts_sensitive_fields() {
        serial_test!();

        let config = SensitiveConfig {
            host: "db.example.com".into(),
            password: "super_secret".into(),
            api_token: "tok_abc123".into(),
            encryption_key: "aes256key".into(),
            normal_field: 42,
            nested: NestedSensitive {
                db_password: "nested_secret".into(),
                port: 5432,
            },
        };
        register::<SensitiveConfig>("db", &config);

        let dump = dump_effective();
        // Non-sensitive preserved
        assert_eq!(dump["db"]["host"], "db.example.com");
        assert_eq!(dump["db"]["normal_field"], 42);
        assert_eq!(dump["db"]["nested"]["port"], 5432);

        // Sensitive fields redacted
        assert_eq!(dump["db"]["password"], REDACTED);
        assert_eq!(dump["db"]["api_token"], REDACTED);
        assert_eq!(dump["db"]["encryption_key"], REDACTED);
        assert_eq!(dump["db"]["nested"]["db_password"], REDACTED);
    }

    #[test]
    fn dump_unredacted_preserves_all_fields() {
        serial_test!();

        let config = SensitiveConfig {
            password: "visible".into(),
            ..Default::default()
        };
        register::<SensitiveConfig>("db", &config);

        let dump = dump_effective_unredacted();
        assert_eq!(dump["db"]["password"], "visible");
    }

    #[test]
    fn dump_defaults_returns_default_values() {
        serial_test!();

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
        serial_test!();

        let v1 = TestConfig {
            threshold: 0.5,
            ..Default::default()
        };
        register::<TestConfig>("module", &v1);
        assert_eq!(get_section("module").unwrap().effective["threshold"], 0.5);

        let v2 = TestConfig {
            threshold: 0.9,
            ..Default::default()
        };
        register::<TestConfig>("module", &v2);
        assert_eq!(get_section("module").unwrap().effective["threshold"], 0.9);
    }

    #[test]
    fn empty_registry() {
        serial_test!();

        assert!(sections().is_empty());
        assert_eq!(dump_effective(), JsonValue::Object(serde_json::Map::new()));
        assert_eq!(dump_defaults(), JsonValue::Object(serde_json::Map::new()));
        assert!(!is_registered("anything"));
        assert!(get_section("anything").is_none());
    }

    // ── Change notification ─────────────────────────────────────

    #[test]
    fn on_change_fires_on_update() {
        serial_test!();

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        on_change("my_key", move |_value| {
            counter_clone.fetch_add(1, Ordering::Relaxed);
        });

        let config = TestConfig {
            enabled: true,
            ..Default::default()
        };
        update::<TestConfig>("my_key", &config);

        assert_eq!(counter.load(Ordering::Relaxed), 1);

        // Second update fires again
        update::<TestConfig>("my_key", &config);
        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn on_change_receives_new_value() {
        serial_test!();

        let captured = Arc::new(Mutex::new(JsonValue::Null));
        let captured_clone = captured.clone();

        on_change("watched", move |value| {
            if let Ok(mut guard) = captured_clone.lock() {
                *guard = value.clone();
            }
        });

        let config = TestConfig {
            enabled: true,
            threshold: 0.99,
            ..Default::default()
        };
        update::<TestConfig>("watched", &config);

        let val = captured.lock().unwrap().clone();
        assert_eq!(val["enabled"], true);
        assert_eq!(val["threshold"], 0.99);
    }

    #[test]
    fn on_change_only_fires_for_subscribed_key() {
        serial_test!();

        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();

        on_change("key_a", move |_| {
            counter_clone.fetch_add(1, Ordering::Relaxed);
        });

        // Update a different key — listener should NOT fire
        update::<TestConfig>("key_b", &TestConfig::default());
        assert_eq!(counter.load(Ordering::Relaxed), 0);

        // Update the subscribed key — listener fires
        update::<TestConfig>("key_a", &TestConfig::default());
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn update_also_registers() {
        serial_test!();

        assert!(!is_registered("fresh"));
        update::<TestConfig>(
            "fresh",
            &TestConfig {
                enabled: true,
                ..Default::default()
            },
        );
        assert!(is_registered("fresh"));
        assert_eq!(get_section("fresh").unwrap().effective["enabled"], true);
    }

    // ── Redaction test structs (module-level to avoid items_after_statements) ──

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
    struct MixedCase {
        #[serde(rename = "Password")]
        password_upper: String,
        #[serde(rename = "API_TOKEN")]
        token_upper: String,
        #[serde(rename = "mySecret")]
        secret_camel: String,
    }

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
    struct DeepNested {
        level1: Level1,
    }
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
    struct Level1 {
        level2: Level2,
        name: String,
    }
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
    struct Level2 {
        api_token: String,
        db_password: String,
        port: u16,
    }

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
    struct WithArray {
        items: Vec<ArrayItem>,
    }
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
    struct ArrayItem {
        name: String,
        secret_key: String,
    }

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    struct WithDefaultSecret {
        api_token: String,
        host: String,
    }
    impl Default for WithDefaultSecret {
        fn default() -> Self {
            Self {
                api_token: "default-placeholder-token".into(),
                host: "localhost".into(),
            }
        }
    }

    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
    struct DoubleProtected {
        #[serde(skip_serializing)]
        #[allow(dead_code)]
        hidden_secret: String,
        visible_token: String,
        normal: String,
    }

    // ── Redaction guarantee tests ──────────────────────────────

    /// Config struct that exercises ALL sensitive field name patterns.
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
    struct AllSensitivePatterns {
        // Each SENSITIVE_PATTERNS entry must be covered
        my_password: String,
        db_secret: String,
        api_token: String,
        encryption_key: String,
        aws_credential: String,
        oauth_auth_code: String,
        private_data: String,
        tls_cert_path: String,
        // Non-sensitive controls (must NOT be redacted)
        hostname: String,
        port: u16,
        enabled: bool,
        timeout_ms: u64,
    }

    #[test]
    fn redaction_covers_all_sensitive_patterns() {
        serial_test!();

        let config = AllSensitivePatterns {
            my_password: "pass123".into(),
            db_secret: "sec456".into(),
            api_token: "tok789".into(),
            encryption_key: "key012".into(),
            aws_credential: "cred345".into(),
            oauth_auth_code: "auth678".into(),
            private_data: "priv901".into(),
            tls_cert_path: "/etc/tls/cert.pem".into(),
            hostname: "db.prod.internal".into(),
            port: 5432,
            enabled: true,
            timeout_ms: 30000,
        };
        register::<AllSensitivePatterns>("all_patterns", &config);

        let dump = dump_effective();
        let section = &dump["all_patterns"];

        // Every sensitive field MUST be redacted
        assert_eq!(section["my_password"], REDACTED, "password pattern missed");
        assert_eq!(section["db_secret"], REDACTED, "secret pattern missed");
        assert_eq!(section["api_token"], REDACTED, "token pattern missed");
        assert_eq!(section["encryption_key"], REDACTED, "key pattern missed");
        assert_eq!(
            section["aws_credential"], REDACTED,
            "credential pattern missed"
        );
        assert_eq!(section["oauth_auth_code"], REDACTED, "auth pattern missed");
        assert_eq!(section["private_data"], REDACTED, "private pattern missed");
        assert_eq!(section["tls_cert_path"], REDACTED, "cert pattern missed");

        // Non-sensitive fields MUST be preserved
        assert_eq!(section["hostname"], "db.prod.internal");
        assert_eq!(section["port"], 5432);
        assert_eq!(section["enabled"], true);
        assert_eq!(section["timeout_ms"], 30000);
    }

    #[test]
    fn redaction_is_case_insensitive() {
        serial_test!();

        let config = MixedCase {
            password_upper: "visible_if_broken".into(),
            token_upper: "visible_if_broken".into(),
            secret_camel: "visible_if_broken".into(),
        };
        register::<MixedCase>("case_test", &config);

        let dump = dump_effective();
        let section = &dump["case_test"];

        assert_eq!(section["Password"], REDACTED);
        assert_eq!(section["API_TOKEN"], REDACTED);
        assert_eq!(section["mySecret"], REDACTED);
    }

    #[test]
    fn redaction_handles_deeply_nested_secrets() {
        serial_test!();

        let config = DeepNested {
            level1: Level1 {
                level2: Level2 {
                    api_token: "deep_secret_1".into(),
                    db_password: "deep_secret_2".into(),
                    port: 3306,
                },
                name: "safe_value".into(),
            },
        };
        register::<DeepNested>("deep", &config);

        let dump = dump_effective();
        assert_eq!(dump["deep"]["level1"]["level2"]["api_token"], REDACTED);
        assert_eq!(dump["deep"]["level1"]["level2"]["db_password"], REDACTED);
        assert_eq!(dump["deep"]["level1"]["level2"]["port"], 3306);
        assert_eq!(dump["deep"]["level1"]["name"], "safe_value");
    }

    #[test]
    fn redaction_handles_arrays_with_sensitive_objects() {
        serial_test!();

        let config = WithArray {
            items: vec![
                ArrayItem {
                    name: "item1".into(),
                    secret_key: "sk_1".into(),
                },
                ArrayItem {
                    name: "item2".into(),
                    secret_key: "sk_2".into(),
                },
            ],
        };
        register::<WithArray>("array_test", &config);

        let dump = dump_effective();
        let items = dump["array_test"]["items"].as_array().unwrap();
        for item in items {
            assert_eq!(item["secret_key"], REDACTED);
            assert_ne!(item["name"], REDACTED); // name should be preserved
        }
    }

    #[test]
    fn no_secret_values_in_redacted_dump_string() {
        serial_test!();

        let secrets = [
            "hunter2",
            "sk_live_abc123",
            "super_s3cret!",
            "my-private-key-data",
        ];

        let config = AllSensitivePatterns {
            my_password: secrets[0].into(),
            db_secret: secrets[1].into(),
            api_token: secrets[2].into(),
            encryption_key: secrets[3].into(),
            ..Default::default()
        };
        register::<AllSensitivePatterns>("leak_check", &config);

        // Serialise the full dump to a string and scan for ANY secret value
        let dump = dump_effective();
        let dump_str = serde_json::to_string(&dump).unwrap();

        for secret in &secrets {
            assert!(
                !dump_str.contains(secret),
                "SECRET LEAKED in dump_effective(): '{secret}' found in output"
            );
        }
    }

    #[test]
    fn defaults_dump_also_redacted() {
        serial_test!();

        register::<WithDefaultSecret>("default_secrets", &WithDefaultSecret::default());

        let dump = dump_defaults();
        assert_eq!(dump["default_secrets"]["api_token"], REDACTED);
        assert_eq!(dump["default_secrets"]["host"], "localhost");
    }

    #[test]
    fn skip_serializing_plus_heuristic_double_protection() {
        serial_test!();

        let config = DoubleProtected {
            hidden_secret: "should_not_appear".into(),
            visible_token: "should_be_redacted".into(),
            normal: "visible".into(),
        };
        register::<DoubleProtected>("double", &config);

        let dump = dump_effective();
        let section = &dump["double"];

        // skip_serializing: field absent entirely
        assert!(section.get("hidden_secret").is_none());
        // heuristic: field present but redacted
        assert_eq!(section["visible_token"], REDACTED);
        // normal: preserved
        assert_eq!(section["normal"], "visible");

        // String scan: neither secret should appear
        let dump_str = serde_json::to_string(&dump).unwrap();
        assert!(!dump_str.contains("should_not_appear"));
        assert!(!dump_str.contains("should_be_redacted"));
    }

    // ── Change notification ─────────────────────────────────────

    #[test]
    fn multiple_listeners_on_same_key() {
        serial_test!();

        let c1 = Arc::new(AtomicU32::new(0));
        let c2 = Arc::new(AtomicU32::new(0));
        let c1c = c1.clone();
        let c2c = c2.clone();

        on_change("shared", move |_| {
            c1c.fetch_add(1, Ordering::Relaxed);
        });
        on_change("shared", move |_| {
            c2c.fetch_add(1, Ordering::Relaxed);
        });

        update::<TestConfig>("shared", &TestConfig::default());

        assert_eq!(c1.load(Ordering::Relaxed), 1);
        assert_eq!(c2.load(Ordering::Relaxed), 1);
    }
}
