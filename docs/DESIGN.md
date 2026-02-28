# Design Document - hyperi-rustlib

## Overview

`hyperi-rustlib` is a shared utility library for Rust applications, providing configuration management, structured logging, Prometheus metrics, and environment detection. It is the Rust equivalent of `hyperi-pylib` (Python) and `hyperi-golib` (Go).

---

## Design Principles

1. **Prefer existing crates over bespoke code** - Use well-maintained, performant Rust libraries
2. **Zero-configuration defaults** - Works out of the box with sensible defaults
3. **Container-aware** - Automatic detection and adaptation for K8s/Docker/bare metal
4. **Parity with siblings** - Behaviour matches hyperi-pylib and hyperi-golib where applicable
5. **Idiomatic Rust** - Follow Rust conventions (Result types, traits, builders)

---

## Crate Selection

| Component | Crate | Version | Rationale |
| --------- | ----- | ------- | --------- |
| Config | `figment` | 0.10 | Hierarchical config with serde, supports CLI/ENV/files |
| Config (env) | `dotenvy` | 0.15 | `.env` file loading |
| Logger | `tracing` | 0.1 | Industry standard, async-friendly |
| Logger (subscriber) | `tracing-subscriber` | 0.3 | JSON/text formatters, filtering |
| Metrics | `metrics` | 0.23 | Simple API, widely adopted |
| Metrics (exporter) | `metrics-exporter-prometheus` | 0.15 | Built-in Prometheus exporter |
| HTTP (P2) | `reqwest` | 0.12 | De facto standard HTTP client |
| HTTP retry (P2) | `reqwest-middleware` + `reqwest-retry` | 0.3 | Retry middleware |
| Serialisation | `serde` + `serde_json` + `serde_yaml` | 1.0 | Universal serialisation |
| CLI integration | `clap` | 4.5 | For config CLI arg layer |
| Async runtime | `tokio` | 1.0 | For metrics server, async features |
| Process info | `sysinfo` | 0.32 | Process metrics (CPU, memory) |
| TTY detection | `atty` | 0.2 | Logger format auto-detection |
| Colours | `owo-colors` | 4.0 | Terminal colours (Solarised) |

---

## Module Structure

```text
hyperi_rustlib/
├── Cargo.toml
├── src/
│   ├── lib.rs              # Public API exports
│   ├── env.rs              # Environment detection
│   ├── runtime.rs          # Runtime paths (XDG/container)
│   ├── config/
│   │   ├── mod.rs          # Config cascade
│   │   └── merge.rs        # File merging utilities
│   ├── logger/
│   │   ├── mod.rs          # Logger setup
│   │   ├── format.rs       # JSON/Text formatters
│   │   └── masking.rs      # Sensitive data filter
│   ├── metrics/
│   │   ├── mod.rs          # MetricsManager
│   │   ├── process.rs      # Process metrics
│   │   └── container.rs    # Container/cgroup metrics
│   ├── http.rs             # HTTP client (P2)
│   ├── database.rs         # DB URL builders (P2)
│   └── cache.rs            # Disk cache (P2)
├── tests/
│   ├── common/             # Shared test fixtures and utilities
│   ├── parity/             # Cross-implementation tests (vs Go/Python)
│   ├── integration/        # Docker/K8s/external service tests
│   └── e2e/                # Full pipeline tests
└── benches/                # Criterion benchmarks
```

---

## Component Designs

### 1. Environment Detection (`env` module)

```rust
/// Runtime environment types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Kubernetes,
    Docker,
    Container,  // Generic container (not K8s or Docker)
    BareMetal,
}

impl Environment {
    /// Detect current runtime environment
    pub fn detect() -> Self { ... }

    /// Check if running in any container
    pub fn is_container(&self) -> bool { ... }

    /// Check if running in Kubernetes
    pub fn is_kubernetes(&self) -> bool { ... }
}

/// Check if deployed via Helm
pub fn is_helm() -> bool { ... }
```

**Detection Logic (priority order):**

1. K8s service account token exists (`/var/run/secrets/kubernetes.io/serviceaccount/token`)
2. K8s env vars present (`KUBERNETES_SERVICE_HOST`)
3. Docker: `/.dockerenv` exists
4. Container: cgroups contain `/docker/` or `/kubepods/`
5. Default: BareMetal

### 2. Runtime Paths (`runtime` module)

