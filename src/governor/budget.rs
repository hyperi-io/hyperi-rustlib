// Project:   hyperi-rustlib
// File:      src/governor/budget.rs
// Purpose:   Byte-budget controller: AIMD lever with memory HARD override
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Byte-budget controller: the self-regulation lever.
//!
//! Sizes the inbound byte budget so the stage runs at a target
//! utilisation `rho ~= 0.7` -- busy enough to be efficient, with enough
//! headroom that a burst does not blow the buffer. The loop is AIMD
//! (additive-increase / multiplicative-decrease), the classic congestion
//! lever:
//!
//! - `rho = EMA(process_time) / EMA(ingest_interval)` -- how much of the
//!   inter-arrival gap the stage spends processing. `rho < target` means
//!   slack (we can pull more); `rho > target` means we are falling
//!   behind.
//! - **slack** (`rho < target`): additive-increase the budget by
//!   `ai_step`, capped at `max_bytes`.
//! - **behind** (`rho > target`): multiplicative-decrease the budget by
//!   `md_factor` (`< 1`).
//! - **memory HARD override**: if the pressure latch says hold (or the
//!   memory source reads high), multiplicative-decrease IMMEDIATELY,
//!   regardless of rho. Memory never waits for the rho loop.
//! - **floor**: the budget never drops below `min_bytes` (derived from
//!   `floor_records`) and never reaches `0`. The [`record_cap`] poll
//!   safety cap stays `>= 1`.
//!
//! [`record_cap`]: ByteBudgetController::record_cap
//!
//! Starts BIG (`start_bytes`) and lets the decrease loop find the right
//! level, rather than starting small and ramping -- a cold pipeline should
//! not be artificially throttled.
//!
//! Gated behind the `governor` feature; folded per block by
//! [`run_governed`](crate::worker::BatchEngine::run_governed) (default-on).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::source::UnifiedPressure;

/// EMA smoothing factor for the process-time / ingest-interval signals.
const DEFAULT_EMA_ALPHA: f64 = 0.3;

/// Target utilisation: keep the stage ~70% busy, 30% headroom.
const DEFAULT_TARGET_RHO: f64 = 0.7;

/// Configuration for the [`ByteBudgetController`].
///
/// All byte values are in bytes. The controller starts at `start_bytes`
/// and moves between `min_bytes` (derived from `floor_records`) and
/// `max_bytes`.
#[derive(Debug, Clone, Copy)]
pub struct ByteBudgetConfig {
    /// Initial budget. Starts BIG so a cold pipeline is not throttled.
    pub start_bytes: u64,
    /// Hard ceiling on the budget (additive-increase saturates here).
    pub max_bytes: u64,
    /// Floor in records: the budget never drops below
    /// `floor_records * nominal_record_bytes` (and never below 1 byte).
    pub floor_records: u64,
    /// Nominal per-record size used to derive the byte floor from
    /// `floor_records`. Record sizes vary at runtime; this is only the
    /// floor estimate, not a live measurement.
    pub nominal_record_bytes: u64,
    /// Target utilisation `rho` in `(0, 1)`. Default `0.7`.
    pub target_rho: f64,
    /// Additive-increase step (bytes added per slack observation).
    pub ai_step: u64,
    /// Multiplicative-decrease factor in `(0, 1)`. Default e.g. `0.5`.
    pub md_factor: f64,
    /// EMA smoothing factor in `(0, 1]` for the timing signals.
    pub ema_alpha: f64,
    /// Poll-safety cap on record count, independent of the byte budget.
    /// A tiny-record flood cannot blow the count even within budget.
    pub record_cap: usize,
}

impl Default for ByteBudgetConfig {
    fn default() -> Self {
        Self {
            // 8 MiB start, 64 MiB ceiling -- generous defaults; a real
            // deployment tunes these via config in a later phase.
            start_bytes: 8 * 1024 * 1024,
            max_bytes: 64 * 1024 * 1024,
            floor_records: 1,
            nominal_record_bytes: 1024,
            target_rho: DEFAULT_TARGET_RHO,
            ai_step: 256 * 1024,
            md_factor: 0.5,
            ema_alpha: DEFAULT_EMA_ALPHA,
            record_cap: 2000,
        }
    }
}

impl ByteBudgetConfig {
    /// The derived absolute byte floor: `floor_records * nominal_record_bytes`,
    /// clamped to at least `1` (the budget is never `0`).
    #[must_use]
    fn min_bytes(&self) -> u64 {
        self.floor_records
            .saturating_mul(self.nominal_record_bytes)
            .max(1)
    }

