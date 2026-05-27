// Project:   hyperi-rustlib
// File:      src/database/mod.rs
// Purpose:   Database connection string builders from env vars and config
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Database connection string builders.
//!
//! Builds connection URLs from environment variables with standard prefixes.
//! Each builder reads `{PREFIX}_HOST`, `{PREFIX}_PORT`, `{PREFIX}_USER`,
//! `{PREFIX}_PASSWORD`, `{PREFIX}_DB` and constructs the appropriate URL.
//!
//! Password fields use [`crate::SensitiveString`] for compile-time safe redaction.
//!
//! # Supported Databases
//!
//! | Database | Default Port | URL Format |
//! |----------|-------------|------------|
//! | PostgreSQL | 5432 | `postgresql://user:pass@host:port/db` |
//! | ClickHouse | 8123 | `http://user:pass@host:port/db` (HTTP) |
//! | ClickHouse Native | 9000 | `tcp://user:pass@host:port/db` |
//! | Redis/Valkey | 6379 | `redis://user:pass@host:port/db` |
//! | MongoDB | 27017 | `mongodb://user:pass@host:port/db` |
//!
//! # Usage
//!
//! ```rust
//! use hyperi_rustlib::database::{PostgresUrl, DatabaseUrl};
//!
//! // From explicit values
//! let url = PostgresUrl::new("db.prod.internal", 5432, "app_user", "secret", "dfe_db");
//! assert!(url.to_url().starts_with("postgresql://"));
//!
//! // From env vars (reads POSTGRES_HOST, POSTGRES_PORT, etc.)
//! let url = PostgresUrl::from_env("POSTGRES");
//! ```
//!
//! # Config Cascade
//!
//! ```yaml
//! database:
//!   postgres:
//!     host: db.prod.internal
//!     port: 5432
//!     user: app_user
//!     password: secret
//!     db: dfe_db
//! ```

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};

use crate::SensitiveString;

/// Userinfo-encoding set: every byte that needs percent-encoding in
/// the `user:password` section of a URL. Per RFC 3986 the userinfo
/// production allows `unreserved / pct-encoded / sub-delims / ":"`,
/// but several sub-delims (`?`, `#`, `/`, `@`) would break parsing if
/// they appear before they're meant to. The safe set is everything
/// non-alphanumeric except the unreserved punctuation (`-._~`).
const USERINFO: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// Path-segment encoding set: same as userinfo for our purposes
/// (database names are sometimes treated as path components and
/// must not contain `/`, `?`, or `#`).
const PATH_SEGMENT: &AsciiSet = USERINFO;

/// Trait for database connection URL builders.
pub trait DatabaseUrl {
    /// Build the connection URL string.
    ///
    /// Password is included in the URL — use `.to_url()` only for passing
    /// to database drivers, never for logging. Use `Display` for safe output.
    fn to_url(&self) -> String;

    /// The database type name (for logging/metrics).
    fn db_type(&self) -> &'static str;
}

/// Common database connection fields.
///
/// `password` is a [`SensitiveString`] so its value is redacted in
/// `Debug` / `serde::Serialize` output — never use the field directly
/// in logs. Code that needs to round-trip a `DbConnection` through
/// `serde` (e.g. figment env overlays) MUST wrap the serialise/deserialise
/// in [`crate::expose_during`] so the value survives — that's the same
/// shape every other consumer needs for `SensitiveString` fields.
///
/// `Debug` is derived because `SensitiveString::Debug` already prints
/// `SensitiveString(***REDACTED***)`, so deriving Debug for the wrapper
/// no longer leaks the password.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbConnection {
    #[serde(default = "default_localhost")]
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub user: String,
    /// Password as a redacting type. The previous `String` typing meant
    /// any accidental Debug / serde dump of a `DbConnection` leaked the
    /// plaintext password.
    #[serde(default)]
    pub password: SensitiveString,
    #[serde(default)]
    pub db: String,
    /// Extra query parameters (e.g., `sslmode=require`).
    #[serde(default)]
    pub params: Option<String>,
}

fn default_localhost() -> String {
    "localhost".into()
}

