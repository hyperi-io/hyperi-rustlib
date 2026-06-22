// Project:   hyperi-rustlib
// File:      src/memory/cgroup.rs
// Purpose:   Cgroup-aware memory limit + pressure detection
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Cgroup-aware memory limit and pressure detection.
//!
//! Three signals, all container-first (what the kernel/OOM-killer act on, not
//! host-wide `used/total`):
//!
//! - **limit** (`memory.max`): the hard ceiling -- crossing it is an OOM-kill.
//! - **high** (`memory.high`): the soft throttle -- the kernel reclaims hard
//!   and throttles allocations here, BEFORE the OOM-kill. Shedding before it
//!   avoids a latency cliff.
//! - **PSI** (`memory.pressure` `some avg10`): the earliest signal -- the
//!   fraction of the last 10s in which a task stalled waiting on memory. It
//!   rises on reclaim/thrash before byte ratios cross a threshold.

use std::fs;
use std::path::Path;

/// cgroup v2 mount root. The `*_at` helpers take a root so tests can point at
/// a fixture directory; the public functions use this real path.
const CGROUP_V2_ROOT: &str = "/sys/fs/cgroup";

/// Detect the memory limit for this process.
///
/// Priority:
/// 1. Cgroup v2: `/sys/fs/cgroup/memory.max`
/// 2. Cgroup v1: `/sys/fs/cgroup/memory/memory.limit_in_bytes`
/// 3. System available memory (via sysinfo)
///
/// Returns the limit in bytes.
pub fn detect_memory_limit() -> u64 {
    // Try cgroup v2
    if let Some(limit) = read_cgroup_v2_limit_at(Path::new(CGROUP_V2_ROOT)) {
        tracing::info!(
            limit_bytes = limit,
            source = "cgroup-v2",
            "detected memory limit"
        );
        return limit;
    }

    // Try cgroup v1
    if let Some(limit) = read_cgroup_v1_limit() {
        tracing::info!(
            limit_bytes = limit,
            source = "cgroup-v1",
            "detected memory limit"
        );
        return limit;
    }

    // Fallback to system memory
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let total = sys.total_memory();
    tracing::info!(
        limit_bytes = total,
        source = "system-memory",
        "detected memory limit (no cgroup)"
    );
    total
}

/// Soft throttle ceiling: cgroup v2 `memory.high`.
///
/// The kernel begins aggressive reclaim and allocation throttling here, before
/// the hard `memory.max` OOM-kill, so an app that sheds before `memory.high`
/// avoids the throughput cliff. `None` when unset (`max`) or not cgroup v2.
#[must_use]
pub fn detect_memory_high() -> Option<u64> {
    read_cgroup_v2_high_at(Path::new(CGROUP_V2_ROOT))
}

/// Detect THIS container's own memory pressure as a 0.0-1.0+ usage fraction.
///
/// Returns the WORST of `current/max` and `current/high` (v2), so the signal
/// rises as the container approaches EITHER the hard OOM ceiling or the soft
/// throttle, whichever is nearer. Falls back to v1 `usage/limit`. `None` when
/// no cgroup memory limit is in force (bare metal, or `memory.max == max`), in
/// which case callers fall back to a process/host signal.
///
/// This is the signal a container scheduler (K8s/cgroup OOM killer) actually
/// acts on -- unlike host-wide `used/total` memory, which on a large shared
/// host is unrelated to this container's limit.
#[must_use]
pub fn detect_memory_pressure() -> Option<f64> {
    detect_memory_pressure_at(Path::new(CGROUP_V2_ROOT))
}

/// Memory PSI stall fraction: cgroup v2 `memory.pressure` `some avg10`, as a
/// 0.0-1.0 fraction (`avg10` is the percentage of the last 10s in which at
/// least one task stalled waiting on memory).
///
/// This is the earliest memory-pressure signal -- it rises on reclaim/thrash
/// before byte ratios cross a threshold. Exposed for observability (emit it as
/// a gauge and alert on it); it is deliberately NOT folded into the scale/shed
/// decision, because the stall-percent at which to act is workload-specific and
/// wants per-service calibration, not a guessed constant.
///
/// `None` when PSI is unavailable (kernel < 4.20, PSI disabled, or cgroup v1).
#[must_use]
pub fn detect_memory_stall() -> Option<f64> {
    read_memory_psi_some_avg10_at(Path::new(CGROUP_V2_ROOT))
}

