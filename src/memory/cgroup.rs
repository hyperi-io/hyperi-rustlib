// Project:   hyperi-rustlib
// File:      src/memory/cgroup.rs
// Purpose:   Cgroup-aware memory limit detection
// Language:  Rust
//
// License:   FSL-1.1-ALv2
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

fn read_cgroup_v2_limit() -> Option<u64> {
    let content = fs::read_to_string("/sys/fs/cgroup/memory.max").ok()?;
    let trimmed = content.trim();
    if trimmed == "max" {
        return None; // No limit set
    }
    trimmed.parse::<u64>().ok()
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
}
