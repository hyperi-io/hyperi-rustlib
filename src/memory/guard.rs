// Project:   hyperi-rustlib
// File:      src/memory/guard.rs
// Purpose:   Memory guard with backpressure signals
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Memory guard with backpressure signals.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::cgroup;

/// Process-wide total-heap byte source, set once at startup.
///
/// See [`set_heap_source`] for the rationale. Allocator-agnostic: any
/// `fn() -> usize` returning live heap bytes (e.g. `cap::Cap::allocated`,
/// jemalloc `stats.allocated`).
static HEAP_SOURCE: OnceLock<fn() -> usize> = OnceLock::new();

/// Register a process-wide source of total live-heap bytes.
///
/// When set, every [`MemoryGuard`] switches its read path
/// ([`current_bytes`](MemoryGuard::current_bytes), pressure checks, and
/// [`try_reserve`](MemoryGuard::try_reserve) admission) from the per-batch
/// reservation counter to this source -- a cheap, accurate, *total-process*
/// heap figure that also catches growth the per-batch reservations never see
/// (e.g. a transform ballooning a `Vec`).
///
/// **Why a global hook and not a dependency:** a tracking allocator must be
/// the binary's single `#[global_allocator]`, which is the *application's*
/// choice, not a library's -- and rustlib is `#![forbid(unsafe_code)]`, so it
/// cannot implement one anyway. The application installs its allocator and
/// wires it here in a few lines. This keeps rustlib allocator-agnostic with no
/// allocator dependency in its graph.
///
/// The first call wins and returns `true`; later calls are a no-op and return
/// `false` (the existing source is kept). Call once at startup, before
/// constructing guards.
///
/// The application picks a tracking allocator -- prefer an actively-maintained
/// one such as `tikv-jemalloc-ctl` (`stats.allocated`); the `cap` crate also
/// works but is effectively unmaintained (last release 2023).
///
/// ```ignore
/// // In the application binary, using jemalloc:
/// #[global_allocator]
/// static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
///
/// fn main() {
///     hyperi_rustlib::memory::set_heap_source(|| {
///         tikv_jemalloc_ctl::epoch::advance().ok();
///         tikv_jemalloc_ctl::stats::allocated::read().unwrap_or(0)
///     });
///     // ... build ServiceRuntime / MemoryGuard ...
/// }
/// ```
#[must_use]
pub fn set_heap_source(source: fn() -> usize) -> bool {
    HEAP_SOURCE.set(source).is_ok()
}

/// Read the registered total-heap source, if any.
#[inline]
fn heap_bytes() -> Option<u64> {
    HEAP_SOURCE.get().map(|f| f() as u64)
}

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
    /// Usage above pressure_threshold -- apply backpressure.
    High,
}

/// Configuration for `MemoryGuard`.
///
/// When the `config` feature is enabled, this can be loaded from the config
/// cascade under the `memory` key:
///
/// ```yaml
/// memory:
///   limit_bytes: 0           # 0 = auto-detect from cgroup/system
///   pressure_threshold: 0.80 # backpressure at 80% of effective limit
///   cgroup_headroom: 0.85    # use 85% of cgroup limit
/// ```
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct MemoryGuardConfig {
    /// Explicit memory limit in bytes. 0 = auto-detect from cgroup/system.
    #[serde(default)]
    pub limit_bytes: u64,
    /// Fraction of limit at which backpressure activates (default 0.8).
    #[serde(default = "default_pressure_threshold")]
    pub pressure_threshold: f64,
    /// Fraction of cgroup limit to use as the effective limit (default 0.85).
    /// Leaves headroom for the process itself (stack, code, etc.).
    #[serde(default = "default_cgroup_headroom")]
    pub cgroup_headroom: f64,
}

fn default_pressure_threshold() -> f64 {
    DEFAULT_PRESSURE_THRESHOLD
}

fn default_cgroup_headroom() -> f64 {
    DEFAULT_CGROUP_HEADROOM
}

/// A fraction is valid iff it is finite and within `(0.0, 1.0]`.
fn check_fraction(v: f64, name: &str) -> Result<(), String> {
    if !v.is_finite() || v <= 0.0 || v > 1.0 {
        return Err(format!(
            "memory.{name} must be a finite fraction in (0.0, 1.0], got {v}"
        ));
    }
    Ok(())
}