```rust
/// Standard application paths based on environment
#[derive(Debug, Clone)]
pub struct RuntimePaths {
    pub config_dir: PathBuf,   // Read-only config
    pub secrets_dir: PathBuf,  // Read-only secrets
    pub data_dir: PathBuf,     // Persistent data
    pub temp_dir: PathBuf,     // Ephemeral temp
    pub logs_dir: PathBuf,     // Application logs
    pub cache_dir: PathBuf,    // Cache files
}

impl RuntimePaths {
    /// Discover paths based on detected environment
    pub fn discover() -> Self { ... }

    /// Discover with explicit environment
    pub fn discover_for(env: Environment) -> Self { ... }
}
```

**Path Resolution:**

| Environment | config_dir | secrets_dir | data_dir |
| ----------- | ---------- | ----------- | -------- |
| Kubernetes | `/app/config` | `/app/secrets` | `/app/data` |
| Docker | `/app/config` | `/app/secrets` | `/app/data` |
| BareMetal | `$XDG_CONFIG_HOME/{app}` | `~/.{app}/secrets` | `$XDG_DATA_HOME/{app}` |

### 3. Configuration (`config` module)

```rust
/// Configuration manager with 7-layer cascade
pub struct Config {
    inner: Figment,
}

/// Configuration options
#[derive(Default)]
pub struct ConfigOptions {
    pub env_prefix: String,         // e.g., "MYAPP"
    pub app_env: Option<String>,    // Override APP_ENV detection
    pub app_name: Option<String>,   // For ~/.config/{app_name}/ discovery
    pub config_paths: Vec<PathBuf>, // Additional config paths
}

impl Config {
    /// Create new config with options
    pub fn new(opts: ConfigOptions) -> Result<Self, ConfigError> { ... }

    /// Load configuration (applies all cascade layers)
    pub fn load(&mut self) -> Result<(), ConfigError> { ... }

    // Typed getters
    pub fn get_string(&self, key: &str) -> Option<String> { ... }
    pub fn get_int(&self, key: &str) -> Option<i64> { ... }
    pub fn get_bool(&self, key: &str) -> Option<bool> { ... }
    pub fn get_duration(&self, key: &str) -> Option<Duration> { ... }

    /// Get scoped sub-configuration
    pub fn sub(&self, key: &str) -> Option<Config> { ... }

    /// Deserialise into typed struct
    pub fn unmarshal<T: DeserializeOwned>(&self) -> Result<T, ConfigError> { ... }
    pub fn unmarshal_key<T: DeserializeOwned>(&self, key: &str) -> Result<T, ConfigError> { ... }
}

// Global singleton
static CONFIG: OnceLock<Config> = OnceLock::new();

pub fn setup(opts: ConfigOptions) -> Result<(), ConfigError> { ... }
pub fn get() -> &'static Config { ... }
```

**7-Layer Cascade (highest priority first):**

```rust
Figment::new()
    // 7. Hard-coded defaults (lowest)
    .merge(Serialized::defaults(defaults))
    // 6. defaults.yaml
    .merge(Yaml::file("defaults.yaml"))
    // 5. settings.yaml
    .merge(Yaml::file("settings.yaml"))
    // 4. settings.{env}.yaml
    .merge(Yaml::file(format!("settings.{}.yaml", app_env)))
    // 3. .env file
    .merge(Env::prefixed("").only(&dotenv_keys))
    // 2. Environment variables
    .merge(Env::prefixed(&env_prefix).split("_"))
    // 1. CLI args (highest)
    .merge(Serialized::defaults(cli_args))
```

### 4. Logger (`logger` module)

```rust
/// Log output format
#[derive(Debug, Clone, Copy, Default)]
pub enum LogFormat {
    Json,
    Text,
    #[default]
    Auto,  // JSON in containers, Text on TTY
}

/// Logger configuration
#[derive(Debug, Clone)]
pub struct LoggerOptions {
    pub level: tracing::Level,
    pub format: LogFormat,
    pub add_source: bool,           // Include file:line
    pub enable_masking: bool,       // Mask sensitive fields
    pub sensitive_fields: Vec<String>,
}

impl Default for LoggerOptions {
    fn default() -> Self {
        Self {
            level: tracing::Level::INFO,
            format: LogFormat::Auto,
            add_source: true,
            enable_masking: true,
            sensitive_fields: default_sensitive_fields(),
        }
    }
}

/// Default sensitive field names
pub fn default_sensitive_fields() -> Vec<String> {
    vec![
        "password", "passwd", "pwd",
        "secret", "token", "api_key", "apikey",
        "auth", "authorization", "bearer",
        "credential", "private_key", "privatekey",
        "access_key", "secret_key",
        "client_secret", "refresh_token",
    ].into_iter().map(String::from).collect()
}

/// Initialise global logger
pub fn setup(opts: LoggerOptions) -> Result<(), LoggerError> { ... }

/// Initialise with defaults (respects LOG_LEVEL, LOG_FORMAT, NO_COLOR env vars)
pub fn setup_default() -> Result<(), LoggerError> { ... }
```