impl DbConnection {
    fn from_env_with_defaults(prefix: &str, default_port: u16) -> Self {
        Self {
            host: std::env::var(format!("{prefix}_HOST")).unwrap_or_else(|_| "localhost".into()),
            port: std::env::var(format!("{prefix}_PORT"))
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default_port),
            user: std::env::var(format!("{prefix}_USER")).unwrap_or_default(),
            password: std::env::var(format!("{prefix}_PASSWORD"))
                .map(SensitiveString::from)
                .unwrap_or_default(),
            db: std::env::var(format!("{prefix}_DB")).unwrap_or_default(),
            params: std::env::var(format!("{prefix}_PARAMS")).ok(),
        }
    }

    /// Build a URL with the given scheme, percent-encoding each
    /// component so that special characters (`:`, `/`, `@`, `?`, `#`,
    /// `=`, `&`) in user/password/db don't break the parser. The
    /// previous string-interpolation shape silently corrupted URLs for
    /// any credential containing one of those bytes — and those bytes
    /// are common in generated/random passwords.
    fn url_with_scheme(&self, scheme: &str) -> String {
        let user_enc = utf8_percent_encode(&self.user, USERINFO);
        let pass_raw = self.password.expose();
        let pass_enc = utf8_percent_encode(pass_raw, USERINFO);

        let auth = if self.user.is_empty() && pass_raw.is_empty() {
            String::new()
        } else if pass_raw.is_empty() {
            format!("{user_enc}@")
        } else {
            format!("{user_enc}:{pass_enc}@")
        };

        let db_path = if self.db.is_empty() {
            String::new()
        } else {
            format!("/{}", utf8_percent_encode(&self.db, PATH_SEGMENT))
        };

        // Query parameters are passed through verbatim — callers are
        // expected to provide a properly-encoded `key=value&key2=value2`
        // string (typically a small fixed set like `sslmode=require`).
        let params = self
            .params
            .as_ref()
            .map(|p| format!("?{p}"))
            .unwrap_or_default();

        // IPv6 literals must be bracketed in URL host position
        // (RFC 3986 §3.2.2). Don't bracket what's already bracketed.
        let host_fmt = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };

        format!("{scheme}://{auth}{host_fmt}:{}{db_path}{params}", self.port)
    }
}

/// PostgreSQL connection URL builder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostgresUrl(pub DbConnection);

impl PostgresUrl {
    #[must_use]
    pub fn new(host: &str, port: u16, user: &str, password: &str, db: &str) -> Self {
        Self(DbConnection {
            host: host.into(),
            port,
            user: user.into(),
            password: password.into(),
            db: db.into(),
            params: None,
        })
    }

    /// Build from env vars: `{prefix}_HOST`, `{prefix}_PORT`, etc.
    #[must_use]
    pub fn from_env(prefix: &str) -> Self {
        Self(DbConnection::from_env_with_defaults(prefix, 5432))
    }

    /// Add query parameters (e.g., `sslmode=require`).
    #[must_use]
    pub fn with_params(mut self, params: &str) -> Self {
        self.0.params = Some(params.into());
        self
    }
}

impl DatabaseUrl for PostgresUrl {
    fn to_url(&self) -> String {
        self.0.url_with_scheme("postgresql")
    }

    fn db_type(&self) -> &'static str {
        "postgresql"
    }
}

/// ClickHouse HTTP connection URL builder (port 8123).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClickHouseUrl(pub DbConnection);

impl ClickHouseUrl {
    #[must_use]
    pub fn new(host: &str, port: u16, user: &str, password: &str, db: &str) -> Self {
        Self(DbConnection {
            host: host.into(),
            port,
            user: user.into(),
            password: password.into(),
            db: db.into(),
            params: None,
        })
    }

    /// Build from env vars with HTTP default port (8123).
    #[must_use]
    pub fn from_env(prefix: &str) -> Self {
        Self(DbConnection::from_env_with_defaults(prefix, 8123))
    }

    /// Build from env vars with native protocol default port (9000).
    #[must_use]
    pub fn from_env_native(prefix: &str) -> Self {
        Self(DbConnection::from_env_with_defaults(prefix, 9000))
    }
}

impl DatabaseUrl for ClickHouseUrl {
    fn to_url(&self) -> String {
        self.0.url_with_scheme("http")
    }

    fn db_type(&self) -> &'static str {
        "clickhouse"
    }
}

/// Redis/Valkey connection URL builder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisUrl(pub DbConnection);

impl RedisUrl {
    #[must_use]
    pub fn new(host: &str, port: u16, password: &str, db: &str) -> Self {
        Self(DbConnection {
            host: host.into(),
            port,
            user: String::new(),
            password: password.into(),
            db: db.into(),
            params: None,
        })
    }

    /// Build from env vars: `{prefix}_HOST`, `{prefix}_PORT`, etc.
    #[must_use]
    pub fn from_env(prefix: &str) -> Self {
        Self(DbConnection::from_env_with_defaults(prefix, 6379))
    }
}

impl DatabaseUrl for RedisUrl {
    fn to_url(&self) -> String {
        self.0.url_with_scheme("redis")
    }