    /// Sanitise the config so the control loop cannot misbehave: clamp
    /// `target_rho` and `ema_alpha` into their open ranges, force
    /// `md_factor` into `(0, 1)`, and ensure `record_cap >= 1`,
    /// `max_bytes >= min_bytes`, and `start_bytes` inside `[min, max]`.
    fn sanitised(mut self) -> Self {
        if !self.target_rho.is_finite() || self.target_rho <= 0.0 || self.target_rho >= 1.0 {
            self.target_rho = DEFAULT_TARGET_RHO;
        }
        if !self.ema_alpha.is_finite() || self.ema_alpha <= 0.0 || self.ema_alpha > 1.0 {
            self.ema_alpha = DEFAULT_EMA_ALPHA;
        }
        if !self.md_factor.is_finite() || self.md_factor <= 0.0 || self.md_factor >= 1.0 {
            self.md_factor = 0.5;
        }
        self.record_cap = self.record_cap.max(1);
        let min = self.min_bytes();
        self.max_bytes = self.max_bytes.max(min);
        self.start_bytes = self.start_bytes.clamp(min, self.max_bytes);
        self
    }
}

/// Stores an `f64` as an atomic bit-pattern so the controller stays
/// `Sync` with interior mutability and no lock (the crate forbids
/// `unsafe`, so a `Cell` would not be `Sync`).
struct AtomicF64(AtomicU64);

impl AtomicF64 {
    fn new(value: f64) -> Self {
        Self(AtomicU64::new(value.to_bits()))
    }
    fn load(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }
    fn store(&self, value: f64) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }
}

/// AIMD byte-budget lever with a memory HARD override.
///
/// See the [module docs](crate::governor) for the algorithm. `observe()`
/// is the control step; `byte_budget()` and `record_cap()` are the cheap reads
/// the recv loop consults. All state is interior-mutable and `Sync`.
pub struct ByteBudgetController {
    cfg: ByteBudgetConfig,
    pressure: Arc<UnifiedPressure>,
    /// EMA of observed batch process time, in seconds.
    ema_process_s: AtomicF64,
    /// EMA of observed ingest inter-arrival interval, in seconds.
    ema_ingest_s: AtomicF64,
    /// Whether any timing observation has been folded in yet.
    seeded: std::sync::atomic::AtomicBool,
    /// Current byte budget.
    budget: AtomicU64,
}

impl ByteBudgetController {
    /// Build a controller from config and a shared pressure latch.
    ///
    /// The config is sanitised (ranges clamped, floors enforced); the
    /// budget starts at the sanitised `start_bytes`.
    #[must_use]
    pub fn new(cfg: ByteBudgetConfig, pressure: Arc<UnifiedPressure>) -> Self {
        let cfg = cfg.sanitised();
        Self {
            budget: AtomicU64::new(cfg.start_bytes),
            ema_process_s: AtomicF64::new(0.0),
            ema_ingest_s: AtomicF64::new(0.0),
            seeded: std::sync::atomic::AtomicBool::new(false),
            cfg,
            pressure,
        }
    }

    /// Fold one observation into the control loop and update the budget.
    ///
    /// `batch_bytes` is currently informational (the loop drives off
    /// timing, not size); `process_time` is how long the batch took to
    /// process and `ingest_interval` is the gap since the previous
    /// batch's arrival.
    ///
    /// Steps, in order:
    /// 1. EMA-smooth `process_time` and `ingest_interval`.
    /// 2. Compute `rho = ema_process / ema_ingest`. A zero (or
    ///    sub-resolution) `ingest_interval` means arrivals are
    ///    back-to-back faster than we process -- treat rho as high
    ///    (behind), which is the safe direction (shrink).
    /// 3. If memory says hold (the HARD override), multiplicative-decrease
    ///    and return -- memory never waits for the rho loop.
    /// 4. Otherwise AIMD on rho vs `target_rho`.
    /// 5. Clamp to `[min_bytes, max_bytes]`.
    pub fn observe(&self, batch_bytes: u64, process_time: Duration, ingest_interval: Duration) {
        let _ = batch_bytes; // reserved for a future size-aware refinement

        let alpha = self.cfg.ema_alpha;
        let proc_s = process_time.as_secs_f64();
        let ingest_s = ingest_interval.as_secs_f64();

        // Step 1: EMA. Seed on the first observation so the average is not
        // dragged from a 0.0 cold start.
        if self.seeded.swap(true, Ordering::Relaxed) {
            let new_proc = alpha.mul_add(proc_s, (1.0 - alpha) * self.ema_process_s.load());
            let new_ingest = alpha.mul_add(ingest_s, (1.0 - alpha) * self.ema_ingest_s.load());
            self.ema_process_s.store(new_proc);
            self.ema_ingest_s.store(new_ingest);
        } else {
            self.ema_process_s.store(proc_s);
            self.ema_ingest_s.store(ingest_s);
        }

        // Step 3: memory HARD override takes precedence over rho entirely.
        if self.pressure.should_hold() {
            self.multiplicative_decrease();
            return;
        }

        // Step 2: rho. Guard div-by-zero: a non-positive ingest EMA means
        // arrivals outrun processing -> treat as "behind" (shrink).
        let ema_ingest = self.ema_ingest_s.load();
        let ema_process = self.ema_process_s.load();
        let behind = if ema_ingest <= f64::EPSILON {
            // Back-to-back arrivals with any processing cost -> behind.
            // Pure-zero processing AND zero ingest -> nothing to do.
            ema_process > 0.0
        } else {
            (ema_process / ema_ingest) > self.cfg.target_rho
        };

        // Step 4: AIMD.
        if behind {
            self.multiplicative_decrease();
        } else {
            self.additive_increase();
        }
    }

