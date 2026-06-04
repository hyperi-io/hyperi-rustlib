// Project:   hyperi-rustlib
// File:      src/governor/source.rs
// Purpose:   Pressure seam + memory source for the self-regulation governor
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Pressure seam: normalised readings, sources, and the unified latch.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::memory::MemoryGuard;

/// A normalised pressure reading, clamped to `[0.0, 1.0]` on construction.
///
/// `NaN` collapses to `0.0` (treat an unreadable source as no pressure,
/// never as max pressure -- a `NaN` masquerading as `1.0` would wedge the
/// governor into a permanent hold).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Pressure(f64);

impl Pressure {
    /// Construct a reading, clamping to `[0.0, 1.0]`. `NaN` becomes `0.0`.
    #[must_use]
    pub fn new(value: f64) -> Self {
        // `clamp` panics on NaN, and `f64::max(NaN, 0.0)` returns 0.0 only
        // for the left-NaN form -- be explicit so the intent is obvious.
        let v = if value.is_nan() {
            0.0
        } else {
            value.clamp(0.0, 1.0)
        };
        Self(v)
    }

    /// The clamped reading in `[0.0, 1.0]`.
    #[must_use]
    pub fn get(&self) -> f64 {
        self.0
    }
}

/// A source of normalised pressure feeding the unified governor.
///
/// Implementors are wrappers over the real signal (memory guard, and
/// later CPU, queue depth, etc.). Keeping the trait here -- not in the
/// signal's own module -- keeps `memory` a leaf with no governor
/// dependency.
pub trait PressureSource: Send + Sync {
    /// Stable identifier for diagnostics (e.g. `"memory"`).
    fn name(&self) -> &'static str;

    /// Sample the current pressure.
    fn sample(&self) -> Pressure;

    /// Sensitivity weight applied to SOFT signals in the combine. HARD
    /// signals ignore this (they are never down-weighted). Default `1.0`.
    fn weight(&self) -> f64 {
        1.0
    }

    /// HARD signals are never masked or down-weighted -- their raw reading
    /// always competes for the combined level. Default `false`.
    fn is_hard(&self) -> bool {
        false
    }
}

/// HARD pressure source backed by the [`MemoryGuard`].
///
/// A thin wrapper so `memory` stays a leaf module: the trait
/// implementation lives here, in the governor, not in `guard.rs`.
pub struct MemoryPressureSource(Arc<MemoryGuard>);

impl MemoryPressureSource {
    /// Wrap a shared memory guard as a pressure source.
    #[must_use]
    pub fn new(guard: Arc<MemoryGuard>) -> Self {
        Self(guard)
    }
}

impl PressureSource for MemoryPressureSource {
    fn name(&self) -> &'static str {
        "memory"
    }

    fn sample(&self) -> Pressure {
        Pressure::new(self.0.pressure_ratio())
    }

    fn weight(&self) -> f64 {
        1.0
    }

    fn is_hard(&self) -> bool {
        true
    }
}

/// Hysteresis band for the pause/resume latch.
///
/// `pause_above` must be strictly greater than `resume_below`, otherwise
/// there is no band to hold the latch and it degenerates to a single
/// threshold (flapping). [`Self::new`] validates this.
#[derive(Debug, Clone, Copy)]
pub struct Hysteresis {
    /// Arm the latch (start holding) when the level reaches this.
    pub pause_above: f64,
    /// Release the latch (stop holding) when the level drops to this.
    pub resume_below: f64,
}

impl Hysteresis {
    /// Construct a band, validating `pause_above > resume_below`.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the bounds are non-finite or `pause_above` is not
    /// strictly greater than `resume_below`.
    pub fn new(pause_above: f64, resume_below: f64) -> Result<Self, String> {
        if !pause_above.is_finite() || !resume_below.is_finite() {
            return Err(format!(
                "hysteresis bounds must be finite, got pause_above={pause_above}, \
                 resume_below={resume_below}"
            ));
        }
        if pause_above <= resume_below {
            return Err(format!(
                "hysteresis requires pause_above > resume_below, got \
                 pause_above={pause_above}, resume_below={resume_below}"
            ));
        }
        Ok(Self {
            pause_above,
            resume_below,
        })
    }
}

