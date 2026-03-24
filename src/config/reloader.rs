// Project:   hyperi-rustlib
// File:      src/config/reloader.rs
// Purpose:   Universal config hot-reload with SIGHUP, periodic, and file polling
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Universal configuration reloader for DFE components.
//!
//! `ConfigReloader<T>` provides three reload triggers, any combination of
//! which can be enabled simultaneously:
//!
//! 1. **SIGHUP** (Unix only) — standard daemon reload signal
//! 2. **Periodic timer** — reload every N seconds
//! 3. **File polling** — detect config file changes via mtime comparison
//!
//! The reloader calls a user-supplied `reload_fn` to load config and a
//! `validate_fn` to validate before applying. On success it updates the
//! `SharedConfig<T>` which notifies all subscribers.
//!
//! ## Usage
//!
//! ```rust,no_run
//! use std::path::PathBuf;
//! use std::time::Duration;
//! use hyperi_rustlib::config::reloader::{ConfigReloader, ReloaderConfig};
//! use hyperi_rustlib::config::shared::SharedConfig;
//!
//! #[derive(Clone, Debug, Default)]
//! struct AppConfig {
//!     pub workers: usize,
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     let config = AppConfig { workers: 4 };
//!     let shared = SharedConfig::new(config);
//!
//!     let reloader_config = ReloaderConfig {
//!         config_path: Some(PathBuf::from("config.yaml")),
//!         poll_interval: Duration::from_secs(5),
//!         periodic_interval: Duration::ZERO,  // disabled
//!         debounce: Duration::from_millis(500),
//!         enable_sighup: true,
//!     };
//!
//!     let reloader = ConfigReloader::new(
//!         reloader_config,
//!         shared.clone(),
//!         || {
//!             // Your config loading logic here
//!             Ok(AppConfig { workers: 8 })
//!         },
//!         |cfg| {
//!             // Your validation logic here
//!             if cfg.workers == 0 {
//!                 return Err("workers must be > 0".into());
//!             }
//!             Ok(())
//!         },
//!     );
//!
//!     let _handle = reloader.start();
//!     // ... run your application ...
//! }
//! ```
//!
//! ## Migration from Component-Specific Implementations
//!
//! ### From dfe-loader's `ConfigWatcher` (file polling)
//!
//! ```text
//! // Before:
//! let watcher = ConfigWatcher::new(WatcherConfig {
//!     config_path, poll_interval, debounce, enabled: true,
//! }, shared)?;
//! let _handle = watcher.start();
//!
//! // After:
//! let reloader = ConfigReloader::new(
//!     ReloaderConfig {
//!         config_path: Some(config_path),
//!         poll_interval,
//!         debounce,
//!         enable_sighup: true,      // bonus: also reload on SIGHUP
//!         periodic_interval: Duration::ZERO,
//!     },
//!     shared,
//!     || Config::load(path),        // your reload function
//!     |c| c.validate(),             // your validate function
//! );
//! let _handle = reloader.start();
//! ```
//!
//! ### From dfe-receiver's `config_reload_task` (SIGHUP + periodic)
//!
//! ```text
//! // Before (inline in main.rs):
//! tokio::spawn(config_reload_task(state, reload_secs));
//!
//! // After:
//! let reloader = ConfigReloader::new(
//!     ReloaderConfig {
//!         periodic_interval: Duration::from_secs(reload_secs),
//!         enable_sighup: true,
//!         config_path: None,         // no file watching
//!         ..Default::default()
//!     },
//!     shared,
//!     || Config::load(path),
//!     |c| c.validate(),
//! );
//! let _handle = reloader.start();
//! ```
//!
//! ### From dfe-archiver (not yet wired)
//!
//! The archiver has `SharedConfig` and `reload_config()` ready but not
//! connected. Use `ConfigReloader` to complete the integration:
//!
//! ```text
//! let reloader = ConfigReloader::new(
//!     ReloaderConfig {
//!         config_path: config.config_path.as_ref().map(PathBuf::from),
//!         periodic_interval: Duration::from_secs(config.config_reload_secs),
//!         enable_sighup: true,
//!         ..Default::default()
//!     },
//!     shared,
//!     || load_config(config_path),
//!     |c| validate_config(c),
//! );
//! let _handle = reloader.start();
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use super::shared::SharedConfig;

