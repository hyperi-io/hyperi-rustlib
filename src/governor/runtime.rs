// Project:   hyperi-rustlib
// File:      src/governor/runtime.rs
// Purpose:   SelfRegulationGovernor -- the built, wired-in governor bundle
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! The constructed governor bundle the runtime threads into transports + driver.
//!
//! [`SelfRegulationGovernor`] is what [`SelfRegulationConfig::build`] produces
//! when `enabled = true`: ONE shared [`UnifiedPressure`] (memory-only HARD
//! source today) plus ONE [`ByteBudgetController`] (the AIMD lever) over that
//! same pressure. The runtime constructs it BEFORE the transports and the
//! engine driver so the pressure can be threaded into:
//!
//! - the inbound gate of each receive transport (Kafka pause-partitions,
//!   HTTP/gRPC 503), via [`pressure`](Self::pressure);
//! - the byte-budget lever feeding the streaming driver's sub-block size +
//!   recv `max`, via [`budget`](Self::budget).
//!
//! When `enabled = false` the runtime builds NOTHING -- it never calls
//! [`SelfRegulationConfig::build`] -- so all the downstream `Option`s stay
//! `None` and the data path is byte-identical to pre-governor behaviour.

use std::sync::Arc;

use crate::memory::MemoryGuard;

use super::config::SelfRegulationConfig;
use super::{ByteBudgetController, MemoryPressureSource, PressureSource, UnifiedPressure};

/// The constructed self-regulation governor: shared pressure + byte budget.
///
/// Built once by the runtime when self-regulation is enabled, then cloned
/// (cheap `Arc` bumps) into the transports and the driver.
#[derive(Clone)]
pub struct SelfRegulationGovernor {
    pressure: Arc<UnifiedPressure>,
    budget: Arc<ByteBudgetController>,
}

impl SelfRegulationGovernor {
    /// The shared pressure governor. Thread a clone into each receive
    /// transport's inbound gate (`InboundGate::new(governor.pressure(), ...)`)
    /// and into the HTTP/gRPC `with_pressure(Some(...))` hooks.
    #[must_use]
    pub fn pressure(&self) -> Arc<UnifiedPressure> {
        Arc::clone(&self.pressure)
    }

    /// The shared byte-budget controller (AIMD lever). The streaming driver
    /// reads [`byte_budget`](ByteBudgetController::byte_budget) per block for
    /// the sub-block size + recv `max`, and calls
    /// [`observe`](ByteBudgetController::observe) per block.
    #[must_use]
    pub fn budget(&self) -> Arc<ByteBudgetController> {
        Arc::clone(&self.budget)
    }

    /// Build an [`InboundGate`](super::InboundGate) for a given source label
    /// and actuator, wrapped in an [`ObservingActuator`](super::ObservingActuator)
    /// so pause/resume edges emit metrics + brake-reason logs.
    ///
    /// This is the one-call form of the gate dance: pass the transport's own
    /// actuator (e.g. `KafkaTransport::gate_actuator()`) and a label, get back
    /// a gate over THIS governor's shared pressure. Hand the result to
    /// `KafkaTransport::with_inbound_gate(...)`.
    #[must_use]
    pub fn inbound_gate(
        &self,
        source: &'static str,
        actuator: Box<dyn super::GateActuator>,
    ) -> super::InboundGate {
        super::InboundGate::new(
            self.pressure(),
            Box::new(super::ObservingActuator::new(source, actuator)),
        )
    }

    /// Attach a self-regulation inbound gate to a Kafka receive transport
    /// (`transport-kafka` feature) -- the full
    /// `gate_actuator -> InboundGate -> with_inbound_gate` dance in one call.
    ///
    /// Pauses the consumer's ASSIGNED partitions under pressure (member stays
    /// in the group -- no rebalance) with pause/resume edges observed via
    /// [`ObservingActuator`](super::ObservingActuator). Returns the transport
    /// with the gate attached.
    #[cfg(feature = "transport-kafka")]
    #[must_use]
    pub fn attach_kafka_gate(
        &self,
        transport: crate::transport::kafka::KafkaTransport,
    ) -> crate::transport::kafka::KafkaTransport {
        let gate = self.inbound_gate("kafka", transport.gate_actuator());
        transport.with_inbound_gate(gate)
    }
}

impl SelfRegulationConfig {
    /// Construct the [`SelfRegulationGovernor`] over a shared memory guard.
    ///
    /// Returns `None` when `enabled = false` -- the caller then builds nothing
    /// and the data path stays byte-identical to pre-governor behaviour.
    ///
    /// Construction order matters: this is called BEFORE the transports + the
    /// engine driver so the produced pressure / budget can be threaded in.
    #[must_use]
    pub fn build(&self, memory_guard: Arc<MemoryGuard>) -> Option<SelfRegulationGovernor> {
        if !self.enabled {
            tracing::info!("self-regulation governor disabled (self_regulation.enabled=false)");
            return None;
        }

        // ONE pressure governor over a single HARD memory source. New SOFT
        // sources (CPU, queue depth) plug in later via `add_source` with zero
        // change to the gate / budget APIs.
        let sources: Vec<Arc<dyn PressureSource>> =
            vec![Arc::new(MemoryPressureSource::new(memory_guard)) as Arc<dyn PressureSource>];
        let pressure = Arc::new(UnifiedPressure::new(sources, self.hysteresis()));

        // ONE byte-budget controller (AIMD lever) over the SAME pressure, so
        // its memory HARD override consults the same latch the gate does.
        let budget = Arc::new(ByteBudgetController::new(
            self.byte_budget_config(),
            Arc::clone(&pressure),
        ));

        tracing::info!(
            profile = ?self.profile,
            pause_above = self.pause_above,
            resume_below = self.resume_below,
            start_byte_budget = budget.byte_budget(),
            "self-regulation governor enabled (default-on)"
        );

        Some(SelfRegulationGovernor { pressure, budget })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryGuard, MemoryGuardConfig};

    fn guard() -> Arc<MemoryGuard> {
        Arc::new(MemoryGuard::new(MemoryGuardConfig {
            limit_bytes: 1024 * 1024,
            ..Default::default()
        }))
    }

    #[test]
    fn disabled_builds_nothing() {
        let cfg = SelfRegulationConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(
            cfg.build(guard()).is_none(),
            "disabled -> no governor constructed (Options stay None)"
        );
    }

    #[test]
    fn enabled_builds_pressure_and_budget() {
        let cfg = SelfRegulationConfig::default();
        let gov = cfg.build(guard()).expect("enabled by default");
        // Pressure is low (empty guard) -> gate would admit, budget starts big.
        assert!(gov.pressure().level() < cfg.pause_above);
        assert!(gov.budget().byte_budget() >= 1);
    }

    #[test]
    fn pressure_reflects_memory_guard() {
        let g = guard();
        g.add_bytes(900 * 1024); // ~88% of 1 MiB
        let gov = SelfRegulationConfig::default()
            .build(Arc::clone(&g))
            .expect("enabled");
        // The HARD memory source feeds the pressure level directly.
        assert!(
            gov.pressure().level() > 0.5,
            "high memory should raise the pressure level, got {}",
            gov.pressure().level()
        );
    }
}
