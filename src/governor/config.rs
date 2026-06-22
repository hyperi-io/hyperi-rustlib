// Project:   hyperi-rustlib
// File:      src/governor/config.rs
// Purpose:   SelfRegulationConfig -- cascade-overridable governor settings
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Cascade-overridable configuration for the self-regulation governor.
//!
//! [`SelfRegulationConfig`] is the single config section that turns the
//! data-plane governor ON (the default) or OFF. It is a sibling to
//! [`MemoryGuardConfig`](crate::memory::MemoryGuardConfig) /
//! [`ScalingPressureConfig`](crate::ScalingPressureConfig): loaded from the
//! 8-layer cascade under the `self_regulation` key and registered in the
//! config registry so the `/config` admin endpoint and hot-reload see it.
//!
//! # Default-ON, opt-out
//!
//! `enabled` defaults to `true`. When the `governor` feature is compiled in,
//! the runtime builds the pressure governor and threads it into the
//! transports + driver. To turn it OFF (byte-identical to pre-governor
//! behaviour), set:
//!
//! ```yaml
//! self_regulation:
//!   enabled: false
//! ```
//!
//! When `enabled = false` the runtime constructs NOTHING -- no pressure, no
//! gate, no byte-budget controller -- so every `Option` stays `None` and the
//! data path is the original whole-batch loop.

use serde::{Deserialize, Serialize};

use super::{ByteBudgetConfig, Hysteresis};

/// Sizing profile for the self-regulation byte budget.
///
/// Mirrors the three Kafka `SelfRegulationProfile` names so a single
/// `self_regulation.profile` config value reads the same regardless of
/// transport. It tunes the AIMD byte-budget envelope (start / ceiling /
/// step), not the Kafka client knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelfRegulationProfile {
    /// Maximum throughput: large start budget + high ceiling, coarse steps.
    #[default]
    Throughput,
    /// Balanced: moderate budget envelope.
    Balanced,
    /// Low latency: small budget envelope so blocks stay small + bursty.
    LowLatency,
}

impl SelfRegulationProfile {
    /// The byte-budget envelope for this profile. The hysteresis band and
    /// target utilisation are profile-independent (set in
    /// [`SelfRegulationConfig`]); this only sizes the AIMD lever.
    #[must_use]
    fn byte_budget_envelope(self) -> (u64, u64, u64, usize) {
        // (start_bytes, max_bytes, ai_step, record_cap)
        match self {
            Self::Throughput => (16 * 1024 * 1024, 128 * 1024 * 1024, 512 * 1024, 2000),
            Self::Balanced => (8 * 1024 * 1024, 64 * 1024 * 1024, 256 * 1024, 1000),
            Self::LowLatency => (1024 * 1024, 16 * 1024 * 1024, 128 * 1024, 500),
        }
    }
}

/// Default for [`SelfRegulationConfig::enabled`] -- the governor is ON by
/// default (opt-out via `self_regulation.enabled = false`).
const fn default_enabled() -> bool {
    true
}

fn default_pause_above() -> f64 {
    0.80
}

fn default_resume_below() -> f64 {
    0.65
}

fn default_target_rho() -> f64 {
    0.7
}

fn default_md_factor() -> f64 {
    0.5
}

/// Cascade-overridable settings for the self-regulation governor.
///
/// Loaded under the `self_regulation` key. All fields have sensible defaults
/// so an app that sets nothing gets a fully working, default-ON governor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SelfRegulationConfig {
    /// Master switch. `true` (default) -> the runtime builds the governor and
    /// threads it into the transports + driver. `false` -> nothing is built
    /// (byte-identical to pre-governor behaviour).
    pub enabled: bool,

    /// Sizing profile for the AIMD byte budget.
    pub profile: SelfRegulationProfile,

    /// Hysteresis: arm the inbound hold when combined pressure reaches this.
    pub pause_above: f64,

    /// Hysteresis: release the inbound hold when pressure drops to this.
    /// Must be strictly less than `pause_above` (validated; an invalid band
    /// falls back to the defaults).
    pub resume_below: f64,

    /// Target utilisation `rho` for the byte-budget AIMD loop, in `(0, 1)`.
    pub target_rho: f64,

    /// Multiplicative-decrease factor for the byte budget, in `(0, 1)`.
    pub md_factor: f64,
}

impl Default for SelfRegulationConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            profile: SelfRegulationProfile::default(),
            pause_above: default_pause_above(),
            resume_below: default_resume_below(),
            target_rho: default_target_rho(),
            md_factor: default_md_factor(),
        }
    }
}

impl SelfRegulationConfig {
    /// Load from the config cascade under the `self_regulation` key, registering
    /// the section so the `/config` admin endpoint + hot-reload see it.
    ///
    /// Falls back to [`SelfRegulationConfig::default()`] (default-ON) when the
    /// cascade is not initialised or the key is absent.
    #[must_use]
    pub fn from_cascade() -> Self {
        #[cfg(feature = "config")]
        {
            // `or_warn`: absent `self_regulation` key -> default-ON (silent);
            // present-but-malformed -> WARN + default (was silently swallowed
            // pre-2.8.11). Absent-key default-ON behaviour is unchanged.
            if let Some(cfg) = crate::config::try_get()
                && let Some(value) = cfg.unmarshal_key_registered_or_warn::<Self>("self_regulation")
            {
                return value;
            }
        }
        Self::default()
    }

