// Project:   hyperi-rustlib
// File:      src/scaling/mod.rs
// Purpose:   Scaling pressure calculation for KEDA autoscaling
// Language:  Rust
//
// License:   FSL-1.1-ALv2
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Scaling pressure calculation for autoscaler integration.
//!
//! Produces a 0.0–100.0 composite metric based on weighted application
//! signals with two hard gates (circuit breaker, memory pressure).
//! Designed for KEDA but works with any autoscaler that reads Prometheus
//! gauges.
//!
//! ## Architecture
//!
//! ```text
//! App signals ──→ ScalingPressure ──→ {prefix}_scaling_pressure gauge
//!                  ├─ Gate: circuit breaker open → 0.0
//!                  ├─ Gate: memory ≥ threshold → 100.0
//!                  └─ Weighted composite → 0.0–100.0
//! ```
//!
//! ## Usage
//!
//! 1. Define components with weights and saturation points
//! 2. Create `ScalingPressure` with base config + components
//! 3. Update component values from your pipeline (lock-free)
//! 4. Call `calculate()` when rendering Prometheus metrics
//!
//! ```rust
//! use hyperi_rustlib::scaling::{ScalingPressure, ScalingPressureConfig, ScalingComponent};
//!
//! let pressure = ScalingPressure::new(
//!     ScalingPressureConfig::default(),
//!     vec![
//!         ScalingComponent::new("kafka_lag", 0.35, 100_000.0),
//!         ScalingComponent::new("buffer_depth", 0.25, 10_000.0),
//!         ScalingComponent::new("memory", 0.40, 1.0),
//!     ],
//! );
//!
//! // Update from pipeline (lock-free, call from any thread)
//! pressure.set_component("kafka_lag", 50_000.0);
//! pressure.set_memory(400_000_000, 1_000_000_000);
//!
//! // Render in Prometheus endpoint
//! let value = pressure.calculate();
//! assert!(value >= 0.0 && value <= 100.0);
//! ```
//!
//! ## CPU Scaling
//!
//! CPU is intentionally **not** included in the composite. KEDA's native
//! CPU trigger reads from the Kubernetes metrics-server (container-level
//! CPU utilisation). Configure both triggers independently in your
//! KEDA `ScaledObject`:
//!
//! - `scaling_pressure` gauge → Prometheus scaler (app-level signals)
//! - CPU utilisation → CPU scaler (container-level, via metrics-server)
//!
//! KEDA scales to the MAX of all triggers.

mod config;
mod pressure;
mod rate_window;

pub use config::{ScalingComponent, ScalingPressureConfig};
pub use pressure::{ComponentSnapshot, GateType, PressureSnapshot, ScalingPressure};
pub use rate_window::RateWindow;