    fn db_type(&self) -> &'static str {
        "redis"
    }
}

/// MongoDB connection URL builder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MongoUrl(pub DbConnection);

impl MongoUrl {
    #[must_use]
    pub fn new(host: &str, port: u16, user: &str, password: &str, db: &str) -> Self {
        Self(DbConnection {
            host: host.into(),
            port,
            user: user.into(),
            password: password.into(),
            db: db.into(),
            params: None,
        })
    }

    /// Build from env vars: `{prefix}_HOST`, `{prefix}_PORT`, etc.
    #[must_use]
    pub fn from_env(prefix: &str) -> Self {
        Self(DbConnection::from_env_with_defaults(prefix, 27017))
    }

    /// Add query parameters (e.g., `authSource=admin&replicaSet=rs0`).
    #[must_use]
    pub fn with_params(mut self, params: &str) -> Self {
        self.0.params = Some(params.into());
        self
    }
}

impl DatabaseUrl for MongoUrl {
    fn to_url(&self) -> String {
        self.0.url_with_scheme("mongodb")
    }

    fn db_type(&self) -> &'static str {
        "mongodb"
    }
}

/// Safe `Display` implementation — redacts password.
impl std::fmt::Display for PostgresUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "postgresql://{}:***@{}:{}/{}",
            self.0.user, self.0.host, self.0.port, self.0.db
        )
    }
}

impl std::fmt::Display for ClickHouseUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "http://{}:***@{}:{}/{}",
            self.0.user, self.0.host, self.0.port, self.0.db
        )
    }
}

impl std::fmt::Display for RedisUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "redis://***@{}:{}/{}",
            self.0.host, self.0.port, self.0.db
        )
    }
}