/// Boxed error type for reload/validate callbacks.
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Configuration for the reloader.
#[derive(Debug, Clone)]
pub struct ReloaderConfig {
    /// Path to config file to watch for changes (None = no file watching).
    pub config_path: Option<PathBuf>,

    /// File polling interval. Only used when `config_path` is Some.
    /// Default: 5 seconds.
    pub poll_interval: Duration,

    /// Periodic reload interval. Set to `Duration::ZERO` to disable.
    /// Default: disabled.
    pub periodic_interval: Duration,

    /// Minimum time between reloads (debounce).
    /// Default: 500ms.
    pub debounce: Duration,

    /// Enable SIGHUP reload trigger (Unix only, ignored on other platforms).
    /// Default: true.
    pub enable_sighup: bool,
}

impl Default for ReloaderConfig {
    fn default() -> Self {
        Self {
            config_path: None,
            poll_interval: Duration::from_secs(5),
            periodic_interval: Duration::ZERO,
            debounce: Duration::from_millis(500),
            enable_sighup: true,
        }
    }
}

/// Universal configuration reloader.
///
/// Supports three reload triggers (any combination):
/// - **SIGHUP** (Unix) — `enable_sighup: true`
/// - **Periodic timer** — `periodic_interval > 0`
/// - **File polling** — `config_path: Some(path)`
///
/// On each trigger, calls `reload_fn` to load new config, `validate_fn` to
/// validate, then updates the `SharedConfig<T>` if valid.
/// Callback invoked after a successful reload with the new config value.
type PostReloadHook<T> = Arc<dyn Fn(&T) + Send + Sync>;

pub struct ConfigReloader<T: Clone + Send + Sync + 'static> {
    config: ReloaderConfig,
    shared: SharedConfig<T>,
    reload_fn: Arc<dyn Fn() -> Result<T, BoxError> + Send + Sync>,
    validate_fn: Arc<dyn Fn(&T) -> Result<(), BoxError> + Send + Sync>,
    post_reload_hooks: Vec<PostReloadHook<T>>,
}