/// A per-source diagnostic line in a [`UnifiedPressureSnapshot`].
#[derive(Debug, Clone)]
pub struct SourceReading {
    /// Source identifier.
    pub name: &'static str,
    /// Raw clamped sample.
    pub raw: f64,
    /// Weight the source declares.
    pub weight: f64,
    /// Whether the source is HARD (raw, never masked).
    pub is_hard: bool,
    /// The value that competed for the combined level (raw for HARD,
    /// `raw * weight` for SOFT).
    pub effective: f64,
}

/// Point-in-time breakdown of the governor for diagnostics / metrics.
#[derive(Debug, Clone)]
pub struct UnifiedPressureSnapshot {
    /// Per-source readings.
    pub sources: Vec<SourceReading>,
    /// Max raw reading across HARD sources (`0.0` if none).
    pub hard_max: f64,
    /// Max weighted reading across SOFT sources (`0.0` if none).
    pub soft_max: f64,
    /// Combined level (`hard_max.max(soft_max)`).
    pub level: f64,
    /// Latched hold state at snapshot time.
    pub paused: bool,
}

/// Combines pressure sources into one level under a hysteretic latch.
///
/// See the [module docs](crate::governor) for the design invariants. The
/// latch state is an [`AtomicBool`] so [`should_hold`](Self::should_hold)
/// is a cheap, `Sync` hot-path check.
pub struct UnifiedPressure {
    sources: Vec<Arc<dyn PressureSource>>,
    hyst: Hysteresis,
    paused: AtomicBool,
}

impl UnifiedPressure {
    /// Build a governor over the given sources and hysteresis band.
    #[must_use]
    pub fn new(sources: Vec<Arc<dyn PressureSource>>, hyst: Hysteresis) -> Self {
        Self {
            sources,
            hyst,
            paused: AtomicBool::new(false),
        }
    }

    /// Add a source after construction.
    ///
    /// Proves the seam accepts a new signal kind (e.g. a future CPU
    /// source) with zero change to the gate API -- existing callers of
    /// [`level`](Self::level) / [`should_hold`](Self::should_hold) are
    /// untouched.
    pub fn add_source(&mut self, source: Arc<dyn PressureSource>) {
        self.sources.push(source);
    }

    /// Combined pressure level in `[0.0, 1.0]`.
    ///
    /// `hard_max` = max raw reading over HARD sources (never weighted,
    /// never masked). `soft_max` = max of `sample * weight` over SOFT
    /// sources. `level = hard_max.max(soft_max)`.
    #[must_use]
    pub fn level(&self) -> f64 {
        let mut hard_max = 0.0_f64;
        let mut soft_max = 0.0_f64;
        for src in &self.sources {
            let raw = src.sample().get();
            if src.is_hard() {
                hard_max = hard_max.max(raw);
            } else {
                soft_max = soft_max.max(raw * src.weight());
            }
        }
        hard_max.max(soft_max)
    }

    /// Hysteretic hold latch over [`level`](Self::level).
    ///
    /// - Held and `level <= resume_below` -> release, return `false`.
    /// - Not held and `level >= pause_above` -> arm, return `true`.
    /// - Otherwise -> return the current latch state (the band holds it).
    #[must_use]
    pub fn should_hold(&self) -> bool {
        let level = self.level();
        let paused = self.paused.load(Ordering::Acquire);
        if paused {
            if level <= self.hyst.resume_below {
                self.paused.store(false, Ordering::Release);
                return false;
            }
            true
        } else {
            if level >= self.hyst.pause_above {
                self.paused.store(true, Ordering::Release);
                return true;
            }
            false
        }
    }