impl std::fmt::Display for MongoUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "mongodb://{}:***@{}:{}/{}",
            self.0.user, self.0.host, self.0.port, self.0.db
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgres_url_with_all_fields() {
        let url = PostgresUrl::new("db.prod", 5432, "app", "secret", "mydb");
        assert_eq!(url.to_url(), "postgresql://app:secret@db.prod:5432/mydb");
        assert_eq!(url.db_type(), "postgresql");
    }

    #[test]
    fn postgres_url_with_params() {
        let url = PostgresUrl::new("db.prod", 5432, "app", "secret", "mydb")
            .with_params("sslmode=require");
        assert_eq!(
            url.to_url(),
            "postgresql://app:secret@db.prod:5432/mydb?sslmode=require"
        );
    }

    #[test]
    fn postgres_url_no_password() {
        let url = PostgresUrl::new("db.prod", 5432, "app", "", "mydb");
        assert_eq!(url.to_url(), "postgresql://app@db.prod:5432/mydb");
    }

    #[test]
    fn postgres_url_no_auth() {
        let url = PostgresUrl::new("db.prod", 5432, "", "", "mydb");
        assert_eq!(url.to_url(), "postgresql://db.prod:5432/mydb");
    }

    #[test]
    fn postgres_display_redacts_password() {
        let url = PostgresUrl::new("db.prod", 5432, "app", "hunter2", "mydb");
        let display = format!("{url}");
        assert!(!display.contains("hunter2"));
        assert!(display.contains("***"));
    }

    #[test]
    fn clickhouse_http_url() {
        let url = ClickHouseUrl::new("ch.prod", 8123, "default", "secret", "dfe");
        assert_eq!(url.to_url(), "http://default:secret@ch.prod:8123/dfe");
        assert_eq!(url.db_type(), "clickhouse");
    }

    #[test]
    fn redis_url() {
        let url = RedisUrl::new("redis.prod", 6379, "secret", "0");
        assert_eq!(url.to_url(), "redis://:secret@redis.prod:6379/0");
        assert_eq!(url.db_type(), "redis");
    }

    #[test]
    fn redis_url_no_password() {
        let url = RedisUrl::new("redis.prod", 6379, "", "0");
        assert_eq!(url.to_url(), "redis://redis.prod:6379/0");
    }

    #[test]
    fn redis_display_redacts() {
        let url = RedisUrl::new("redis.prod", 6379, "secret123", "0");
        let display = format!("{url}");
        assert!(!display.contains("secret123"));
    }

    #[test]
    fn mongo_url() {
        let url = MongoUrl::new("mongo.prod", 27017, "admin", "secret", "mydb");
        assert_eq!(url.to_url(), "mongodb://admin:secret@mongo.prod:27017/mydb");
        assert_eq!(url.db_type(), "mongodb");
    }

    #[test]
    fn mongo_url_with_params() {
        let url = MongoUrl::new("mongo.prod", 27017, "admin", "secret", "mydb")
            .with_params("authSource=admin&replicaSet=rs0");
        assert_eq!(
            url.to_url(),
            "mongodb://admin:secret@mongo.prod:27017/mydb?authSource=admin&replicaSet=rs0"
        );
    }

    #[test]
    fn mongo_display_redacts() {
        let url = MongoUrl::new("mongo.prod", 27017, "admin", "hunter2", "mydb");
        let display = format!("{url}");
        assert!(!display.contains("hunter2"));
    }

    #[test]
    fn url_percent_encodes_password_with_special_chars() {
        // Real-world generated passwords routinely contain `@`, `/`, `:`,
        // `#`, `=`. Without encoding the URL parser misroutes them — the
        // `@` ends the userinfo early, the `/` starts the path early.
        let url = PostgresUrl::new("db.prod", 5432, "user", "p@ss/w:rd#1=2", "mydb");
        let s = url.to_url();
        // `@` -> %40, `/` -> %2F, `:` -> %3A, `#` -> %23, `=` -> %3D
        assert!(s.contains("p%40ss%2Fw%3Ard%231%3D2"), "got: {s}");
        // Userinfo terminator should be the SINGLE @ separating creds
        // from host — never a raw @ from the password.
        assert_eq!(s.matches('@').count(), 1, "got: {s}");
    }

    #[test]
    fn url_percent_encodes_user_with_special_chars() {
        let url = PostgresUrl::new("db", 5432, "user@example.com", "pw", "mydb");
        let s = url.to_url();
        assert!(s.contains("user%40example.com:pw@"), "got: {s}");
    }

    #[test]
    fn url_percent_encodes_db_with_special_chars() {
        // Database names containing `/` are uncommon but legal in some
        // engines (clickhouse multi-tenant prefixes, for example).
        let url = PostgresUrl::new("db", 5432, "u", "p", "tenant/db");
        let s = url.to_url();
        assert!(s.contains("/tenant%2Fdb"), "got: {s}");
    }

    #[test]
    fn debug_of_dbconnection_redacts_password() {
        let dbc = DbConnection {
            host: "db".into(),
            port: 5432,
            user: "u".into(),
            password: SensitiveString::new("the_real_secret"),
            db: "mydb".into(),
            params: None,
        };
        let debug = format!("{dbc:?}");
        assert!(!debug.contains("the_real_secret"), "debug leaked: {debug}");
        assert!(debug.contains("REDACTED"));
    }

    #[test]
    fn serialize_dbconnection_redacts_by_default() {
        let dbc = DbConnection {
            host: "db".into(),
            port: 5432,
            user: "u".into(),
            password: SensitiveString::new("the_real_secret"),
            db: "mydb".into(),
            params: None,
        };
        let json = serde_json::to_string(&dbc).unwrap();
        assert!(!json.contains("the_real_secret"));
        assert!(json.contains("REDACTED"));
    }

    #[test]
    fn round_trip_via_expose_during_preserves_password() {
        // Mirrors the dfe-loader figment cascade pattern. Without
        // expose_during the password becomes the literal REDACTED string.
        let dbc = DbConnection {
            host: "db".into(),
            port: 5432,
            user: "u".into(),
            password: SensitiveString::new("the_real_secret"),
            db: "mydb".into(),
            params: None,
        };
        let round_tripped: DbConnection = crate::expose_during(|| {
            let v = serde_json::to_value(&dbc).unwrap();
            serde_json::from_value(v).unwrap()
        });
        assert_eq!(round_tripped.password.expose(), "the_real_secret");
    }

    /// IPv6 literals get RFC 3986 brackets.
    #[test]
    fn ipv6_host_is_bracketed() {
        let dbc = DbConnection {
            host: "::1".into(),
            port: 5432,
            user: "u".into(),
            password: SensitiveString::new("p"),
            db: "d".into(),
            params: None,
        };
        let url = dbc.url_with_scheme("postgresql");
        assert!(url.contains("@[::1]:5432/"), "got: {url}");
    }

    /// Pre-bracketed host stays single-bracketed.
    #[test]
    fn pre_bracketed_ipv6_host_not_double_bracketed() {
        let dbc = DbConnection {
            host: "[fe80::1]".into(),
            port: 5432,
            user: "u".into(),
            password: SensitiveString::new("p"),
            db: "d".into(),
            params: None,
        };
        let url = dbc.url_with_scheme("postgresql");
        assert!(url.contains("@[fe80::1]:5432/"), "got: {url}");
        assert!(!url.contains("[[fe80::1]]"));
    }
}
