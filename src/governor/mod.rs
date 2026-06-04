// Project:   hyperi-rustlib
// File:      src/governor/mod.rs
// Purpose:   Unified self-regulation pressure governor
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Unified self-regulation governor.
//!
//! The pressure seam for the data-plane self-regulation governor. It
//! combines a set of [`PressureSource`]s into a single normalised
//! [`level`](UnifiedPressure::level) and a hysteretic
//! [`should_hold`](UnifiedPressure::should_hold) latch that downstream
//! stages consult to decide whether to pause inbound work.
//!
//! The pieces: [`UnifiedPressure`] (the latch over the sources),
//! [`InboundGate`] (turns the latch into pause/resume EDGES on the inbound
//! source -- never the sink), [`ByteBudgetController`] (the AIMD lever sizing
//! the streaming sub-block budget), [`SelfRegulationConfig`] (the cascade
//! `self_regulation` section, default-ON), and [`SelfRegulationGovernor`]
//! (the built bundle the runtime threads into transports + driver).
//!
//! Self-regulation is **ON by default** (opt-out `self_regulation.enabled =
//! false`). See the docs for the full picture: `docs/SELF-REGULATION.md`
//! (the three brains -- memory is the HARD source of truth, CPU deliberately
//! dropped), `docs/BACKPRESSURE.md` (gate the source, never the sink), and
//! `docs/KAFKA-PATH.md` (the three batch sizes + the rho ~ 0.7 loop).
//!
//! # Design invariants
//!
//! - **HARD signals are never masked.** A HARD source (e.g. the memory
//!   guard) contributes its raw reading to the combined level with no
//!   weight applied. A saturated SOFT signal can never *lower* the level
//!   below what the HARD signal demands, and the absence of a HARD signal
//!   can never be hidden by a busy SOFT one. This is the never-OOM
//!   guarantee: the memory signal always gets through.
//! - **SOFT signals are weighted.** Each SOFT source's reading is scaled
//!   by its [`weight`](PressureSource::weight) (a sensitivity knob) before
//!   it competes for the level. A low-weight SOFT source at full
//!   saturation cannot force a hold the HARD signal would not.
//! - **Hysteresis prevents flapping.** The latch arms at `pause_above`
//!   and releases at `resume_below`; between the two it holds its current
//!   state. This stops a reading that oscillates around a single
//!   threshold from rapidly toggling pause/resume.
//!
//! The seam is feature-gated (the `governor` feature) and default-ON when
//! compiled in: the runtime builds the governor and threads it into the
//! transports + driver unless `self_regulation.enabled = false`. New source
//! kinds (e.g. a future CPU source) plug in via
//! [`UnifiedPressure::add_source`] with zero change to the gate API.

mod budget;
mod config;
mod gate;
mod runtime;
mod source;

pub use budget::{ByteBudgetConfig, ByteBudgetController};
pub use config::{SelfRegulationConfig, SelfRegulationProfile};
pub use gate::{Admit, GateActuator, InboundGate, NoopActuator, ObservingActuator};
pub use runtime::SelfRegulationGovernor;
pub use source::{
    Hysteresis, MemoryPressureSource, Pressure, PressureSource, UnifiedPressure,
    UnifiedPressureSnapshot,
};
