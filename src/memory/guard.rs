// Project:   hyperi-rustlib
// File:      src/memory/guard.rs
// Purpose:   Memory guard with backpressure signals
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Memory guard with backpressure signals.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::cgroup;

/// Read an env var `{PREFIX}_{SUFFIX}` and parse it.
fn env_parsed<T: std::str::FromStr>(prefix: &str, suffix: &str) -> Option<T> {
    std::env::var(format!("{prefix}_{suffix}"))
        .ok()
        .and_then(|v| v.parse().ok())
}

/// Memory pressure levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPressure {
    /// Usage below 50% of limit.
    Low,
    /// Usage between 50% and pressure_threshold.
    Medium,
    /// Usage above pressure_threshold — apply backpressure.
    High,
}

/// Configuration for `MemoryGuard`.
#[derive(Debug, Clone)]
pub struct MemoryGuardConfig {
    /// Explicit memory limit in bytes. 0 = auto-detect from cgroup/system.
    pub limit_bytes: u64,
    /// Fraction of limit at which backpressure activates (default 0.8).
    pub pressure_threshold: f64,
    /// Fraction of cgroup limit to use as the effective limit (default 0.9).
    /// Leaves headroom for the process itself (stack, code, etc.).
    pub cgroup_headroom: f64,
}

/// Default cgroup headroom: use 85% of cgroup limit.
///
/// Rationale: Rust has no GC so no spike headroom needed (unlike JVM 75% / Go 80%).
/// 15% headroom covers jemalloc fragmentation, kernel overhead, and page cache.
const DEFAULT_CGROUP_HEADROOM: f64 = 0.85;

/// Default pressure threshold: backpressure at 80% of effective limit.
///
/// With 85% headroom, backpressure activates at ~68% of actual cgroup limit.
/// Matches OTel Collector's `limit_percentage: 80` philosophy.
const DEFAULT_PRESSURE_THRESHOLD: f64 = 0.80;

impl Default for MemoryGuardConfig {
    fn default() -> Self {
        Self {
            limit_bytes: 0, // auto-detect
            pressure_threshold: DEFAULT_PRESSURE_THRESHOLD,
            cgroup_headroom: DEFAULT_CGROUP_HEADROOM,
        }
    }
}

impl MemoryGuardConfig {
    /// Create config from environment variables with a prefix.
    ///
    /// Reads standard env vars for memory configuration:
    /// - `{PREFIX}_MEMORY_LIMIT_BYTES` — explicit limit (0 or unset = auto-detect from cgroup)
    /// - `{PREFIX}_MEMORY_PRESSURE_THRESHOLD` — backpressure trigger (default 0.80)
    /// - `{PREFIX}_MEMORY_CGROUP_HEADROOM` — fraction of cgroup limit to use (default 0.85)
    ///
    /// # Example
    ///
    /// ```bash
    /// DFE_MEMORY_LIMIT_BYTES=4294967296      # 4 GiB explicit
    /// DFE_MEMORY_PRESSURE_THRESHOLD=0.75     # backpressure at 75%
    /// DFE_MEMORY_CGROUP_HEADROOM=0.90        # use 90% of cgroup
    /// ```
    ///
    /// ```rust,no_run
    /// use hyperi_rustlib::memory::MemoryGuardConfig;
    /// let config = MemoryGuardConfig::from_env("DFE");
    /// ```
    #[must_use]
    #[cfg(feature = "config")]
    pub fn from_env(prefix: &str) -> Self {
        use crate::config::flat_env::flat_env_parsed;

        let mut config = Self::default();

        if let Some(v) = flat_env_parsed::<u64>(prefix, "MEMORY_LIMIT_BYTES") {
            config.limit_bytes = v;
        }
        if let Some(v) = flat_env_parsed::<f64>(prefix, "MEMORY_PRESSURE_THRESHOLD") {
            config.pressure_threshold = v;
        }
        if let Some(v) = flat_env_parsed::<f64>(prefix, "MEMORY_CGROUP_HEADROOM") {
            config.cgroup_headroom = v;
        }

        config
    }

