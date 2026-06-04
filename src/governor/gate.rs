// Project:   hyperi-rustlib
// File:      src/governor/gate.rs
// Purpose:   Inbound gate: edge-detecting pause/resume over the pressure latch
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Inbound gate: drives an actuator on each pause/resume transition.
//!
//! The [`UnifiedPressure`] latch tells us *whether* to hold; the
//! [`InboundGate`] turns that latched boolean into EDGE events. Each
//! [`evaluate`](InboundGate::evaluate) samples the latch and, on a
//! transition only, calls the [`GateActuator`] exactly once -- `pause()`
//! on the false->true (rising) edge, `resume()` on the true->false
//! (falling) edge. While the latch stays held, repeated `evaluate()`
//! calls return [`Admit::Hold`] but do NOT re-call `pause()`; likewise a
//! released latch returns [`Admit::Yes`] without re-calling `resume()`.
//!
//! This is deliberately the INBOUND side only: the actuator pauses the
//! recv/ingest of a source (stops pulling new work) so the in-flight
//! buffer drains under pressure. It is never wired to the outbound drain
//! (sink) -- gating the drain would deadlock the pipeline. `send` is
//! never involved here.
//!
//! Additive and default-off (the `governor` feature). NOT wired into any
//! transport, driver, or runtime here; that lands in a later phase.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::source::UnifiedPressure;

/// Drives the inbound source on pause/resume edges.
///
/// Implementors translate a gate edge into a concrete action on the
/// ingest side -- e.g. stop polling a Kafka consumer, stop accepting on an
/// HTTP listener. The gate guarantees each method fires EXACTLY ONCE per
/// transition, so an implementation is free to be non-idempotent (toggle
/// a flag, pause/resume a stream) without double-pausing.
pub trait GateActuator: Send + Sync {
    /// Pause the inbound source. Called once on the rising edge.
    fn pause(&self);
    /// Resume the inbound source. Called once on the falling edge.
    fn resume(&self);
}

/// An observability decorator over a [`GateActuator`].
///
/// Wrap the real actuator (the Kafka pause/resume actuator, a [`NoopActuator`],
/// etc.) so each pause/resume EDGE emits a metric and a brake-reason log line,
/// then forwards to the inner actuator. Because the [`InboundGate`] fires each
/// edge EXACTLY ONCE, the `inbound_paused` gauge and the
/// `self_regulation_inbound_pauses_total` counter track real transitions, not
/// per-evaluate noise.
///
/// This makes inbound throttling VISIBLE: a `paused` log on the rising edge and
/// a `resumed` log on the falling edge, plus a gauge dashboards can graph.
pub struct ObservingActuator {
    inner: Box<dyn GateActuator>,
    /// Stable source label for the log line (e.g. `"kafka"`, `"http"`).
    source: &'static str,
}

impl ObservingActuator {
    /// Wrap `inner` so pause/resume edges emit metrics + logs under `source`.
    #[must_use]
    pub fn new(source: &'static str, inner: Box<dyn GateActuator>) -> Self {
        Self { inner, source }
    }
}

impl GateActuator for ObservingActuator {
    fn pause(&self) {
        #[cfg(feature = "metrics")]
        {
            ::metrics::gauge!("inbound_paused").set(1.0);
            ::metrics::counter!("self_regulation_inbound_pauses_total").increment(1);
        }
        tracing::warn!(
            source = self.source,
            "self-regulation: inbound PAUSED under pressure (memory/back-pressure brake)"
        );
        self.inner.pause();
    }

    fn resume(&self) {
        #[cfg(feature = "metrics")]
        ::metrics::gauge!("inbound_paused").set(0.0);
        tracing::info!(
            source = self.source,
            "self-regulation: inbound RESUMED, pressure cleared"
        );
        self.inner.resume();
    }
}

/// A no-op actuator for tests and send-only pipelines.
///
/// Useful when a stage wants the gate's [`Admit`] decision (to stop
/// pulling work in its own loop) but has nothing external to pause --
/// the gate's held/released state is still observable via
/// [`InboundGate::is_held`].
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopActuator;

impl GateActuator for NoopActuator {
    fn pause(&self) {}
    fn resume(&self) {}
}

/// The gate's admission decision for the next unit of inbound work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    /// Admit the next unit -- the gate is open.
    Yes,
    /// Hold (do not admit) -- the gate is closed under pressure.
    Hold,
}

