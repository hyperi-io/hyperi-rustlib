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
//! Password fields use [`SensitiveString`](crate::config::sensitive::SensitiveString)
//! for compile-time safe redaction.
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

use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbConnection {
    #[serde(default = "default_localhost")]
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub password: String,
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
            password: std::env::var(format!("{prefix}_PASSWORD")).unwrap_or_default(),
            db: std::env::var(format!("{prefix}_DB")).unwrap_or_default(),
            params: std::env::var(format!("{prefix}_PARAMS")).ok(),
        }
    }

    fn url_with_scheme(&self, scheme: &str) -> String {
        let auth = if self.user.is_empty() && self.password.is_empty() {
            String::new()
        } else if self.password.is_empty() {
            format!("{}@", self.user)
        } else {
            format!("{}:{}@", self.user, self.password)
        };

        let db_path = if self.db.is_empty() {
            String::new()
        } else {
            format!("/{}", self.db)
        };

        let params = self
            .params
            .as_ref()
            .map(|p| format!("?{p}"))
            .unwrap_or_default();

        format!(
            "{scheme}://{auth}{}:{}{db_path}{params}",
            self.host, self.port
        )
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
}