    /// Build the [`Hysteresis`] band from the config.
    ///
    /// An inverted / non-finite band falls back to the defaults
    /// (`0.80 / 0.65`) rather than failing -- a bad knob must not wedge the
    /// governor.
    #[must_use]
    pub fn hysteresis(&self) -> Hysteresis {
        Hysteresis::new(self.pause_above, self.resume_below).unwrap_or_else(|e| {
            tracing::warn!(
                error = %e,
                "invalid self_regulation hysteresis band; using defaults 0.80/0.65"
            );
            Hysteresis::new(default_pause_above(), default_resume_below())
                .expect("default band is valid")
        })
    }

    /// Build the [`ByteBudgetConfig`] from the profile envelope + overridable
    /// `target_rho` / `md_factor`. The controller sanitises ranges itself.
    #[must_use]
    pub fn byte_budget_config(&self) -> ByteBudgetConfig {
        let (start_bytes, max_bytes, ai_step, record_cap) = self.profile.byte_budget_envelope();
        ByteBudgetConfig {
            start_bytes,
            max_bytes,
            ai_step,
            record_cap,
            target_rho: self.target_rho,
            md_factor: self.md_factor,
            ..ByteBudgetConfig::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_enabled() {
        let cfg = SelfRegulationConfig::default();
        assert!(cfg.enabled, "governor is ON by default (opt-out)");
        assert_eq!(cfg.profile, SelfRegulationProfile::Throughput);
    }

    #[test]
    fn from_cascade_falls_back_to_default_on() {
        // No cascade initialised in this unit test -> default (enabled).
        let cfg = SelfRegulationConfig::from_cascade();
        assert!(cfg.enabled);
    }

    #[test]
    fn hysteresis_uses_config_band() {
        let cfg = SelfRegulationConfig {
            pause_above: 0.9,
            resume_below: 0.5,
            ..Default::default()
        };
        let h = cfg.hysteresis();
        assert!((h.pause_above - 0.9).abs() < 1e-9);
        assert!((h.resume_below - 0.5).abs() < 1e-9);
    }

    #[test]
    fn inverted_band_falls_back_to_defaults() {
        let cfg = SelfRegulationConfig {
            pause_above: 0.3,
            resume_below: 0.8, // inverted
            ..Default::default()
        };
        let h = cfg.hysteresis();
        assert!((h.pause_above - 0.80).abs() < 1e-9);
        assert!((h.resume_below - 0.65).abs() < 1e-9);
    }

    /// An out-of-`[0,1]` band (here a negative resume that could never release
    /// the latch) must fall back to the safe defaults, not wedge the governor.
    #[test]
    fn out_of_range_band_falls_back_to_defaults() {
        let cfg = SelfRegulationConfig {
            pause_above: 0.9,
            resume_below: -0.2, // below the pressure clamp floor
            ..Default::default()
        };
        let h = cfg.hysteresis();
        assert!((h.pause_above - 0.80).abs() < 1e-9);
        assert!((h.resume_below - 0.65).abs() < 1e-9);
    }

    #[test]
    fn profile_sizes_the_byte_budget() {
        let tp = SelfRegulationConfig {
            profile: SelfRegulationProfile::Throughput,
            ..Default::default()
        }
        .byte_budget_config();
        let ll = SelfRegulationConfig {
            profile: SelfRegulationProfile::LowLatency,
            ..Default::default()
        }
        .byte_budget_config();
        assert!(
            tp.start_bytes > ll.start_bytes,
            "throughput starts bigger than low-latency"
        );
        assert!(tp.max_bytes > ll.max_bytes);
    }

    #[cfg(feature = "config")]
    #[test]
    fn serde_roundtrip_and_disabled_parse() {
        let yaml = "enabled: false\nprofile: low_latency\n";
        let cfg: SelfRegulationConfig = serde_yaml_ng::from_str(yaml).unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.profile, SelfRegulationProfile::LowLatency);
        // Defaults fill the rest.
        assert!((cfg.pause_above - 0.80).abs() < 1e-9);
    }

    /// The governor profile must serialise as snake_case so the
    /// `self_regulation.profile` cascade key reads identically to the Kafka
    /// sizing profile (rustlib<->pylib config-consistency rule).
    #[cfg(feature = "config")]
    #[test]
    fn profile_serialises_snake_case() {
        let j = serde_json::to_string(&SelfRegulationProfile::LowLatency).unwrap();
        assert_eq!(j, "\"low_latency\"");
        let j = serde_json::to_string(&SelfRegulationProfile::Throughput).unwrap();
        assert_eq!(j, "\"throughput\"");
        let j = serde_json::to_string(&SelfRegulationProfile::Balanced).unwrap();
        assert_eq!(j, "\"balanced\"");
    }
}