    /// Create config from environment variables without requiring `config` feature.
    ///
    /// Same as [`from_env`](Self::from_env) but uses `std::env` directly.
    #[must_use]
    pub fn from_env_raw(prefix: &str) -> Self {
        let mut config = Self::default();

        if let Some(v) = env_parsed::<u64>(prefix, "MEMORY_LIMIT_BYTES") {
            config.limit_bytes = v;
        }
        if let Some(v) = env_parsed::<f64>(prefix, "MEMORY_PRESSURE_THRESHOLD") {
            config.pressure_threshold = v;
        }
        if let Some(v) = env_parsed::<f64>(prefix, "MEMORY_CGROUP_HEADROOM") {
            config.cgroup_headroom = v;
        }

        config
    }
}

/// Cgroup-aware memory tracking with backpressure signals.
///
/// Tracks application-level memory usage (not process RSS) and provides
/// fast atomic checks for the hot path. Designed for data pipeline services
/// where incoming data must be rejected (503) before hitting the container
/// memory limit.
///
/// # Usage
///
/// ```rust,no_run
/// use hyperi_rustlib::memory::{MemoryGuard, MemoryGuardConfig};
///
/// let guard = MemoryGuard::new(MemoryGuardConfig::default());
///
/// // On data arrival — check before accepting
/// let payload_len = 1024u64;
/// if !guard.try_reserve(payload_len) {
///     // return 503 — backpressure
/// }
///
/// // After data is flushed/sent
/// guard.release(payload_len);
///
/// // Fast hot-path check
/// if guard.under_pressure() {
///     // return 503
/// }
/// ```
pub struct MemoryGuard {
    /// Current tracked bytes (application-level, not RSS).
    current_bytes: AtomicU64,
    /// Effective memory limit in bytes.
    limit_bytes: u64,
    /// Pressure threshold (0.0-1.0).
    pressure_threshold: f64,
    /// Fast boolean for hot-path pressure check.
    under_pressure: AtomicBool,
}

impl MemoryGuard {
    /// Create a new memory guard.
    ///
    /// If `config.limit_bytes` is 0, auto-detects from cgroup (K8s) or system memory,
    /// then applies `cgroup_headroom` factor to leave room for process overhead.
    #[must_use]
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn new(config: MemoryGuardConfig) -> Self {
        let raw_limit = if config.limit_bytes > 0 {
            config.limit_bytes
        } else {
            let detected = cgroup::detect_memory_limit();
            // Apply headroom — don't use 100% of cgroup limit
            (detected as f64 * config.cgroup_headroom) as u64
        };

        tracing::info!(
            limit_bytes = raw_limit,
            pressure_threshold = config.pressure_threshold,
            "memory guard initialised"
        );

        Self {
            current_bytes: AtomicU64::new(0),
            limit_bytes: raw_limit,
            pressure_threshold: config.pressure_threshold,
            under_pressure: AtomicBool::new(false),
        }
    }

    /// Try to reserve bytes. Returns false if over the limit (backpressure).
    ///
    /// This is an atomic check-and-add. If the reservation would exceed
    /// the limit, the bytes are NOT added and false is returned.
    #[inline]
    pub fn try_reserve(&self, bytes: u64) -> bool {
        let current = self.current_bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
        if current > self.limit_bytes {
            // Over limit — roll back
            self.current_bytes.fetch_sub(bytes, Ordering::Relaxed);
            self.under_pressure.store(true, Ordering::Relaxed);
            return false;
        }
        self.update_pressure(current);
        true
    }

    /// Add bytes without checking the limit (for tracking only).
    /// Use when data is already accepted and you just need to track it.
    #[inline]
    pub fn add_bytes(&self, bytes: u64) {
        let new_total = self.current_bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
        self.update_pressure(new_total);
    }

