// Project:   hyperi-rustlib
// File:      src/memory/cgroup.rs
// Purpose:   Cgroup-aware memory limit detection
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Cgroup-aware memory limit detection.

use std::fs;

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
    if let Some(limit) = read_cgroup_v2_limit() {
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

/// Detect THIS container's own memory pressure as `current / limit`.
///
/// Reads the cgroup's `memory.current` (v2) / `memory.usage_in_bytes` (v1)
/// against its `memory.max` / `memory.limit_in_bytes`. Returns `None` when no
/// cgroup memory limit is in force (bare metal, or `memory.max == max`), in
/// which case callers should fall back to a process/host signal.
///
/// This is the signal a container scheduler (K8s/cgroup OOM killer) actually
/// acts on -- unlike host-wide `used/total` memory, which on a large shared
/// host is unrelated to this container's limit.
#[must_use]
pub fn detect_memory_pressure() -> Option<f64> {
    let limit = read_cgroup_v2_limit().or_else(read_cgroup_v1_limit)?;
    if limit == 0 {
        return None;
    }
    let current = read_cgroup_v2_current().or_else(read_cgroup_v1_current)?;
    Some(current as f64 / limit as f64)
}

fn read_cgroup_v2_limit() -> Option<u64> {
    let content = fs::read_to_string("/sys/fs/cgroup/memory.max").ok()?;
    let trimmed = content.trim();
    if trimmed == "max" {
        return None; // No limit set
    }
    trimmed.parse::<u64>().ok()
}

fn read_cgroup_v2_current() -> Option<u64> {
    fs::read_to_string("/sys/fs/cgroup/memory.current")
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
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
}