/// Edge-detecting inbound gate over a [`UnifiedPressure`] latch.
///
/// Wraps the recv/ingest side of a stage. Each
/// [`evaluate`](Self::evaluate) consults the latch and drives the
/// [`GateActuator`] once per transition. See the
/// [module docs](crate::governor) for the full contract.
pub struct InboundGate {
    pressure: Arc<UnifiedPressure>,
    actuator: Box<dyn GateActuator>,
    /// Last edge state we drove the actuator to. Tracked separately from
    /// the pressure latch so the actuator fires EXACTLY ONCE per
    /// transition even though `should_hold()` returns `true` repeatedly
    /// while latched.
    paused_edge: AtomicBool,
}

impl InboundGate {
    /// Build a gate over a shared pressure latch and an actuator.
    ///
    /// The gate starts in the released (open) state; the first
    /// [`evaluate`](Self::evaluate) under pressure will fire `pause()`.
    #[must_use]
    pub fn new(pressure: Arc<UnifiedPressure>, actuator: Box<dyn GateActuator>) -> Self {
        Self {
            pressure,
            actuator,
            paused_edge: AtomicBool::new(false),
        }
    }

    /// Sample the latch and drive the actuator on a transition.
    ///
    /// Computes [`should_hold`](UnifiedPressure::should_hold) from the
    /// pressure, then uses a `compare_exchange` on the edge flag so:
    ///
    /// - false->true (rising edge): `compare_exchange(false, true)`
    ///   succeeds exactly once -> call `pause()`.
    /// - true->false (falling edge): `compare_exchange(true, false)`
    ///   succeeds exactly once -> call `resume()`.
    /// - no change: `compare_exchange` fails -> no actuator call.
    ///
    /// Returns [`Admit::Hold`] when held, [`Admit::Yes`] otherwise. Never
    /// touches the outbound side.
    pub fn evaluate(&self) -> Admit {
        let hold = self.pressure.should_hold();
        if hold {
            // Rising edge: flip false -> true exactly once.
            if self
                .paused_edge
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.actuator.pause();
            }
            Admit::Hold
        } else {
            // Falling edge: flip true -> false exactly once.
            if self
                .paused_edge
                .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.actuator.resume();
            }
            Admit::Yes
        }
    }

    /// Whether the gate last drove the actuator to the held state.
    ///
    /// Reflects the edge flag, not a fresh pressure sample -- it is the
    /// state the actuator has been driven to by the most recent
    /// [`evaluate`](Self::evaluate).
    #[must_use]
    pub fn is_held(&self) -> bool {
        self.paused_edge.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governor::source::{Hysteresis, Pressure, PressureSource};
    use std::sync::atomic::{AtomicU64, AtomicUsize};

    /// Scriptable pressure source (mirrors the G1 test double): stores the
    /// reading as a bit-pattern `u64` so it stays `Sync` without `unsafe`
    /// or a lock.
    struct MockSource {
        value: AtomicU64,
        hard: bool,
    }

    impl MockSource {
        fn new(value: f64, hard: bool) -> Self {
            Self {
                value: AtomicU64::new(value.to_bits()),
                hard,
            }
        }
        fn set(&self, value: f64) {
            self.value.store(value.to_bits(), Ordering::Relaxed);
        }
    }

    impl PressureSource for MockSource {
        fn name(&self) -> &'static str {
            "mock"
        }
        fn sample(&self) -> Pressure {
            Pressure::new(f64::from_bits(self.value.load(Ordering::Relaxed)))
        }
        fn is_hard(&self) -> bool {
            self.hard
        }
    }

    /// Counting actuator: records exactly how many times each edge fired.
    struct CountingActuator {
        pause_calls: AtomicUsize,
        resume_calls: AtomicUsize,
    }

    impl CountingActuator {
        fn new() -> Self {
            Self {
                pause_calls: AtomicUsize::new(0),
                resume_calls: AtomicUsize::new(0),
            }
        }
        fn pauses(&self) -> usize {
            self.pause_calls.load(Ordering::Relaxed)
        }
        fn resumes(&self) -> usize {
            self.resume_calls.load(Ordering::Relaxed)
        }
    }

    impl GateActuator for CountingActuator {
        fn pause(&self) {
            self.pause_calls.fetch_add(1, Ordering::Relaxed);
        }
        fn resume(&self) {
            self.resume_calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// A `GateActuator` that forwards to a shared `Arc<CountingActuator>`
    /// so the test can both hand the gate an actuator AND inspect the
    /// counts afterwards (the gate takes a `Box`, consuming ownership).
    struct SharedActuator(Arc<CountingActuator>);

    impl GateActuator for SharedActuator {
        fn pause(&self) {
            self.0.pause();
        }
        fn resume(&self) {
            self.0.resume();
        }
    }

    fn governor_with(source: Arc<MockSource>) -> Arc<UnifiedPressure> {
        let hyst = Hysteresis::new(0.80, 0.65).expect("valid band");
        Arc::new(UnifiedPressure::new(
            vec![source as Arc<dyn PressureSource>],
            hyst,
        ))
    }

    /// THE adversarial proving test for the gate.
    ///
    /// Drives one gate through low->high->high->low->low->high and proves:
    ///   1. `pause()` fires EXACTLY ONCE per rising edge (not once per
    ///      `evaluate()` while latched);
    ///   2. `resume()` fires EXACTLY ONCE per falling edge;
    ///   3. `evaluate()` returns `Hold` while latched, `Yes` otherwise;
    ///   4. the latch re-arms cleanly (the second rising edge fires
    ///      `pause()` again -- no sticky state).
    #[test]
    fn gate_drives_actuator_exactly_once_per_edge() {
        let mem = Arc::new(MockSource::new(0.10, true));
        let pressure = governor_with(Arc::clone(&mem));
        let counter = Arc::new(CountingActuator::new());
        let gate = InboundGate::new(
            Arc::clone(&pressure),
            Box::new(SharedActuator(Arc::clone(&counter))),
        );

        // LOW: open, no actuator calls yet.
        assert_eq!(gate.evaluate(), Admit::Yes);
        assert!(!gate.is_held());
        assert_eq!(counter.pauses(), 0);
        assert_eq!(counter.resumes(), 0);

        // RISING edge: 0.10 -> 0.90 (>= pause_above) -> pause() ONCE.
        mem.set(0.90);
        assert_eq!(gate.evaluate(), Admit::Hold);
        assert!(gate.is_held());
        assert_eq!(counter.pauses(), 1, "pause once on rising edge");
        assert_eq!(counter.resumes(), 0);

        // STILL HIGH: latched. Many evaluate()s, still Hold, NO extra
        // pause() -- this is the edge-dedup invariant.
        for _ in 0..5 {
            assert_eq!(gate.evaluate(), Admit::Hold);
        }
        assert_eq!(counter.pauses(), 1, "no re-pause while latched");
        assert_eq!(counter.resumes(), 0);

        // Inside the band (0.70 > resume_below 0.65): latch HOLDS, still
        // no extra calls.
        mem.set(0.70);
        assert_eq!(gate.evaluate(), Admit::Hold);
        assert_eq!(counter.pauses(), 1, "band holds, no re-pause");
        assert_eq!(counter.resumes(), 0);

        // FALLING edge: 0.70 -> 0.50 (<= resume_below) -> resume() ONCE.
        mem.set(0.50);
        assert_eq!(gate.evaluate(), Admit::Yes);
        assert!(!gate.is_held());
        assert_eq!(counter.pauses(), 1);
        assert_eq!(counter.resumes(), 1, "resume once on falling edge");

        // STILL LOW: open. Many evaluate()s, still Yes, NO extra resume().
        for _ in 0..5 {
            assert_eq!(gate.evaluate(), Admit::Yes);
        }
        assert_eq!(counter.pauses(), 1);
        assert_eq!(counter.resumes(), 1, "no re-resume while released");

        // SECOND RISING edge: re-arms cleanly -> pause() AGAIN (count 2).
        mem.set(0.95);
        assert_eq!(gate.evaluate(), Admit::Hold);
        assert!(gate.is_held());
        assert_eq!(counter.pauses(), 2, "latch re-arms, pause fires again");
        assert_eq!(counter.resumes(), 1);
    }

    #[test]
    fn noop_actuator_gate_still_tracks_held_state() {
        let mem = Arc::new(MockSource::new(0.10, true));
        let pressure = governor_with(Arc::clone(&mem));
        let gate = InboundGate::new(Arc::clone(&pressure), Box::new(NoopActuator));

        assert_eq!(gate.evaluate(), Admit::Yes);
        assert!(!gate.is_held());

        mem.set(0.90);
        assert_eq!(gate.evaluate(), Admit::Hold);
        assert!(gate.is_held());

        mem.set(0.10);
        assert_eq!(gate.evaluate(), Admit::Yes);
        assert!(!gate.is_held());
    }
}
