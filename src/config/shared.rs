// Project:   hyperi-rustlib
// File:      src/config/shared.rs
// Purpose:   Thread-safe shared configuration with hot-reload support
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Generic thread-safe shared configuration with version tracking.
//!
//! `SharedConfig<T>` wraps any config struct in an `Arc<RwLock<T>>` with a
//! monotonic version counter and a tokio watch channel for subscriber
//! notifications. This is the universal building block for hot-reload across
//! all DFE components (loader, receiver, archiver).
//!
//! ## Usage
//!
//! ```rust
//! use hyperi_rustlib::config::shared::SharedConfig;
//!
//! #[derive(Clone, Debug, Default)]
//! struct AppConfig {
//!     pub buffer_size: usize,
//!     pub log_level: String,
//! }
//!
//! // Create shared config
//! let shared = SharedConfig::new(AppConfig {
//!     buffer_size: 1024,
//!     log_level: "info".into(),
//! });
//!
//! // Read config (zero-copy via read guard)
//! {
//!     let cfg = shared.read();
//!     assert_eq!(cfg.buffer_size, 1024);
//! }
//!
//! // Subscribe to changes
//! let mut rx = shared.subscribe();
//!
//! // Update config (notifies all subscribers)
//! let mut new_cfg = shared.get();
//! new_cfg.buffer_size = 2048;
//! shared.update(new_cfg);
//!
//! assert_eq!(shared.version(), 1);
//! assert_eq!(*rx.borrow(), 1);
//! ```
//!
//! ## Migration from Component-Specific Implementations
//!
//! All DFE components previously had their own `SharedConfig` hard-coded to
//! their specific `Config` struct. This generic version is a drop-in
//! replacement:
//!
//! ```text
//! // Before (component-specific):
//! use crate::config::SharedConfig;           // hard-coded to crate::Config
//!
//! // After (generic from rustlib):
//! use hyperi_rustlib::config::SharedConfig;   // SharedConfig<Config>
//! let shared = SharedConfig::new(config);     // type inferred from argument
//! ```
//!
//! ### API Compatibility
//!
//! | Component Method | rustlib Equivalent | Notes |
//! |------------------|--------------------|-------|
//! | `read()` | `read()` | Returns `RwLockReadGuard` |
//! | `get()` | `get()` | Clones current config |
//! | `with(f)` | `with(f)` | Closure-based read |
//! | `update(cfg)` | `update(cfg)` | Write + version bump + notify |
//! | `version()` | `version()` | Atomic version counter |
//! | `subscribe()` | `subscribe()` | `watch::Receiver<u64>` |
//! | `clone_inner()` | Removed | Use `Clone` on `SharedConfig` instead |

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::watch;
use tracing::debug;

/// Thread-safe shared configuration with version tracking and change
/// notification.
///
/// Designed for hot-reload: components subscribe to config changes via the
/// watch channel and react accordingly. The version counter provides a cheap
/// way to detect whether config has changed since last check.
///
/// `T` must be `Clone` (for `get()`), `Send + Sync` (for cross-thread access),
/// and `'static` (for use in async tasks).
pub struct SharedConfig<T> {
    inner: Arc<RwLock<T>>,
    version: Arc<AtomicU64>,
    watch_tx: Arc<watch::Sender<u64>>,
    watch_rx: watch::Receiver<u64>,
}

impl<T: Clone + Send + Sync + 'static> SharedConfig<T> {
    /// Create a new shared config from an initial value.
    ///
    /// Version starts at 0. First `update()` bumps to 1.
    #[must_use]
    pub fn new(config: T) -> Self {
        let (watch_tx, watch_rx) = watch::channel(0);

        Self {
            inner: Arc::new(RwLock::new(config)),
            version: Arc::new(AtomicU64::new(0)),
            watch_tx: Arc::new(watch_tx),
            watch_rx,
        }
    }

    /// Read the current configuration (zero-copy).
    ///
    /// Returns a read guard that holds the lock. Multiple readers can hold
    /// the lock simultaneously. Prefer this over `get()` when you don't need
    /// to hold the config across await points.
    #[inline]
    pub fn read(&self) -> parking_lot::RwLockReadGuard<'_, T> {
        self.inner.read()
    }

    /// Get a clone of the current configuration.
    ///
    /// Use when you need to hold the config across await points or pass it
    /// to other functions. For read-only access in synchronous code, prefer
    /// `read()` or `with()`.
    #[must_use]
    pub fn get(&self) -> T {
        self.inner.read().clone()
    }

    /// Access configuration via a closure (avoids cloning).
    ///
    /// The closure receives a reference to the current config while holding
    /// the read lock. Useful for extracting a few fields without cloning the
    /// entire struct.
    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        let guard = self.inner.read();
        f(&guard)
    }

    /// Get the current config version.
    ///
    /// Version is 0 after creation, incremented by 1 on each `update()`.
    /// Monotonically increasing — never decreases.
    #[inline]
    #[must_use]
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// Update the configuration atomically.
    ///
    /// This will:
    /// 1. Acquire write lock and replace the config
    /// 2. Increment the version counter
    /// 3. Notify all subscribers via the watch channel
    pub fn update(&self, new_config: T) {
        {
            let mut guard = self.inner.write();
            *guard = new_config;
        }

        let new_version = self.version.fetch_add(1, Ordering::AcqRel) + 1;

        // Notify subscribers (ignore error if no receivers)
        let _ = self.watch_tx.send(new_version);

        debug!(version = new_version, "Configuration updated");
    }

    /// Subscribe to configuration changes.
    ///
    /// Returns a `watch::Receiver<u64>` that yields the new version number
    /// on each config update. Use `rx.changed().await` to wait for the next
    /// change.
    ///
    /// ```ignore
    /// let mut rx = shared.subscribe();
    /// loop {
    ///     rx.changed().await.unwrap();
    ///     let version = *rx.borrow();
    ///     let config = shared.get();
    ///     // React to config change...
    /// }
    /// ```
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.watch_rx.clone()
    }
}