/// Return `v` if it is a valid fraction, else log an error and substitute
/// `default`. Defensive guard so a bad config cannot produce a zero/`NaN`
/// limit and a divide-by-zero pressure ratio.
fn sane_fraction(v: f64, default: f64, name: &str) -> f64 {
    if check_fraction(v, name).is_err() {
        tracing::error!(
            value = v,
            "invalid memory.{name} (need finite fraction in (0,1]); using default {default}"
        );
        default
    } else {
        v
    }
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
    /// Load from the config cascade, falling back to defaults.
    ///
    /// When the `config` feature is enabled and `config::setup()` has been
    /// called, reads the `memory` key from the cascade. Otherwise returns
    /// [`MemoryGuardConfig::default()`].
    #[must_use]
    pub fn from_cascade() -> Self {
        #[cfg(feature = "config")]
        {
            if let Some(cfg) = crate::config::try_get()
                && let Ok(memory) = cfg.unmarshal_key_registered::<Self>("memory")
            {
                return memory;
            }
        }
        Self::default()
    }

    /// Create config from environment variables with a prefix.
    ///
    /// Reads standard env vars for memory configuration:
    /// - `{PREFIX}_MEMORY_LIMIT_BYTES` -- explicit limit (0 or unset = auto-detect from cgroup)
    /// - `{PREFIX}_MEMORY_PRESSURE_THRESHOLD` -- backpressure trigger (default 0.80)
    /// - `{PREFIX}_MEMORY_CGROUP_HEADROOM` -- fraction of cgroup limit to use (default 0.85)
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

    /// Validate the config, returning an error describing the first invalid
    /// field. `pressure_threshold` and `cgroup_headroom` must each be a finite
    /// fraction in `(0.0, 1.0]`. Call this at startup to fail fast on bad
    /// config rather than relying on [`MemoryGuard::new`]'s defensive clamping.
    ///
    /// # Errors
    ///
    /// Returns `Err` with a human-readable message if a fraction field is
    /// non-finite, `<= 0.0`, or `> 1.0`.
    pub fn validate(&self) -> Result<(), String> {
        check_fraction(self.pressure_threshold, "pressure_threshold")?;
        check_fraction(self.cgroup_headroom, "cgroup_headroom")?;
        Ok(())
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
/// // On data arrival -- check before accepting
/// let payload_len = 1024u64;
/// if !guard.try_reserve(payload_len) {
///     // return 503 -- backpressure
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
        // Defensive: a non-finite / out-of-range threshold or headroom would
        // produce a zero/NaN limit and a divide-by-zero pressure ratio. Clamp
        // to the safe default and log loudly. Callers wanting hard rejection
        // should call `config.validate()` at startup.
        let pressure_threshold = sane_fraction(
            config.pressure_threshold,
            DEFAULT_PRESSURE_THRESHOLD,
            "pressure_threshold",
        );
        let cgroup_headroom = sane_fraction(
            config.cgroup_headroom,
            DEFAULT_CGROUP_HEADROOM,
            "cgroup_headroom",
        );

        let raw_limit = if config.limit_bytes > 0 {
            config.limit_bytes
        } else {
            let detected = cgroup::detect_memory_limit();
            // Apply headroom -- don't use 100% of cgroup limit
            (detected as f64 * cgroup_headroom) as u64
        };
        // Never permit a zero effective limit: every pressure calculation
        // divides by it.
        let limit_bytes = raw_limit.max(1);

        tracing::info!(limit_bytes, pressure_threshold, "memory guard initialised");

        Self {
            current_bytes: AtomicU64::new(0),
            limit_bytes,
            pressure_threshold,
            under_pressure: AtomicBool::new(false),
        }
    }

    /// Try to reserve bytes. Returns false if over the limit (backpressure).
    ///
    /// With a registered [`set_heap_source`], this is a projected-admission
    /// check against the *true total heap* (`heap() + bytes <= limit`) and does
    /// NOT mutate the reservation counter -- the allocator already accounts the
    /// bytes once they are allocated, and frees them on drop, so no `release`
    /// is needed. Without a source it is the classic atomic check-and-add on
    /// the per-batch counter (rolled back if it would exceed the limit).
    #[inline]
    pub fn try_reserve(&self, bytes: u64) -> bool {
        if let Some(heap) = heap_bytes() {
            return heap + bytes <= self.limit_bytes;
        }
        let current = self.current_bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
        if current > self.limit_bytes {
            // Over limit -- roll back
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

    /// Fast hot-path pressure check.
    ///
    /// With a registered [`set_heap_source`], computes live from the true heap
    /// (one atomic load + compare); otherwise reads the cached flag maintained
    /// by `try_reserve`/`add_bytes`/`release`.
    #[inline]
    pub fn under_pressure(&self) -> bool {
        if heap_bytes().is_some() {
            return self.pressure_ratio() >= self.pressure_threshold;
        }
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
        self.current_bytes() as f64 / self.limit_bytes as f64
    }

    /// Current memory usage in bytes.
    ///
    /// Returns the true total live heap when a [`set_heap_source`] is
    /// registered, otherwise the sum of outstanding per-batch reservations.
    #[inline]
    pub fn current_bytes(&self) -> u64 {
        heap_bytes().unwrap_or_else(|| self.current_bytes.load(Ordering::Relaxed))
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
        guard.add_bytes(900); // 90% -- over threshold
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
        guard.release(200); // release more than added -- saturates to 0
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
        // All bytes should be released -- may not be exactly 0 due to ordering
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

    // Process-global heap source for the switch test. nextest isolates each
    // test in its own process, so registering it here is contained to this
    // test and does not leak into the per-batch-counter tests above. (This is
    // the single test in this module that touches the global hook.)
    static TEST_HEAP: AtomicU64 = AtomicU64::new(0);
    fn test_heap_source() -> usize {
        TEST_HEAP.load(Ordering::Relaxed) as usize
    }

    #[test]
    fn heap_source_overrides_read_path_and_admission() {
        assert!(set_heap_source(test_heap_source), "first set wins");
        assert!(
            !set_heap_source(test_heap_source),
            "second set is a no-op (first-wins)"
        );

        let guard = MemoryGuard::new(MemoryGuardConfig {
            limit_bytes: 1_000,
            pressure_threshold: 0.8,
            ..Default::default()
        });

        // Reads come from the heap source, not the reservation counter.
        TEST_HEAP.store(250, Ordering::Relaxed);
        assert_eq!(guard.current_bytes(), 250);
        assert!((guard.pressure_ratio() - 0.25).abs() < 0.001);
        assert!(!guard.under_pressure());

        // Pressure tracks the live heap -- including growth never reserved,
        // which the per-batch counter would have been blind to.
        TEST_HEAP.store(850, Ordering::Relaxed);
        assert!(
            guard.under_pressure(),
            "85% live heap is over the 80% threshold"
        );
        assert_eq!(guard.pressure(), MemoryPressure::High);

        // try_reserve is a projected-admission check against the true heap and
        // does NOT mutate the reservation counter.
        TEST_HEAP.store(900, Ordering::Relaxed);
        assert!(guard.try_reserve(100), "900 + 100 == limit, admitted");
        assert!(!guard.try_reserve(200), "900 + 200 > limit, rejected");
        assert_eq!(
            guard.current_bytes(),
            900,
            "counter untouched by try_reserve"
        );
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
    fn test_validate_accepts_defaults_and_rejects_bad_fractions() {
        assert!(MemoryGuardConfig::default().validate().is_ok());

        for bad in [0.0, -0.1, 1.5, f64::NAN, f64::INFINITY] {
            let cfg = MemoryGuardConfig {
                pressure_threshold: bad,
                ..Default::default()
            };
            assert!(
                cfg.validate().is_err(),
                "pressure_threshold={bad} must be rejected"
            );
            let cfg = MemoryGuardConfig {
                cgroup_headroom: bad,
                ..Default::default()
            };
            assert!(
                cfg.validate().is_err(),
                "cgroup_headroom={bad} must be rejected"
            );
        }
    }

    #[test]
    fn test_new_clamps_invalid_config_no_divide_by_zero() {
        // A zero/NaN headroom with auto-detect could yield a zero limit ->
        // divide-by-zero. A zero pressure_threshold would make every ratio
        // "over". new() must clamp to safe defaults and keep ratios finite.
        let guard = MemoryGuard::new(MemoryGuardConfig {
            limit_bytes: 0,
            pressure_threshold: 0.0,
            cgroup_headroom: 0.0,
        });
        assert!(guard.limit_bytes() >= 1, "limit floored at >=1");
        guard.add_bytes(10);
        assert!(
            guard.pressure_ratio().is_finite(),
            "pressure ratio must be finite, not div-by-zero"
        );
    }

    #[test]
    fn test_new_with_nan_threshold_is_finite() {
        let guard = MemoryGuard::new(MemoryGuardConfig {
            limit_bytes: 1000,
            pressure_threshold: f64::NAN,
            cgroup_headroom: f64::NAN,
        });
        assert_eq!(guard.limit_bytes(), 1000);
        guard.add_bytes(900);
        // Clamped threshold (0.8 default) -> 90% is over -> under pressure.
        assert!(guard.under_pressure());
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
