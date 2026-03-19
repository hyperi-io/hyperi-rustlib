// Project:   hyperi-rustlib
// File:      src/memory/mod.rs
// Purpose:   Memory management and OOM prevention
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Memory management and OOM prevention.
//!
//! Provides cgroup-aware memory tracking with backpressure signals
//! for Kubernetes-deployed services. Prevents OOM-kills by applying
//! backpressure before hitting the container memory limit.
//!
//! # Architecture
//!
//! ```text
//! Layer 1 (opt-in): Cap allocator — hard limit, last-resort crash instead of OOM-kill
//! Layer 2 (default): MemoryGuard — cgroup-aware tracking, backpressure signals
//! ```

pub mod cgroup;
pub mod guard;

pub use cgroup::detect_memory_limit;
pub use guard::{MemoryGuard, MemoryGuardConfig, MemoryPressure};