fn detect_memory_pressure_at(root: &Path) -> Option<f64> {
    // v2: worst of current/max (hard) and current/high (soft throttle).
    if let Some(current) = read_cgroup_v2_current_at(root) {
        let mut worst: Option<f64> = None;
        for limit in [read_cgroup_v2_limit_at(root), read_cgroup_v2_high_at(root)]
            .into_iter()
            .flatten()
            .filter(|l| *l > 0)
        {
            let ratio = current as f64 / limit as f64;
            worst = Some(worst.map_or(ratio, |w| w.max(ratio)));
        }
        return worst;
    }

    // v1 fallback: usage/limit only (no memory.high equivalent).
    let limit = read_cgroup_v1_limit()?;
    if limit == 0 {
        return None;
    }
    let current = read_cgroup_v1_current()?;
    Some(current as f64 / limit as f64)
}

fn read_cgroup_v2_limit_at(root: &Path) -> Option<u64> {
    let content = fs::read_to_string(root.join("memory.max")).ok()?;
    let trimmed = content.trim();
    if trimmed == "max" {
        return None; // No limit set
    }
    trimmed.parse::<u64>().ok()
}

fn read_cgroup_v2_high_at(root: &Path) -> Option<u64> {
    let content = fs::read_to_string(root.join("memory.high")).ok()?;
    let trimmed = content.trim();
    if trimmed == "max" {
        return None; // No soft throttle set
    }
    trimmed.parse::<u64>().ok()
}