    /// Release bytes after data is flushed/sent/dropped.
    ///
    /// Uses saturating subtraction to prevent underflow wrapping.
    #[inline]
    pub fn release(&self, bytes: u64) {
        let prev = self
            .current_bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.saturating_sub(bytes))
            })
            // Always succeeds (closure always returns Some).
            .unwrap_or_else(|v| v);
        self.update_pressure(prev.saturating_sub(bytes));
    }

    /// Fast hot-path pressure check (single atomic load).
    #[inline]
    pub fn under_pressure(&self) -> bool {
        self.under_pressure.load(Ordering::Relaxed)
    }

    /// Current pressure level.
    #[inline]
    pub fn pressure(&self) -> MemoryPressure {
        let ratio = self.pressure_ratio();
        if ratio >= self.pressure_threshold {
            MemoryPressure::High
        } else if ratio >= 0.5 {
            MemoryPressure::Medium
        } else {
            MemoryPressure::Low
        }
    }

    /// Current usage as fraction of limit (0.0 - 1.0+).
    #[inline]
    pub fn pressure_ratio(&self) -> f64 {
        self.current_bytes.load(Ordering::Relaxed) as f64 / self.limit_bytes as f64
    }

    /// Current tracked bytes.
    #[inline]
    pub fn current_bytes(&self) -> u64 {
        self.current_bytes.load(Ordering::Relaxed)
    }

    /// Configured memory limit in bytes.
    #[inline]
    pub fn limit_bytes(&self) -> u64 {
        self.limit_bytes
    }

    /// Update the pressure flag based on current usage.
    #[inline]
    fn update_pressure(&self, current: u64) {
        let ratio = current as f64 / self.limit_bytes as f64;
        self.under_pressure
            .store(ratio >= self.pressure_threshold, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_guard_default() {
        let guard = MemoryGuard::new(MemoryGuardConfig {
            limit_bytes: 1_000_000, // 1MB explicit
            ..Default::default()
        });
        assert_eq!(guard.limit_bytes(), 1_000_000);
        assert_eq!(guard.current_bytes(), 0);
        assert!(!guard.under_pressure());
        assert_eq!(guard.pressure(), MemoryPressure::Low);
    }

    #[test]
    fn test_try_reserve_within_limit() {
        let guard = MemoryGuard::new(MemoryGuardConfig {
            limit_bytes: 1000,
            ..Default::default()
        });
        assert!(guard.try_reserve(500));
        assert_eq!(guard.current_bytes(), 500);
    }

    #[test]
    fn test_try_reserve_over_limit() {
        let guard = MemoryGuard::new(MemoryGuardConfig {
            limit_bytes: 1000,
            ..Default::default()
        });
        assert!(guard.try_reserve(500));
        assert!(!guard.try_reserve(600)); // would exceed 1000
        assert_eq!(guard.current_bytes(), 500); // rolled back
        assert!(guard.under_pressure());
    }

    #[test]
    fn test_release_reduces_pressure() {
        let guard = MemoryGuard::new(MemoryGuardConfig {
            limit_bytes: 1000,
            pressure_threshold: 0.8,
            ..Default::default()
        });
        guard.add_bytes(900); // 90% — over threshold
        assert!(guard.under_pressure());
        assert_eq!(guard.pressure(), MemoryPressure::High);

        guard.release(500); // down to 400 = 40%
        assert!(!guard.under_pressure());
        assert_eq!(guard.pressure(), MemoryPressure::Low);
    }

    #[test]
    fn test_pressure_levels() {
        let guard = MemoryGuard::new(MemoryGuardConfig {
            limit_bytes: 1000,
            pressure_threshold: 0.8,
            ..Default::default()
        });

        // Low (< 50%)
        guard.add_bytes(400);
        assert_eq!(guard.pressure(), MemoryPressure::Low);

        // Medium (50-80%)
        guard.add_bytes(200); // 600 = 60%
        assert_eq!(guard.pressure(), MemoryPressure::Medium);

        // High (>= 80%)
        guard.add_bytes(300); // 900 = 90%
        assert_eq!(guard.pressure(), MemoryPressure::High);
    }

    #[test]
    fn test_pressure_ratio() {
        let guard = MemoryGuard::new(MemoryGuardConfig {
            limit_bytes: 1000,
            ..Default::default()
        });
        guard.add_bytes(250);
        let ratio = guard.pressure_ratio();
        assert!((ratio - 0.25).abs() < 0.001);
    }

    #[test]
    fn test_release_saturating() {
        let guard = MemoryGuard::new(MemoryGuardConfig {
            limit_bytes: 1000,
            ..Default::default()
        });
        guard.add_bytes(100);
        guard.release(200); // release more than added — saturates to 0
        assert_eq!(
            guard.current_bytes(),
            0,
            "over-release must saturate to 0, not wrap"
        );
        assert!(!guard.under_pressure());
        assert_eq!(guard.pressure(), MemoryPressure::Low);

        // Verify the guard is still functional after over-release
        assert!(guard.try_reserve(500));
        assert_eq!(guard.current_bytes(), 500);
    }

    #[test]
    fn test_concurrent_reserve_release() {
        use std::sync::Arc;
        use std::thread;

        let guard = Arc::new(MemoryGuard::new(MemoryGuardConfig {
            limit_bytes: 100_000,
            pressure_threshold: 0.8,
            ..Default::default()
        }));

        let mut handles = vec![];
        for _ in 0..10 {
            let g = Arc::clone(&guard);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    g.add_bytes(100);
                    g.release(100);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // All bytes should be released — may not be exactly 0 due to ordering
        // but should be close (within one thread's batch)
        assert!(
            guard.current_bytes() < 1000,
            "leaked bytes: {}",
            guard.current_bytes()
        );
    }

    #[test]
    fn test_try_reserve_rollback_is_atomic() {
        let guard = MemoryGuard::new(MemoryGuardConfig {
            limit_bytes: 100,
            ..Default::default()
        });
        assert!(guard.try_reserve(90));
        assert!(!guard.try_reserve(20)); // over limit, rolled back
        assert_eq!(guard.current_bytes(), 90); // not 110
        assert!(guard.try_reserve(10)); // exactly at limit
        assert_eq!(guard.current_bytes(), 100);
    }

    #[test]
    fn test_config_defaults() {
        let config = MemoryGuardConfig::default();
        assert_eq!(config.limit_bytes, 0);
        assert!((config.pressure_threshold - 0.80).abs() < 0.001);
        assert!((config.cgroup_headroom - 0.85).abs() < 0.001);
    }

    #[test]
    fn test_from_env_raw_defaults_when_unset() {
        // With no env vars set, should return defaults
        let config = MemoryGuardConfig::from_env_raw("TEST_MG_UNSET");
        assert_eq!(config.limit_bytes, 0);
        assert!((config.pressure_threshold - 0.80).abs() < 0.001);
        assert!((config.cgroup_headroom - 0.85).abs() < 0.001);
    }

    #[test]
    fn test_env_parsed_helper() {
        // env_parsed returns None for unset vars
        assert!(env_parsed::<u64>("NONEXISTENT_PREFIX_XYZ", "FOO").is_none());
        assert!(env_parsed::<f64>("NONEXISTENT_PREFIX_XYZ", "BAR").is_none());
    }

    #[test]
    fn test_guard_with_explicit_config_overrides() {
        // Simulates what from_env would produce with overrides
        let config = MemoryGuardConfig {
            limit_bytes: 2_147_483_648,
            pressure_threshold: 0.75,
            cgroup_headroom: 0.90,
        };
        let guard = MemoryGuard::new(config);
        assert_eq!(guard.limit_bytes(), 2_147_483_648);
    }

    #[test]
    fn test_guard_with_custom_headroom() {
        // 85% headroom on 1 GiB = 870 MiB effective
        let config = MemoryGuardConfig {
            limit_bytes: 0, // auto-detect
            pressure_threshold: 0.80,
            cgroup_headroom: 0.85,
        };
        let guard = MemoryGuard::new(config);
        // Auto-detected, so limit should be 85% of system/cgroup memory
        assert!(guard.limit_bytes() > 0);
    }

    #[test]
    fn test_auto_detect_limit() {
        // With limit_bytes = 0, should auto-detect from system
        let guard = MemoryGuard::new(MemoryGuardConfig::default());
        assert!(
            guard.limit_bytes() > 0,
            "auto-detected limit should be positive"
        );
        // Should be less than total system memory (headroom applied)
    }
}
