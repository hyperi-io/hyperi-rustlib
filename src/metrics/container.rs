// Project:   hs-rustlib
// File:      src/metrics/container.rs
// Purpose:   Container-level metrics from cgroups
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Container metrics from cgroups (v1 and v2).

use std::fs;
use std::path::Path;

/// Container metrics collector.
#[derive(Debug, Clone)]
pub struct ContainerMetrics {
    namespace: String,
    cgroup_version: CgroupVersion,
}

#[derive(Debug, Clone, Copy)]
enum CgroupVersion {
    V1,
    V2,
    Unknown,
}

impl ContainerMetrics {
    /// Create a new container metrics collector.
    #[must_use]
    pub fn new(namespace: &str) -> Self {
        let cgroup_version = detect_cgroup_version();

        let this = Self {
            namespace: namespace.to_string(),
            cgroup_version,
        };

        this.register_metrics();
        this
    }

    /// Register metric descriptions.
    fn register_metrics(&self) {
        let ns = &self.namespace;

        metrics::describe_gauge!(
            format!("{ns}_container_memory_limit_bytes"),
            "Container memory limit in bytes".to_string()
        );
        metrics::describe_gauge!(
            format!("{ns}_container_memory_usage_bytes"),
            "Container memory usage in bytes".to_string()
        );
        metrics::describe_gauge!(
            format!("{ns}_container_cpu_limit_cores"),
            "Container CPU limit in cores".to_string()
        );
    }

    /// Update container metrics.
    pub fn update(&self) {
        let ns = &self.namespace;

        // Memory limit
        if let Some(limit) = self.read_memory_limit() {
            metrics::gauge!(format!("{ns}_container_memory_limit_bytes")).set(limit as f64);
        }

        // Memory usage
        if let Some(usage) = self.read_memory_usage() {
            metrics::gauge!(format!("{ns}_container_memory_usage_bytes")).set(usage as f64);
        }

        // CPU limit
        if let Some(cores) = self.read_cpu_limit() {
            metrics::gauge!(format!("{ns}_container_cpu_limit_cores")).set(cores);
        }
    }

    /// Read memory limit from cgroups.
    fn read_memory_limit(&self) -> Option<u64> {
        match self.cgroup_version {
            CgroupVersion::V2 => {
                // cgroup v2: /sys/fs/cgroup/memory.max
                read_cgroup_value("/sys/fs/cgroup/memory.max")
            }
            CgroupVersion::V1 => {
                // cgroup v1: /sys/fs/cgroup/memory/memory.limit_in_bytes
                read_cgroup_value("/sys/fs/cgroup/memory/memory.limit_in_bytes")
            }
            CgroupVersion::Unknown => None,
        }
    }

    /// Read memory usage from cgroups.
    fn read_memory_usage(&self) -> Option<u64> {
        match self.cgroup_version {
            CgroupVersion::V2 => {
                // cgroup v2: /sys/fs/cgroup/memory.current
                read_cgroup_value("/sys/fs/cgroup/memory.current")
            }
            CgroupVersion::V1 => {
                // cgroup v1: /sys/fs/cgroup/memory/memory.usage_in_bytes
                read_cgroup_value("/sys/fs/cgroup/memory/memory.usage_in_bytes")
            }
            CgroupVersion::Unknown => None,
        }
    }

    /// Read CPU limit from cgroups (returns cores).
    fn read_cpu_limit(&self) -> Option<f64> {
        match self.cgroup_version {
            CgroupVersion::V2 => {
                // cgroup v2: /sys/fs/cgroup/cpu.max contains "quota period"
                let content = fs::read_to_string("/sys/fs/cgroup/cpu.max").ok()?;
                parse_cpu_max_v2(&content)
            }
            CgroupVersion::V1 => {
                // cgroup v1: quota and period in separate files
                let quota = read_cgroup_value("/sys/fs/cgroup/cpu/cpu.cfs_quota_us")?;
                let period = read_cgroup_value("/sys/fs/cgroup/cpu/cpu.cfs_period_us")?;

                if quota == u64::MAX || period == 0 {
                    None
                } else {
                    Some(quota as f64 / period as f64)
                }
            }
            CgroupVersion::Unknown => None,
        }
    }
}

/// Detect which cgroup version is in use.
fn detect_cgroup_version() -> CgroupVersion {
    // cgroup v2 unified hierarchy
    if Path::new("/sys/fs/cgroup/cgroup.controllers").exists() {
        return CgroupVersion::V2;
    }

    // cgroup v1
    if Path::new("/sys/fs/cgroup/memory/memory.limit_in_bytes").exists() {
        return CgroupVersion::V1;
    }

    CgroupVersion::Unknown
}

/// Read a numeric value from a cgroup file.
fn read_cgroup_value(path: &str) -> Option<u64> {
    let content = fs::read_to_string(path).ok()?;
    let trimmed = content.trim();

    // Handle "max" (unlimited)
    if trimmed == "max" {
        return Some(u64::MAX);
    }

    trimmed.parse().ok()
}

/// Parse cpu.max format: "quota period" or "max period".
fn parse_cpu_max_v2(content: &str) -> Option<f64> {
    let parts: Vec<&str> = content.split_whitespace().collect();
    if parts.len() != 2 {
        return None;
    }

    let quota = parts[0];
    let period: u64 = parts[1].parse().ok()?;

    if quota == "max" || period == 0 {
        return None;
    }

    let quota_us: u64 = quota.parse().ok()?;
    Some(quota_us as f64 / period as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_metrics_new() {
        let cm = ContainerMetrics::new("test");
        assert_eq!(cm.namespace, "test");
    }

    #[test]
    fn test_parse_cpu_max_v2() {
        assert_eq!(parse_cpu_max_v2("100000 100000"), Some(1.0));
        assert_eq!(parse_cpu_max_v2("50000 100000"), Some(0.5));
        assert_eq!(parse_cpu_max_v2("max 100000"), None);
        assert_eq!(parse_cpu_max_v2("invalid"), None);
    }

    #[test]
    fn test_container_metrics_update() {
        let cm = ContainerMetrics::new("test");
        // Should not panic even if not in a container
        cm.update();
    }
}