**JSON Output Format (RFC 3339 with timezone):**

```json
{
  "timestamp": "2025-01-20T14:30:00.123+11:00",
  "level": "INFO",
  "target": "myapp::handler",
  "message": "Request processed",
  "fields": {
    "user_id": 123,
    "request_id": "abc-123"
  },
  "file": "src/handler.rs",
  "line": 42
}
```

**Sensitive Data Masking:**

Fields matching sensitive patterns are replaced with `"[REDACTED]"` in log output.

### 5. Metrics (`metrics` module)

```rust
/// Metrics manager
pub struct MetricsManager {
    registry: Registry,
    config: MetricsConfig,
    server_handle: Option<JoinHandle<()>>,
}

/// Metrics configuration
#[derive(Debug, Clone)]
pub struct MetricsConfig {
    pub namespace: String,
    pub enable_process_metrics: bool,
    pub enable_container_metrics: bool,
    pub update_interval: Duration,
}

impl MetricsManager {
    /// Create with namespace
    pub fn new(namespace: &str) -> Self { ... }

    /// Create with custom config
    pub fn with_config(config: MetricsConfig) -> Self { ... }

    // Metric creators
    pub fn counter(&self, name: &str, description: &str) -> Counter { ... }
    pub fn gauge(&self, name: &str, description: &str) -> Gauge { ... }
    pub fn histogram(&self, name: &str, description: &str, buckets: &[f64]) -> Histogram { ... }

    /// Get HTTP handler for /metrics endpoint
    pub fn handler(&self) -> impl Fn(Request) -> Response { ... }

    /// Start standalone metrics server
    pub async fn start_server(&mut self, addr: &str) -> Result<(), MetricsError> { ... }

    /// Stop server gracefully
    pub async fn stop_server(&mut self) -> Result<(), MetricsError> { ... }

    /// Start background metric collection
    pub fn start_auto_update(&self) { ... }

    /// Manual update
    pub fn update(&self) { ... }
}

// Helper functions
pub fn latency_buckets() -> Vec<f64> {
    vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0]
}

pub fn size_buckets() -> Vec<f64> {
    vec![100.0, 1000.0, 10000.0, 100000.0, 1000000.0, 10000000.0]
}
```

**Process Metrics:**

| Metric | Type | Description |
| ------ | ---- | ----------- |
| `{ns}_process_cpu_seconds_total` | Counter | Total CPU time |
| `{ns}_process_resident_memory_bytes` | Gauge | RSS memory |
| `{ns}_process_open_fds` | Gauge | Open file descriptors |
| `{ns}_process_start_time_seconds` | Gauge | Process start time |

**Container Metrics (cgroups):**

| Metric | Type | Description |
| ------ | ---- | ----------- |
| `{ns}_container_memory_limit_bytes` | Gauge | Memory limit from cgroup |
| `{ns}_container_memory_usage_bytes` | Gauge | Current memory usage |
| `{ns}_container_cpu_limit_cores` | Gauge | CPU cores limit |

**Standalone Server Endpoints:**

- `GET /metrics` - Prometheus metrics
- `GET /healthz` - Liveness probe (always 200)
- `GET /readyz` - Readiness probe (200 if healthy)

---

## Feature Flags

```toml
[features]
default = ["config", "logger", "metrics", "env"]

# Core features
config = ["figment", "dotenvy", "serde_yaml"]
logger = ["tracing", "tracing-subscriber"]
metrics = ["metrics", "metrics-exporter-prometheus", "sysinfo"]
env = []

# Extended features (P2)
http = ["reqwest", "reqwest-middleware", "reqwest-retry"]
database = []
cache = ["cached"]

# Optional integrations
tokio = ["tokio/full"]  # For metrics server
```

---

## Error Handling

Each module defines its own error type using `thiserror`:

```rust
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to load config file '{path}': {source}")]
    LoadError { path: PathBuf, #[source] source: figment::Error },

    #[error("missing required key: {0}")]
    MissingKey(String),

    #[error("invalid value for '{key}': {reason}")]
    InvalidValue { key: String, reason: String },
}
```

Top-level re-export:

```rust
pub use config::ConfigError;
pub use logger::LoggerError;
pub use metrics::MetricsError;
```

---

## Testing Strategy

### Unit Tests

Each module has `#[cfg(test)]` tests covering:

- Happy path functionality
- Error cases
- Edge cases (empty inputs, malformed data)

### Parity Tests

Located in `tests/parity/`, these compare Rust output against Go/Python:

```rust
#[test]
fn test_config_cascade_parity() {
    // Set up identical config files and env vars
    // Compare loaded values against known Go output
}

#[test]
fn test_logger_json_format_parity() {
    // Compare JSON log format against Go logger output
}

#[test]
fn test_metrics_exposition_parity() {
    // Compare /metrics output format
}
```

### Integration Tests

Located in `tests/integration/`:

- Docker container deployment
- K8s deployment (if applicable)
- Metrics scraping by Prometheus

---

## Usage Examples

### Basic Setup

```rust
use hyperi_rustlib::{config, logger, metrics, env};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Detect environment
    let env = env::Environment::detect();
    println!("Running in: {:?}", env);

    // Setup logger (respects LOG_LEVEL env var)
    logger::setup_default()?;

    // Load config with 7-layer cascade
    config::setup(config::ConfigOptions {
        env_prefix: "MYAPP".into(),
        ..Default::default()
    })?;

    let cfg = config::get();
    let db_host = cfg.get_string("database.host").unwrap_or_default();

    // Setup metrics
    let metrics = metrics::MetricsManager::new("myapp");
    let request_count = metrics.counter("requests_total", "Total requests");

    // Use tracing macros
    tracing::info!(db_host = %db_host, "Application started");

    Ok(())
}
```

### With Metrics Server

```rust
use hyperi_rustlib::metrics::MetricsManager;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut metrics = MetricsManager::new("myapp");

    // Start metrics server on :9090
    metrics.start_server("0.0.0.0:9090").await?;
    metrics.start_auto_update();

    // Application logic...

    // Graceful shutdown
    metrics.stop_server().await?;
    Ok(())
}
```

### Typed Config

```rust
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct DatabaseConfig {
    host: String,
    port: u16,
    username: String,
    password: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    config::setup(config::ConfigOptions::default())?;

    let db: DatabaseConfig = config::get().unmarshal_key("database")?;
    println!("Connecting to {}:{}", db.host, db.port);

    Ok(())
}
```

---

## API Compatibility

The API is designed to match hyperi-golib patterns where possible:

| Go (hyperi-golib) | Rust (hyperi-rustlib) |
| ------------- | ----------------- |
| `config.New(opts)` | `Config::new(opts)` |
| `config.Load()` | `config.load()` |
| `config.G()` | `config::get()` |
| `config.GetString("key")` | `config.get_string("key")` |
| `logger.Setup(opts)` | `logger::setup(opts)` |
| `logger.Info("msg", "key", val)` | `tracing::info!(key = val, "msg")` |
| `metrics.New(ns)` | `MetricsManager::new(ns)` |
| `metrics.Counter()` | `metrics.counter()` |
| `env.Detect()` | `Environment::detect()` |

---

## Future Considerations (P2)

### HTTP Client

```rust
pub struct HttpClient {
    inner: reqwest::Client,
    base_url: Option<Url>,
    timeout: Duration,
}

impl HttpClient {
    pub fn new() -> Self { ... }
    pub fn with_base_url(url: &str) -> Self { ... }
    pub fn with_timeout(timeout: Duration) -> Self { ... }

    // Automatic retry with exponential backoff
    pub async fn get(&self, url: &str) -> Result<Response, HttpError> { ... }
    pub async fn post<T: Serialize>(&self, url: &str, body: &T) -> Result<Response, HttpError> { ... }
}
```

### Database URL Builders

```rust
pub enum DatabaseType {
    PostgreSQL,
    MySQL,
    ClickHouse,
    Redis,
    MongoDB,
}

pub fn build_database_url(db_type: DatabaseType) -> Result<String, DatabaseError> {
    // Reads {DB_TYPE}_HOST, {DB_TYPE}_PORT, etc. from environment
}
```

---

**Last Updated:** 2025-12-24