impl<T: Clone + Send + Sync + 'static> Clone for SharedConfig<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            version: self.version.clone(),
            watch_tx: self.watch_tx.clone(),
            watch_rx: self.watch_rx.clone(),
        }
    }
}

impl<T: Clone + Send + Sync + Default + 'static> Default for SharedConfig<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: Clone + Send + Sync + 'static> std::fmt::Debug for SharedConfig<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedConfig")
            .field("version", &self.version())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Default, PartialEq)]
    struct TestConfig {
        pub name: String,
        pub value: u64,
    }

    #[test]
    fn test_new_starts_at_version_zero() {
        let shared = SharedConfig::new(TestConfig::default());
        assert_eq!(shared.version(), 0);
    }

    #[test]
    fn test_read_returns_initial_value() {
        let cfg = TestConfig {
            name: "test".into(),
            value: 42,
        };
        let shared = SharedConfig::new(cfg.clone());

        let guard = shared.read();
        assert_eq!(guard.name, "test");
        assert_eq!(guard.value, 42);
    }

    #[test]
    fn test_get_clones_current_config() {
        let cfg = TestConfig {
            name: "original".into(),
            value: 1,
        };
        let shared = SharedConfig::new(cfg);

        let got = shared.get();
        assert_eq!(got.name, "original");
    }

    #[test]
    fn test_with_closure_access() {
        let cfg = TestConfig {
            name: "closure".into(),
            value: 99,
        };
        let shared = SharedConfig::new(cfg);

        let val = shared.with(|c| c.value);
        assert_eq!(val, 99);
    }

    #[test]
    fn test_update_increments_version() {
        let shared = SharedConfig::new(TestConfig::default());

        assert_eq!(shared.version(), 0);

        shared.update(TestConfig {
            name: "v1".into(),
            value: 1,
        });
        assert_eq!(shared.version(), 1);

        shared.update(TestConfig {
            name: "v2".into(),
            value: 2,
        });
        assert_eq!(shared.version(), 2);
    }

    #[test]
    fn test_update_changes_visible() {
        let shared = SharedConfig::new(TestConfig::default());

        shared.update(TestConfig {
            name: "updated".into(),
            value: 100,
        });

        assert_eq!(shared.read().name, "updated");
        assert_eq!(shared.read().value, 100);
    }

    #[tokio::test]
    async fn test_subscribe_receives_notification() {
        let shared = SharedConfig::new(TestConfig::default());
        let mut rx = shared.subscribe();

        assert_eq!(*rx.borrow(), 0);

        shared.update(TestConfig {
            name: "notify".into(),
            value: 1,
        });

        rx.changed().await.expect("should receive change");
        assert_eq!(*rx.borrow(), 1);
    }

    #[tokio::test]
    async fn test_multiple_subscribers_all_notified() {
        let shared = SharedConfig::new(TestConfig::default());

        let mut rx1 = shared.subscribe();
        let mut rx2 = shared.subscribe();
        let mut rx3 = shared.subscribe();

        shared.update(TestConfig {
            name: "multi".into(),
            value: 1,
        });

        rx1.changed().await.expect("subscriber 1");
        rx2.changed().await.expect("subscriber 2");
        rx3.changed().await.expect("subscriber 3");

        assert_eq!(*rx1.borrow(), 1);
        assert_eq!(*rx2.borrow(), 1);
        assert_eq!(*rx3.borrow(), 1);
    }

    #[test]
    fn test_clone_shares_state() {
        let shared = SharedConfig::new(TestConfig::default());
        let cloned = shared.clone();

        shared.update(TestConfig {
            name: "from-original".into(),
            value: 1,
        });

        // Clone sees the update
        assert_eq!(cloned.read().name, "from-original");
        assert_eq!(cloned.version(), 1);

        // Update from clone is visible on original
        cloned.update(TestConfig {
            name: "from-clone".into(),
            value: 2,
        });

        assert_eq!(shared.read().name, "from-clone");
        assert_eq!(shared.version(), 2);
    }

    #[test]
    fn test_default() {
        let shared: SharedConfig<TestConfig> = SharedConfig::default();
        assert_eq!(shared.version(), 0);
        assert_eq!(shared.read().name, "");
    }

    #[tokio::test]
    async fn test_concurrent_read_during_update() {
        let shared = SharedConfig::new(TestConfig {
            name: "initial".into(),
            value: 0,
        });

        let shared_clone = shared.clone();

        // Spawn a reader
        let reader = tokio::spawn(async move {
            let mut values = Vec::new();
            for _ in 0..100 {
                let name = shared_clone.with(|c| c.name.clone());
                values.push(name);
                tokio::task::yield_now().await;
            }
            values
        });

        // Concurrently update
        for i in 0..50 {
            shared.update(TestConfig {
                name: if i % 2 == 0 {
                    "even".into()
                } else {
                    "odd".into()
                },
                value: i,
            });
            tokio::task::yield_now().await;
        }

        // Reader should never panic, all values should be valid
        let values = reader.await.expect("reader task should not panic");
        for v in &values {
            assert!(
                v == "initial" || v == "even" || v == "odd",
                "unexpected value: {v}"
            );
        }
    }
}