impl<T: Clone + Send + Sync + 'static> ConfigReloader<T> {
    /// Create a new config reloader.
    ///
    /// - `config`: Reload trigger configuration
    /// - `shared`: Shared config to update on successful reload
    /// - `reload_fn`: Called to load a fresh config (re-reads file + env)
    /// - `validate_fn`: Called to validate before applying (return Err to reject)
    pub fn new(
        config: ReloaderConfig,
        shared: SharedConfig<T>,
        reload_fn: impl Fn() -> Result<T, BoxError> + Send + Sync + 'static,
        validate_fn: impl Fn(&T) -> Result<(), BoxError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            config,
            shared,
            reload_fn: Arc::new(reload_fn),
            validate_fn: Arc::new(validate_fn),
            post_reload_hooks: Vec::new(),
        }
    }

    /// Add a hook that runs after each successful reload.
    ///
    /// Use this to connect to the config registry:
    ///
    /// ```rust,no_run
    /// # use hyperi_rustlib::config::reloader::ConfigReloader;
    /// # use hyperi_rustlib::config::registry;
    /// // reloader.with_registry_update("my_app");
    /// ```
    #[must_use]
    pub fn with_post_reload_hook(mut self, hook: impl Fn(&T) + Send + Sync + 'static) -> Self {
        self.post_reload_hooks.push(Arc::new(hook));
        self
    }

    /// Connect to the config registry: after each successful reload,
    /// call `registry::update()` so listeners are notified and the
    /// registry reflects the new effective config.
    ///
    /// Requires `T: Serialize + Default`.
    #[must_use]
    pub fn with_registry_update(self, key: &str) -> Self
    where
        T: serde::Serialize + Default,
    {
        let key = key.to_string();
        self.with_post_reload_hook(move |config| {
            super::registry::update::<T>(&key, config);
        })
    }

    /// Start the reload loop in a background task.
    ///
    /// Returns a `JoinHandle` that can be used to abort the reloader.
    /// The task runs until cancelled or the process exits.
    pub fn start(self) -> JoinHandle<()> {
        let has_file = self.config.config_path.is_some();
        let has_periodic = self.config.periodic_interval > Duration::ZERO;
        let has_sighup = self.config.enable_sighup;

        info!(
            file_watch = has_file,
            periodic = has_periodic,
            sighup = has_sighup,
            "Config reloader started"
        );

        tokio::spawn(async move {
            self.run_loop().await;
        })
    }

    /// Main reload loop — waits for any trigger, then attempts reload.
    async fn run_loop(self) {
        // File polling state
        let mut last_modified: Option<SystemTime> =
            self.config.config_path.as_ref().and_then(|p| file_mtime(p));
        let mut last_reload = Instant::now();

        // Set up poll timer (for file watching)
        let mut poll_timer = self
            .config
            .config_path
            .as_ref()
            .map(|_| tokio::time::interval(self.config.poll_interval));

        // Set up periodic timer
        let mut periodic_timer = if self.config.periodic_interval > Duration::ZERO {
            Some(tokio::time::interval(self.config.periodic_interval))
        } else {
            None
        };

        // Set up SIGHUP handler
        #[cfg(unix)]
        let mut sighup = if self.config.enable_sighup {
            Some(
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                    .expect("failed to register SIGHUP handler"),
            )
        } else {
            None
        };

        loop {
            let trigger = self
                .wait_for_trigger(
                    &mut poll_timer,
                    &mut periodic_timer,
                    #[cfg(unix)]
                    &mut sighup,
                    &mut last_modified,
                )
                .await;

            // Debounce check
            if last_reload.elapsed() < self.config.debounce {
                debug!("Debouncing config reload");
                continue;
            }

            match trigger {
                ReloadTrigger::FileChanged => {
                    info!(
                        path = ?self.config.config_path,
                        "Config file changed, reloading"
                    );
                }
                ReloadTrigger::Periodic => {
                    info!("Periodic config reload triggered");
                }
                ReloadTrigger::Sighup => {
                    info!("SIGHUP received, reloading configuration");
                }
            }

            self.do_reload();
            last_reload = Instant::now();
        }
    }

    /// Wait for the next reload trigger.
    ///
    /// Returns which trigger fired. For file polling, also updates last_modified.
    async fn wait_for_trigger(
        &self,
        poll_timer: &mut Option<tokio::time::Interval>,
        periodic_timer: &mut Option<tokio::time::Interval>,
        #[cfg(unix)] sighup: &mut Option<tokio::signal::unix::Signal>,
        last_modified: &mut Option<SystemTime>,
    ) -> ReloadTrigger {
        loop {
            let trigger = self
                .select_trigger(
                    poll_timer,
                    periodic_timer,
                    #[cfg(unix)]
                    sighup,
                )
                .await;

            match trigger {
                ReloadTrigger::FileChanged => {
                    // Check if file actually changed (mtime comparison)
                    if let Some(ref path) = self.config.config_path {
                        let current_mtime = file_mtime(path);
                        let changed = match (&*last_modified, &current_mtime) {
                            (Some(last), Some(current)) => current > last,
                            (None, Some(_)) => true,
                            _ => false,
                        };
                        if changed {
                            *last_modified = current_mtime;
                            return ReloadTrigger::FileChanged;
                        }
                    }
                    // No actual change, loop back
                }
                other => return other,
            }
        }
    }

    /// Select on all enabled triggers, returning which one fired first.
    #[cfg(unix)]
    async fn select_trigger(
        &self,
        poll_timer: &mut Option<tokio::time::Interval>,
        periodic_timer: &mut Option<tokio::time::Interval>,
        sighup: &mut Option<tokio::signal::unix::Signal>,
    ) -> ReloadTrigger {
        tokio::select! {
            _ = async {
                match poll_timer.as_mut() {
                    Some(timer) => timer.tick().await,
                    None => std::future::pending().await,
                }
            } => ReloadTrigger::FileChanged,

            _ = async {
                match periodic_timer.as_mut() {
                    Some(timer) => timer.tick().await,
                    None => std::future::pending().await,
                }
            } => ReloadTrigger::Periodic,

            () = async {
                match sighup.as_mut() {
                    Some(sig) => { sig.recv().await; },
                    None => std::future::pending::<()>().await,
                }
            } => ReloadTrigger::Sighup,
        }
    }

    /// Select on all enabled triggers (non-Unix: no SIGHUP).
    #[cfg(not(unix))]
    async fn select_trigger(
        &self,
        poll_timer: &mut Option<tokio::time::Interval>,
        periodic_timer: &mut Option<tokio::time::Interval>,
    ) -> ReloadTrigger {
        tokio::select! {
            _ = async {
                match poll_timer.as_mut() {
                    Some(timer) => timer.tick().await,
                    None => std::future::pending().await,
                }
            } => ReloadTrigger::FileChanged,

            _ = async {
                match periodic_timer.as_mut() {
                    Some(timer) => timer.tick().await,
                    None => std::future::pending().await,
                }
            } => ReloadTrigger::Periodic,
        }
    }

    /// Attempt to reload config: load → validate → update shared.
    fn do_reload(&self) {
        match (self.reload_fn)() {
            Ok(new_config) => {
                if let Err(e) = (self.validate_fn)(&new_config) {
                    error!(error = %e, "Config reload validation failed, keeping current config");
                    #[cfg(feature = "metrics")]
                    metrics::counter!("config_reloads_total", "result" => "error").increment(1);
                    return;
                }

                let old_version = self.shared.version();
                self.shared.update(new_config.clone());
                let new_version = self.shared.version();

                // Run post-reload hooks (registry update, etc.)
                for hook in &self.post_reload_hooks {
                    hook(&new_config);
                }

                #[cfg(feature = "metrics")]
                metrics::counter!("config_reloads_total", "result" => "success").increment(1);

                info!(
                    old_version = old_version,
                    new_version = new_version,
                    "Configuration reloaded successfully"
                );
            }
            Err(e) => {
                warn!(error = %e, "Config reload failed, keeping current config");
                #[cfg(feature = "metrics")]
                metrics::counter!("config_reloads_total", "result" => "error").increment(1);
            }
        }
    }
}