fn read_cgroup_v2_current_at(root: &Path) -> Option<u64> {
    fs::read_to_string(root.join("memory.current"))
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

/// Parse `some avg10=N` from a cgroup v2 `memory.pressure` file, as a 0-1
/// fraction. The file looks like:
///
/// ```text
/// some avg10=0.42 avg60=0.10 avg300=0.03 total=12345
/// full avg10=0.10 avg60=0.02 avg300=0.00 total=4567
/// ```
fn read_memory_psi_some_avg10_at(root: &Path) -> Option<f64> {
    let content = fs::read_to_string(root.join("memory.pressure")).ok()?;
    let some_line = content.lines().find(|l| l.starts_with("some "))?;
    let avg10 = some_line
        .split_whitespace()
        .find_map(|field| field.strip_prefix("avg10="))?
        .parse::<f64>()
        .ok()?;
    if !avg10.is_finite() {
        return None;
    }
    Some((avg10 / 100.0).clamp(0.0, 1.0))
}

fn read_cgroup_v1_current() -> Option<u64> {
    fs::read_to_string("/sys/fs/cgroup/memory/memory.usage_in_bytes")
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
}

fn read_cgroup_v1_limit() -> Option<u64> {
    let content = fs::read_to_string("/sys/fs/cgroup/memory/memory.limit_in_bytes").ok()?;
    let value = content.trim().parse::<u64>().ok()?;
    // cgroup v1 uses a very large number for "no limit"
    if value > 1 << 62 {
        return None;
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write `name`->`contents` files into a fresh temp dir and return it.
    /// Real files on disk -- the readers do real `fs::read_to_string`, no mocks.
    fn cgroup_fixture(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (name, contents) in files {
            std::fs::write(dir.path().join(name), contents).expect("write fixture");
        }
        dir
    }

    #[test]
    fn test_detect_memory_limit_returns_nonzero() {
        let limit = detect_memory_limit();
        assert!(limit > 0, "memory limit should be positive");
    }

    #[test]
    fn test_detect_memory_pressure_is_none_or_valid_fraction() {
        // Either no cgroup limit is in force (None) or a finite, non-negative
        // ratio. We can't assert an exact value -- it depends on the host.
        match detect_memory_pressure() {
            None => {}
            Some(r) => assert!(
                r.is_finite() && r >= 0.0,
                "cgroup pressure must be finite and non-negative, got {r}"
            ),
        }
    }

    #[test]
    fn v2_limit_reads_max_and_treats_literal_max_as_unlimited() {
        let dir = cgroup_fixture(&[("memory.max", "536870912\n")]);
        assert_eq!(read_cgroup_v2_limit_at(dir.path()), Some(536_870_912));

        let dir = cgroup_fixture(&[("memory.max", "max\n")]);
        assert_eq!(
            read_cgroup_v2_limit_at(dir.path()),
            None,
            "'max' = no limit"
        );
    }

    #[test]
    fn v2_high_reads_soft_throttle_and_handles_unset() {
        let dir = cgroup_fixture(&[("memory.high", "402653184\n")]);
        assert_eq!(read_cgroup_v2_high_at(dir.path()), Some(402_653_184));

        let dir = cgroup_fixture(&[("memory.high", "max\n")]);
        assert_eq!(
            read_cgroup_v2_high_at(dir.path()),
            None,
            "'max' = no throttle"
        );

        // File absent entirely (kernel/cgroup without memory.high).
        let dir = cgroup_fixture(&[]);
        assert_eq!(read_cgroup_v2_high_at(dir.path()), None);
    }

    #[test]
    fn pressure_takes_worst_of_max_and_high() {
        // current=300M, max=512M (0.586), high=400M (0.75). Worst = 0.75:
        // the soft-throttle ratio is nearer, so we shed on it.
        let dir = cgroup_fixture(&[
            ("memory.current", "314572800\n"),
            ("memory.max", "536870912\n"),
            ("memory.high", "419430400\n"),
        ]);
        let p = detect_memory_pressure_at(dir.path()).expect("v2 pressure");
        assert!(
            (p - 0.75).abs() < 0.01,
            "worst-of should pick current/high, got {p}"
        );
    }

    #[test]
    fn pressure_uses_max_when_high_unset() {
        let dir = cgroup_fixture(&[
            ("memory.current", "268435456\n"), // 256M
            ("memory.max", "536870912\n"),     // 512M -> 0.5
            ("memory.high", "max\n"),          // unset
        ]);
        let p = detect_memory_pressure_at(dir.path()).expect("v2 pressure");
        assert!((p - 0.5).abs() < 0.01, "falls back to current/max, got {p}");
    }

    #[test]
    fn pressure_is_none_when_no_limit_in_force() {
        let dir = cgroup_fixture(&[
            ("memory.current", "268435456\n"),
            ("memory.max", "max\n"),
            ("memory.high", "max\n"),
        ]);
        assert_eq!(detect_memory_pressure_at(dir.path()), None);
    }

    #[test]
    fn psi_parses_some_avg10_as_fraction() {
        let dir = cgroup_fixture(&[(
            "memory.pressure",
            "some avg10=42.00 avg60=10.00 avg300=3.00 total=12345\n\
             full avg10=10.00 avg60=2.00 avg300=0.00 total=4567\n",
        )]);
        let stall = read_memory_psi_some_avg10_at(dir.path()).expect("psi");
        assert!(
            (stall - 0.42).abs() < 0.001,
            "avg10=42 -> 0.42, got {stall}"
        );
    }

    #[test]
    fn psi_is_none_when_file_absent() {
        let dir = cgroup_fixture(&[]);
        assert_eq!(read_memory_psi_some_avg10_at(dir.path()), None);
    }

    #[test]
    fn psi_clamps_and_handles_zero_stall() {
        let dir = cgroup_fixture(&[(
            "memory.pressure",
            "some avg10=0.00 avg60=0.00 avg300=0.00 total=0\n",
        )]);
        assert_eq!(read_memory_psi_some_avg10_at(dir.path()), Some(0.0));
    }
}
