// Project:   hyperi-rustlib
// File:      src/metrics/process.rs
// Purpose:   Process-level metrics collection
// Language:  Rust
//
// License:   FSL-1.1-ALv2
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
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

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

        metrics::describe_gauge!(
            format!("{ns}_process_cpu_seconds_total"),
            "Total user and system CPU time spent in seconds".to_string()
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
        let mut system = self.system.lock().expect("lock poisoned");
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            true,
            ProcessRefreshKind::everything(),
        );

        if let Some(process) = system.process(self.pid) {
            let ns = &self.namespace;

            // CPU time (approximate - sysinfo gives percentage, not total time)
            let cpu_usage = f64::from(process.cpu_usage());
            metrics::gauge!(format!("{ns}_process_cpu_seconds_total")).set(cpu_usage);

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