    /// Per-source breakdown plus the combined level and latch state.
    #[must_use]
    pub fn snapshot(&self) -> UnifiedPressureSnapshot {
        let mut readings = Vec::with_capacity(self.sources.len());
        let mut hard_max = 0.0_f64;
        let mut soft_max = 0.0_f64;
        for src in &self.sources {
            let raw = src.sample().get();
            let weight = src.weight();
            let is_hard = src.is_hard();
            let effective = if is_hard { raw } else { raw * weight };
            if is_hard {
                hard_max = hard_max.max(raw);
            } else {
                soft_max = soft_max.max(effective);
            }
            readings.push(SourceReading {
                name: src.name(),
                raw,
                weight,
                is_hard,
                effective,
            });
        }
        UnifiedPressureSnapshot {
            sources: readings,
            hard_max,
            soft_max,
            level: hard_max.max(soft_max),
            paused: self.paused.load(Ordering::Acquire),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    /// Scriptable test double: a source whose reading can be set at
    /// runtime so a single `UnifiedPressure` can be driven through a
    /// rising/falling sequence. Stores the reading as bit-pattern `u64`
    /// so it stays `Sync` without a lock (crate forbids `unsafe`, so a
    /// `Cell` would not be `Sync`).
    struct MockSource {
        name: &'static str,
        value: AtomicU64,
        weight: f64,
        hard: bool,
    }

    impl MockSource {
        fn new(name: &'static str, value: f64, weight: f64, hard: bool) -> Self {
            Self {
                name,
                value: AtomicU64::new(value.to_bits()),
                weight,
                hard,
            }
        }

        fn set(&self, value: f64) {
            self.value.store(value.to_bits(), Ordering::Relaxed);
        }
    }

    impl PressureSource for MockSource {
        fn name(&self) -> &'static str {
            self.name
        }
        fn sample(&self) -> Pressure {
            Pressure::new(f64::from_bits(self.value.load(Ordering::Relaxed)))
        }
        fn weight(&self) -> f64 {
            self.weight
        }
        fn is_hard(&self) -> bool {
            self.hard
        }
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn pressure_clamps_and_handles_nan() {
        assert!(approx(Pressure::new(-1.0).get(), 0.0));
        assert!(approx(Pressure::new(2.0).get(), 1.0));
        assert!(approx(Pressure::new(0.5).get(), 0.5));
        // NaN must collapse to 0.0, NOT to 1.0 -- a NaN reading must never
        // wedge the governor into a permanent hold.
        assert!(approx(Pressure::new(f64::NAN).get(), 0.0));
        assert!(approx(Pressure::new(f64::INFINITY).get(), 1.0));
        assert!(approx(Pressure::new(f64::NEG_INFINITY).get(), 0.0));
    }

    #[test]
    fn hysteresis_rejects_inverted_band() {
        assert!(Hysteresis::new(0.80, 0.65).is_ok());
        assert!(Hysteresis::new(0.65, 0.80).is_err());
        assert!(Hysteresis::new(0.80, 0.80).is_err());
        assert!(Hysteresis::new(f64::NAN, 0.5).is_err());
    }

    /// The adversarial proving test.
    ///
    /// Drives one `UnifiedPressure` through the full pause/resume cycle and
    /// proves the two riskiest invariants:
    ///   1. a saturated SOFT signal at weight 0.5 cannot force a hold the
    ///      HARD signal would not (no soft-masks-hard, no spurious hold);
    ///   2. the hysteresis latch arms on the rising edge, holds inside the
    ///      band, releases on the falling edge, and re-arms cleanly (no
    ///      sticky state).
    ///
    /// Step 6 proves a third soft source plugs in via `add_source` with
    /// zero change to the gate API.
    #[test]
    fn adversarial_combine_and_hysteresis() {
        let hyst = Hysteresis::new(0.80, 0.65).expect("valid band");

        // HARD memory source + a SOFT "cpu" source at weight 0.5.
        let mem = Arc::new(MockSource::new("memory", 0.50, 1.0, true));
        let cpu = Arc::new(MockSource::new("cpu", 1.0, 0.5, false));

        let governor = UnifiedPressure::new(
            vec![
                Arc::clone(&mem) as Arc<dyn PressureSource>,
                Arc::clone(&cpu) as Arc<dyn PressureSource>,
            ],
            hyst,
        );

        // Step 1: memory=0.50, cpu=1.0 (saturated SOFT).
        // soft = 1.0 * 0.5 = 0.50; hard = 0.50; level = max(0.50, 0.50) = 0.50.
        // A saturated SOFT signal at weight 0.5 CANNOT force a hold the HARD
        // signal would not. level < pause_above -> no hold.
        assert!(
            approx(governor.level(), 0.50),
            "level should be 0.50, got {}",
            governor.level()
        );
        assert!(
            !governor.should_hold(),
            "saturated soft signal must not mask/force a hold"
        );

        // Step 2: memory rises to 0.85 -> rising edge latches.
        mem.set(0.85);
        assert!(approx(governor.level(), 0.85), "hard 0.85 dominates");
        assert!(
            governor.should_hold(),
            "rising edge above pause_above latches"
        );

        // Step 3: memory falls to 0.70 -> inside band (> resume_below) -> holds.
        mem.set(0.70);
        assert!(approx(governor.level(), 0.70));
        assert!(
            governor.should_hold(),
            "0.70 is inside the hysteresis band -> latch stays held"
        );

        // Step 4: memory falls to 0.60 -> below resume_below -> releases.
        mem.set(0.60);
        assert!(approx(governor.level(), 0.60));
        assert!(
            !governor.should_hold(),
            "falling edge below resume_below releases the latch"
        );

        // Step 5: memory back to 0.85 -> latch re-arms (no sticky state).
        mem.set(0.85);
        assert!(
            governor.should_hold(),
            "latch must re-arm cleanly with no sticky state"
        );

        // Step 6: add a THIRD soft source via add_source -- proves the seam
        // accepts a new signal kind with zero gate-API change. Release first
        // so we can observe the new source's effect cleanly.
        mem.set(0.10);
        let mut governor = governor;
        let queue = Arc::new(MockSource::new("queue_depth", 0.0, 0.5, false));
        governor.add_source(Arc::clone(&queue) as Arc<dyn PressureSource>);

        // Drop out of the band first (everything low) so the latch releases.
        cpu.set(0.0);
        assert!(!governor.should_hold(), "all sources low -> released");

        // Now saturate the new SOFT source: 1.0 * 0.5 = 0.50, still under
        // pause_above. Same gate API, same behaviour -- a weighted soft
        // source cannot force a hold on its own.
        queue.set(1.0);
        assert!(
            approx(governor.level(), 0.50),
            "new soft source weighted in"
        );
        assert!(
            !governor.should_hold(),
            "weighted third soft source still cannot force a hold"
        );

        // And the HARD signal still gets through unmasked over the new source.
        mem.set(0.90);
        assert!(approx(governor.level(), 0.90), "hard signal unmasked");
        assert!(
            governor.should_hold(),
            "hard signal re-arms over soft sources"
        );
    }

    #[test]
    fn snapshot_reports_per_source_breakdown() {
        let hyst = Hysteresis::new(0.80, 0.65).expect("valid band");
        let mem = Arc::new(MockSource::new("memory", 0.70, 1.0, true));
        let cpu = Arc::new(MockSource::new("cpu", 0.40, 0.5, false));
        let governor = UnifiedPressure::new(
            vec![
                mem as Arc<dyn PressureSource>,
                cpu as Arc<dyn PressureSource>,
            ],
            hyst,
        );

        let snap = governor.snapshot();
        assert_eq!(snap.sources.len(), 2);
        assert!(approx(snap.hard_max, 0.70));
        assert!(approx(snap.soft_max, 0.20)); // 0.40 * 0.5
        assert!(approx(snap.level, 0.70));
        assert!(!snap.paused);

        let cpu_reading = snap
            .sources
            .iter()
            .find(|r| r.name == "cpu")
            .expect("cpu present");
        assert!(!cpu_reading.is_hard);
        assert!(approx(cpu_reading.effective, 0.20));
    }

    #[test]
    fn memory_pressure_source_wraps_guard_as_hard() {
        use crate::memory::{MemoryGuard, MemoryGuardConfig};

        let guard = Arc::new(MemoryGuard::new(MemoryGuardConfig {
            limit_bytes: 1000,
            pressure_threshold: 0.80,
            ..Default::default()
        }));
        guard.add_bytes(700); // 70%
        let src = MemoryPressureSource::new(Arc::clone(&guard));

        assert_eq!(src.name(), "memory");
        assert!(src.is_hard());
        assert!(approx(src.weight(), 1.0));
        assert!(
            approx(src.sample().get(), 0.70),
            "sample should mirror guard.pressure_ratio(), got {}",
            src.sample().get()
        );
    }
}