    /// Additive-increase: budget += ai_step, saturating at `max_bytes`.
    fn additive_increase(&self) {
        let cur = self.budget.load(Ordering::Relaxed);
        let next = cur.saturating_add(self.cfg.ai_step).min(self.cfg.max_bytes);
        self.budget.store(next, Ordering::Relaxed);
    }

    /// Multiplicative-decrease: budget *= md_factor, clamped to the floor
    /// and never `0`.
    fn multiplicative_decrease(&self) {
        let cur = self.budget.load(Ordering::Relaxed);
        // f64 math then back to u64; budgets are well under 2^52 so this is
        // lossless in the operating range. `floor()` then clamp.
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_sign_loss,
            clippy::cast_possible_truncation
        )]
        let scaled = (cur as f64 * self.cfg.md_factor).floor() as u64;
        let next = scaled.max(self.cfg.min_bytes());
        self.budget.store(next, Ordering::Relaxed);
    }

    /// Current byte budget. Always `>= min_bytes`, never `0`.
    #[must_use]
    pub fn byte_budget(&self) -> u64 {
        self.budget.load(Ordering::Relaxed)
    }

    /// Poll-safety record cap (recv max count), independent of the byte
    /// budget. Always `>= 1` so a tiny-record flood cannot blow the count
    /// even when many records fit inside the byte budget.
    #[must_use]
    pub fn record_cap(&self) -> usize {
        self.cfg.record_cap
    }

    /// The shared pressure governor this controller drives off. Lets a caller
    /// (e.g. the governed driver) read the combined
    /// [`level`](UnifiedPressure::level) for the `pressure_ratio` gauge without
    /// holding a second `Arc`.
    #[must_use]
    pub fn pressure(&self) -> &Arc<UnifiedPressure> {
        &self.pressure
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governor::source::{Hysteresis, Pressure, PressureSource};
    use std::sync::atomic::AtomicU64 as StdAtomicU64;

    /// Scriptable HARD source so the test can force `should_hold()`.
    struct MockSource {
        value: StdAtomicU64,
    }
    impl MockSource {
        fn new(value: f64) -> Self {
            Self {
                value: StdAtomicU64::new(value.to_bits()),
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
            true
        }
    }

    fn controller(
        cfg: ByteBudgetConfig,
        src: &Arc<MockSource>,
    ) -> (ByteBudgetController, Arc<UnifiedPressure>) {
        let hyst = Hysteresis::new(0.80, 0.65).expect("valid band");
        let pressure = Arc::new(UnifiedPressure::new(
            vec![Arc::clone(src) as Arc<dyn PressureSource>],
            hyst,
        ));
        (
            ByteBudgetController::new(cfg, Arc::clone(&pressure)),
            pressure,
        )
    }

    fn test_cfg() -> ByteBudgetConfig {
        ByteBudgetConfig {
            start_bytes: 10_000,
            max_bytes: 100_000,
            floor_records: 1,
            nominal_record_bytes: 1000, // min_bytes = 1000
            target_rho: 0.7,
            ai_step: 5_000,
            md_factor: 0.5,
            ema_alpha: 1.0, // alpha=1 -> EMA == latest sample (deterministic)
            record_cap: 2000,
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn starts_big_at_start_bytes() {
        let src = Arc::new(MockSource::new(0.0));
        let (ctl, _p) = controller(test_cfg(), &src);
        assert_eq!(ctl.byte_budget(), 10_000);
        assert!(ctl.record_cap() >= 1);
        assert_eq!(ctl.record_cap(), 2000);
    }

    /// rho < 0.7 (slack) -> budget grows additively, monotone up, capped.
    #[test]
    fn slack_grows_budget_additively_and_caps() {
        let src = Arc::new(MockSource::new(0.0)); // no memory pressure
        let (ctl, _p) = controller(test_cfg(), &src);

        // process 10ms, ingest 100ms -> rho = 0.1 < 0.7 -> slack.
        let mut last = ctl.byte_budget();
        for _ in 0..50 {
            ctl.observe(500, ms(10), ms(100));
            let now = ctl.byte_budget();
            assert!(now >= last, "budget must be monotone up under slack");
            last = now;
        }
        // ai_step 5000, start 10_000, cap 100_000 -> saturates at the cap.
        assert_eq!(ctl.byte_budget(), 100_000, "additive-increase caps at max");
    }

    /// rho > 0.7 (behind) -> budget shrinks multiplicatively toward floor.
    #[test]
    fn behind_shrinks_budget_multiplicatively() {
        let src = Arc::new(MockSource::new(0.0));
        let (ctl, _p) = controller(test_cfg(), &src);

        // process 90ms, ingest 100ms -> rho = 0.9 > 0.7 -> behind.
        let first = ctl.byte_budget();
        ctl.observe(500, ms(90), ms(100));
        let after = ctl.byte_budget();
        assert!(after < first, "behind must shrink the budget");
        // 10_000 * 0.5 = 5_000.
        assert_eq!(after, 5_000);

        // Keep going -> shrinks toward the 1000-byte floor, never below.
        for _ in 0..20 {
            ctl.observe(500, ms(90), ms(100));
        }
        assert_eq!(ctl.byte_budget(), 1_000, "shrink clamps to min_bytes");
        assert!(ctl.byte_budget() >= 1, "never zero");
    }

    /// THE adversarial test: memory HARD override beats rho.
    ///
    /// Even with rho deep in slack (would grow), forcing memory pressure
    /// high (`should_hold()` true) must multiplicative-decrease the budget
    /// IMMEDIATELY, toward the floor, never to zero.
    #[test]
    fn memory_pressure_overrides_rho_and_shrinks_to_floor() {
        let src = Arc::new(MockSource::new(0.0));
        let (ctl, _p) = controller(test_cfg(), &src);

        // Grow a bit first under slack so there is room to shrink.
        ctl.observe(500, ms(10), ms(100)); // -> 15_000
        ctl.observe(500, ms(10), ms(100)); // -> 20_000
        assert_eq!(ctl.byte_budget(), 20_000);

        // Now SLAM memory high. rho is still deep slack (10ms/100ms) but
        // the HARD override must win and SHRINK.
        src.set(0.95);
        let before = ctl.byte_budget();
        ctl.observe(500, ms(10), ms(100));
        let after = ctl.byte_budget();
        assert!(
            after < before,
            "memory override must shrink even when rho says slack"
        );
        assert_eq!(after, 10_000, "20_000 * 0.5 under override");

        // Sustained pressure drives toward the floor, never zero.
        for _ in 0..20 {
            ctl.observe(500, ms(10), ms(100));
        }
        assert_eq!(ctl.byte_budget(), 1_000, "override clamps to floor");
        assert!(ctl.byte_budget() >= 1);
        assert!(ctl.record_cap() >= 1);
    }

    /// ingest_interval == 0 must not panic or divide-by-zero, and must be
    /// treated as "behind" (shrink) -- the safe direction.
    #[test]
    fn zero_ingest_interval_is_safe_and_treated_as_behind() {
        let src = Arc::new(MockSource::new(0.0));
        let (ctl, _p) = controller(test_cfg(), &src);

        let before = ctl.byte_budget();
        // Zero ingest interval, non-zero processing -> behind -> shrink.
        ctl.observe(500, ms(5), Duration::ZERO);
        let after = ctl.byte_budget();
        assert!(after <= before, "zero ingest must not grow the budget");
        assert_eq!(after, 5_000, "treated as behind -> multiplicative-decrease");
        assert!(ctl.byte_budget() >= 1);

        // Both zero -> nothing to do (no processing cost, no arrivals):
        // must not panic and must not collapse the budget below the floor.
        let cur = ctl.byte_budget();
        ctl.observe(0, Duration::ZERO, Duration::ZERO);
        // alpha=1 so ema_process becomes 0 -> not behind -> additive-increase.
        assert!(ctl.byte_budget() >= cur, "both-zero is no-pressure slack");
    }

    /// Config sanitisation: garbage ranges fall back to safe defaults and
    /// the budget never starts at or reaches zero.
    #[test]
    fn config_is_sanitised() {
        let src = Arc::new(MockSource::new(0.0));
        let bad = ByteBudgetConfig {
            start_bytes: 0, // below floor
            max_bytes: 0,   // below floor
            floor_records: 2,
            nominal_record_bytes: 500, // min_bytes = 1000
            target_rho: 5.0,           // out of range -> default 0.7
            ai_step: 1_000,
            md_factor: 2.0, // out of range -> default 0.5
            ema_alpha: 0.0, // out of range -> default
            record_cap: 0,  // -> clamped to 1
        };
        let (ctl, _p) = controller(bad, &src);
        assert_eq!(ctl.byte_budget(), 1_000, "start clamped up to min_bytes");
        assert!(ctl.record_cap() >= 1);
    }
}