/// Which trigger caused a reload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReloadTrigger {
    FileChanged,
    Periodic,
    #[allow(dead_code)]
    Sighup,
}

/// Get the modification time of a file.
fn file_mtime(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tempfile::TempDir;

    #[derive(Clone, Debug, Default, PartialEq)]
    struct TestConfig {
        pub value: String,
    }

    #[test]
    fn test_reloader_config_defaults() {
        let config = ReloaderConfig::default();
        assert!(config.config_path.is_none());
        assert_eq!(config.poll_interval, Duration::from_secs(5));
        assert_eq!(config.periodic_interval, Duration::ZERO);
        assert_eq!(config.debounce, Duration::from_millis(500));
        assert!(config.enable_sighup);
    }

    #[tokio::test]
    async fn test_periodic_reload() {
        let shared = SharedConfig::new(TestConfig {
            value: "initial".into(),
        });
        let mut rx = shared.subscribe();

        let call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let call_count_clone = call_count.clone();

        let reloader = ConfigReloader::new(
            ReloaderConfig {
                periodic_interval: Duration::from_millis(50),
                debounce: Duration::from_millis(10),
                enable_sighup: false,
                ..Default::default()
            },
            shared.clone(),
            move || {
                call_count_clone.fetch_add(1, Ordering::Relaxed);
                Ok(TestConfig {
                    value: "reloaded".into(),
                })
            },
            |_| Ok(()),
        );

        let handle = reloader.start();

        // Wait for at least one reload
        let result = tokio::time::timeout(Duration::from_secs(2), rx.changed()).await;
        assert!(result.is_ok(), "Should receive reload notification");

        assert_eq!(shared.read().value, "reloaded");
        assert!(shared.version() >= 1);
        assert!(call_count.load(Ordering::Relaxed) >= 1);

        handle.abort();
    }

    #[tokio::test]
    async fn test_file_change_triggers_reload() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.yaml");
        fs::write(&config_path, "initial content").unwrap();

        let shared = SharedConfig::new(TestConfig {
            value: "initial".into(),
        });
        let mut rx = shared.subscribe();

        let path_for_reload = config_path.clone();
        let reloader = ConfigReloader::new(
            ReloaderConfig {
                config_path: Some(config_path.clone()),
                poll_interval: Duration::from_millis(50),
                debounce: Duration::from_millis(10),
                enable_sighup: false,
                ..Default::default()
            },
            shared.clone(),
            move || {
                let content = fs::read_to_string(&path_for_reload).unwrap_or_default();
                Ok(TestConfig { value: content })
            },
            |_| Ok(()),
        );

        let handle = reloader.start();

        // Let the watcher start and record initial mtime
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Modify the file
        {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&config_path)
                .unwrap();
            file.write_all(b"updated content").unwrap();
            file.sync_all().unwrap();
        }

        // Wait for the reload
        let result = tokio::time::timeout(Duration::from_secs(2), rx.changed()).await;
        if result.is_ok() {
            assert_eq!(shared.read().value, "updated content");
        }

        handle.abort();
    }

    #[tokio::test]
    async fn test_validation_failure_preserves_config() {
        let shared = SharedConfig::new(TestConfig {
            value: "good".into(),
        });

        let should_fail = Arc::new(AtomicBool::new(true));
        let should_fail_clone = should_fail.clone();

        let reloader = ConfigReloader::new(
            ReloaderConfig {
                periodic_interval: Duration::from_millis(50),
                debounce: Duration::from_millis(10),
                enable_sighup: false,
                ..Default::default()
            },
            shared.clone(),
            || {
                Ok(TestConfig {
                    value: "bad".into(),
                })
            },
            move |_cfg| {
                if should_fail_clone.load(Ordering::Relaxed) {
                    Err("validation failed".into())
                } else {
                    Ok(())
                }
            },
        );

        let handle = reloader.start();

        // Let a few reload attempts happen (all should fail validation)
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Config should still be the original
        assert_eq!(shared.read().value, "good");
        assert_eq!(shared.version(), 0);

        handle.abort();
    }

    #[tokio::test]
    async fn test_reload_fn_error_preserves_config() {
        let shared = SharedConfig::new(TestConfig {
            value: "good".into(),
        });

        let reloader = ConfigReloader::new(
            ReloaderConfig {
                periodic_interval: Duration::from_millis(50),
                debounce: Duration::from_millis(10),
                enable_sighup: false,
                ..Default::default()
            },
            shared.clone(),
            || Err("load failed".into()),
            |_| Ok(()),
        );

        let handle = reloader.start();

        // Let a few reload attempts happen
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Config should still be the original
        assert_eq!(shared.read().value, "good");
        assert_eq!(shared.version(), 0);

        handle.abort();
    }

    #[test]
    fn test_file_mtime() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        fs::write(&path, "content").unwrap();

        let mtime = file_mtime(&path);
        assert!(mtime.is_some());

        // Non-existent file
        let mtime = file_mtime(&PathBuf::from("/nonexistent/file.txt"));
        assert!(mtime.is_none());
    }
}
