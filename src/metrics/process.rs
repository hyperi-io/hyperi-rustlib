// Project:   hyperi-rustlib
// File:      src/metrics/process.rs
// Purpose:   Process-level metrics collection
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Process-level metrics collection.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

/// Process metrics collector.
#[derive(Debug, Clone)]
pub struct ProcessMetrics {
    namespace: String,
    system: Arc<std::sync::Mutex<System>>,
    pid: sysinfo::Pid,
    start_time: f64,
}

impl ProcessMetrics {
    /// Create a new process metrics collector.
    #[must_use]
    pub fn new(namespace: &str) -> Self {
        let pid = sysinfo::Pid::from_u32(std::process::id());
        let system = System::new_with_specifics(
            RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
        );

        let start_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64());

        let this = Self {
            namespace: namespace.to_string(),
            system: Arc::new(std::sync::Mutex::new(system)),
            pid,
            start_time,
        };

        // Register metric descriptions
        this.register_metrics();
        this
    }

    /// Register metric descriptions.
    fn register_metrics(&self) {
        let ns = &self.namespace;

        metrics::describe_counter!(
            format!("{ns}_process_cpu_seconds_total"),
            metrics::Unit::Seconds,
            "Total user + system CPU time consumed, in seconds (cumulative counter)".to_string()
        );
        metrics::describe_gauge!(
            format!("{ns}_process_resident_memory_bytes"),
            "Resident memory size in bytes".to_string()
        );
        metrics::describe_gauge!(
            format!("{ns}_process_virtual_memory_bytes"),
            "Virtual memory size in bytes".to_string()
        );
        metrics::describe_gauge!(
            format!("{ns}_process_open_fds"),
            "Number of open file descriptors".to_string()
        );
        metrics::describe_gauge!(
            format!("{ns}_process_start_time_seconds"),
            "Start time of the process since unix epoch in seconds".to_string()
        );
    }

    /// Update process metrics.
    pub fn update(&self) {
        // Recover from a poisoned lock rather than panicking: a panic in a
        // prior update must not turn metrics collection into a repeat-panic.
        // Observability degrades, it does not crash.
        let mut system = self.system.lock().unwrap_or_else(|e| e.into_inner());
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            true,
            ProcessRefreshKind::everything(),
        );

        if let Some(process) = system.process(self.pid) {
            let ns = &self.namespace;

            // Cumulative CPU seconds as a proper monotonic COUNTER (Prometheus
            // convention). sysinfo's cpu_usage() is an instantaneous percentage,
            // NOT cumulative time -- on Linux read utime+stime from
            // /proc/self/stat instead. Utilisation is derived downstream as
            // rate(this) / container_cpu_limit_cores (in the pressure CEL / a
            // recording rule), not baked in here.
            #[cfg(target_os = "linux")]
            if let Some(cpu_secs) = cumulative_cpu_seconds() {
                // Monotonic, non-negative cumulative seconds -> whole-second
                // truncation into the u64 counter is intentional.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let secs = cpu_secs as u64;
                metrics::counter!(format!("{ns}_process_cpu_seconds_total")).absolute(secs);
            }

            // Memory
            let rss = process.memory();
            let virtual_mem = process.virtual_memory();
            metrics::gauge!(format!("{ns}_process_resident_memory_bytes")).set(rss as f64);
            metrics::gauge!(format!("{ns}_process_virtual_memory_bytes")).set(virtual_mem as f64);

            // File descriptors (Linux-specific)
            #[cfg(target_os = "linux")]
            {
                if let Ok(fds) = count_open_fds() {
                    metrics::gauge!(format!("{ns}_process_open_fds")).set(fds as f64);
                }
            }

            // Start time
            metrics::gauge!(format!("{ns}_process_start_time_seconds")).set(self.start_time);
        }
    }
}

/// Count open file descriptors (Linux only).
#[cfg(target_os = "linux")]
fn count_open_fds() -> std::io::Result<usize> {
    let fd_dir = format!("/proc/{}/fd", std::process::id());
    std::fs::read_dir(fd_dir).map(|entries| entries.count())
}

/// Cumulative process CPU time (user + system) in seconds, from
/// `/proc/self/stat` (Linux). Returns `None` if it cannot be read/parsed.
///
/// USER_HZ (clock ticks per second) is hard-coded to 100 -- the Linux default,
/// and not queryable via `sysconf` under `#![forbid(unsafe_code)]`. The u64
/// counter truncates to whole seconds; that is ample for the `rate()`-derived
/// CPU utilisation the pressure engine consumes.
#[cfg(target_os = "linux")]
pub(crate) fn cumulative_cpu_seconds() -> Option<f64> {
    const USER_HZ: f64 = 100.0;
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // "pid (comm) state ..." -- comm may contain spaces/parens, so split on the
    // LAST ')' to skip it safely.
    let rest = stat.rsplit_once(')')?.1;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // Fields after comm (0-based): [0]=state ... [11]=utime [12]=stime (ticks).
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some((utime + stime) as f64 / USER_HZ)
}

/// Non-Linux fallback: cumulative CPU seconds is unavailable (the scaling engine
/// then derives no CPU term on those platforms). Production targets are Linux.
#[cfg(all(not(target_os = "linux"), feature = "scaling", feature = "expression"))]
pub(crate) fn cumulative_cpu_seconds() -> Option<f64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_metrics_new() {
        let pm = ProcessMetrics::new("test");
        assert_eq!(pm.namespace, "test");
        assert!(pm.start_time > 0.0);
    }

    #[test]
    fn test_process_metrics_update() {
        let pm = ProcessMetrics::new("test");
        // Should not panic
        pm.update();
    }
}
