// Project:   hyperi-rustlib
// File:      tests/scaling_pressure_property.rs
// Purpose:   Property/fuzz + matrix tests for the horizontal scaling-pressure engine
// Language:  Rust
//
// License:   BUSL-1.1
// Copyright: (c) 2026 HYPERI PTY LIMITED

//! Diverse tests for the CEL-over-local-metrics scaling-pressure engine.
//!
//! The pressure-from-CEL-from-metrics path is highly branchy, so this exercises:
//! - a property/fuzz sweep (deterministic LCG -- no `rand` dep) over thousands of
//!   random contexts + a set of expressions, asserting outputs are ALWAYS finite,
//!   bounded, and never panic;
//! - the per-transport compound-pressure matrix;
//! - precedence (config expression vs rustlib smart default);
//! - the context-aware default (kafka-inbound uses lag, non-kafka omits it).
//!
//! Real engine API, no mocks. Requires `scaling` + `expression`.

#![cfg(all(feature = "scaling", feature = "expression"))]

use hyperi_rustlib::scaling::{
    PressureExpr, PressureTargets, ScalingEngine, ScalingEngineConfig, ScalingTransport,
    TransportSignals, inbound_pressure, outbound_pressure,
};

/// Tiny deterministic PRNG (SplitMix64) -- reproducible, no dependency.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform f64 in [0, max].
    // Test PRNG: turning random bits into an f64 is an intentional, lossy mix.
    #[allow(clippy::cast_precision_loss)]
    fn f64_to(&mut self, max: f64) -> f64 {
        let frac = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        frac * max
    }
    fn bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }
    fn kind(&mut self) -> ScalingTransport {
        match self.next_u64() % 8 {
            0 => ScalingTransport::Kafka,
            1 => ScalingTransport::Redis,
            2 => ScalingTransport::Http,
            3 => ScalingTransport::Grpc,
            4 => ScalingTransport::File,
            5 => ScalingTransport::Pipe,
            6 => ScalingTransport::Memory,
            _ => ScalingTransport::Other,
        }
    }
}

fn cfg(pressures: Vec<PressureExpr>, params: &[(&str, f64)]) -> ScalingEngineConfig {
    let mut c = ScalingEngineConfig {
        pressures,
        ..Default::default()
    };
    for (k, v) in params {
        c.params.insert((*k).to_string(), *v);
    }
    c
}

fn expr(name: &str, expression: &str) -> PressureExpr {
    PressureExpr {
        name: name.to_string(),
        expression: expression.to_string(),
        enabled: true,
    }
}

// ── Property: the smart default is ALWAYS finite, in [0,100], circuit-gated ──

#[test]
fn property_smart_default_is_bounded_and_finite() {
    let mut rng = Rng::new(0xDEAD_BEEF);
    for _ in 0..20_000 {
        let kind = rng.kind();
        let lag_target = if rng.bool() {
            rng.f64_to(500_000.0)
        } else {
            0.0
        };
        let (engine, errs) = ScalingEngine::new(
            "prop",
            &cfg(
                vec![],
                &[
                    ("cpu_target", 0.1 + rng.f64_to(0.9)),
                    ("lag_target", lag_target),
                ],
            ),
            kind,
            ScalingTransport::Kafka,
        );
        assert!(errs.is_empty());
        let signals = TransportSignals {
            kafka_assigned_lag: Some(rng.f64_to(5_000_000.0)),
            redis_pending: Some(rng.f64_to(1_000_000.0)),
            inflight: Some(rng.f64_to(5_000.0)),
            shed_rate: Some(rng.f64_to(500.0)),
            circuit_open: rng.bool(),
            ..Default::default()
        };
        let cpu = rng.f64_to(3.0);
        let mem = rng.f64_to(1.0);
        let out = engine.evaluate(&signals, cpu, mem);
        assert_eq!(out.len(), 1);
        let v = out[0].1;
        assert!(v.is_finite(), "non-finite default: {v}");
        assert!((0.0..=100.0).contains(&v), "out of [0,100]: {v}");
        if signals.circuit_open {
            assert!(v.abs() < f64::EPSILON, "circuit open must gate to 0");
        }
    }
}

// ── Property: arbitrary user expressions never panic + stay finite ──

#[test]
fn property_user_expressions_never_panic_and_stay_finite() {
    // A diverse set: arithmetic, ternary (CEL has no max(), so use ?:), params,
    // metrics map, comparisons, division (incl. by potentially-zero target).
    let exprs = [
        "cpu_utilisation_ratio * 100.0",
        "transport_inbound_pressure_ratio * 100.0",
        "circuit_open ? 0.0 : 100.0 * cpu_utilisation_ratio",
        "cpu_utilisation_ratio > transport_inbound_pressure_ratio ? cpu_utilisation_ratio * 100.0 : transport_inbound_pressure_ratio * 100.0",
        "metrics.kafka_assigned_lag / params.lag_target",
        "(cpu_utilisation_ratio / params.cpu_target) * 100.0",
        "memory_ratio * 100.0",
        "transport_outbound_pressure_ratio * 50.0 + cpu_utilisation_ratio * 50.0",
        // Domain signal: present roughly half the ticks (else NoSuchKey -> guarded
        // fallback). Either way the emitted value must be finite -- never panic.
        "custom.ch_backlog / params.ch_target",
    ];
    let pressures: Vec<PressureExpr> = exprs
        .iter()
        .enumerate()
        .map(|(i, e)| expr(&format!("p{i}"), e))
        .collect();
    let n = pressures.len();
    let (engine, errs) = ScalingEngine::new(
        "prop",
        &cfg(
            pressures,
            &[
                ("cpu_target", 0.70),
                ("lag_target", 100_000.0),
                ("ch_target", 50_000.0),
            ],
        ),
        ScalingTransport::Kafka,
        ScalingTransport::Kafka,
    );
    assert!(
        errs.is_empty(),
        "valid expressions should compile: {errs:?}"
    );

    let mut rng = Rng::new(0x1234_5678);
    for _ in 0..20_000 {
        let mut custom = std::collections::BTreeMap::new();
        // Sometimes push the domain signal the custom expression reads, sometimes
        // leave it absent (-> runtime NoSuchKey -> guarded fallback) to fuzz both.
        if rng.bool() {
            custom.insert("ch_backlog".to_string(), rng.f64_to(500_000.0));
        }
        let signals = TransportSignals {
            kafka_assigned_lag: Some(rng.f64_to(5_000_000.0)),
            redis_pending: Some(rng.f64_to(1_000_000.0)),
            inflight: Some(rng.f64_to(5_000.0)),
            shed_rate: Some(rng.f64_to(500.0)),
            send_backpressure_rate: Some(rng.f64_to(500.0)),
            refused_rate: Some(rng.f64_to(500.0)),
            produce_queue_depth: Some(rng.f64_to(100_000.0)),
            circuit_open: rng.bool(),
            custom,
        };
        let out = engine.evaluate(&signals, rng.f64_to(3.0), rng.f64_to(1.0));
        assert_eq!(out.len(), n);
        for (name, v) in out {
            // The engine guards non-finite results (-> last-good / smart default),
            // so EVERY emitted value must be finite regardless of the expression.
            assert!(v.is_finite(), "pressure '{name}' produced non-finite {v}");
        }
    }
}

// ── Property: the compound computor is finite + non-negative for any input ──

#[test]
fn property_compound_pressure_finite_nonnegative() {
    let mut rng = Rng::new(0xABCD_1234);
    for _ in 0..20_000 {
        let signals = TransportSignals {
            kafka_assigned_lag: Some(rng.f64_to(5_000_000.0)),
            redis_pending: Some(rng.f64_to(1_000_000.0)),
            inflight: Some(rng.f64_to(5_000.0)),
            shed_rate: Some(rng.f64_to(500.0)),
            send_backpressure_rate: Some(rng.f64_to(500.0)),
            refused_rate: Some(rng.f64_to(500.0)),
            produce_queue_depth: Some(rng.f64_to(100_000.0)),
            circuit_open: rng.bool(),
            ..Default::default()
        };
        let mut params = std::collections::BTreeMap::new();
        if rng.bool() {
            params.insert("lag_target".to_string(), rng.f64_to(200_000.0));
        }
        if rng.bool() {
            params.insert("redis_lag_target".to_string(), rng.f64_to(100_000.0));
        }
        params.insert(
            "http_concurrency_target".to_string(),
            1.0 + rng.f64_to(500.0),
        );
        params.insert("shed_target".to_string(), 1.0 + rng.f64_to(100.0));
        let targets = PressureTargets::from_params(&params);
        let kind = rng.kind();
        let inb = inbound_pressure(kind, &signals, &targets);
        let outb = outbound_pressure(&signals, &targets);
        assert!(inb.is_finite() && inb >= 0.0, "inbound {inb} for {kind:?}");
        assert!(outb.is_finite() && outb >= 0.0, "outbound {outb}");
        if !kind.is_horizontally_scalable_inbound() {
            assert!(
                inb.abs() < f64::EPSILON,
                "{kind:?} must contribute 0 inbound"
            );
        }
    }
}

// ── Matrix: precedence -- a config expression overrides the smart default ──

#[test]
fn precedence_config_expression_overrides_default() {
    // Smart default at CPU 0.70/0.70 would be 100; the config expression forces 7.
    let (engine, errs) = ScalingEngine::new(
        "t",
        &cfg(vec![expr("fixed", "7.0")], &[("cpu_target", 0.70)]),
        ScalingTransport::Kafka,
        ScalingTransport::Kafka,
    );
    assert!(errs.is_empty());
    let v = engine.evaluate(&TransportSignals::default(), 0.70, 0.0);
    assert_eq!(v.len(), 1);
    assert!((v[0].1 - 7.0).abs() < 1e-9);
}

// ── Matrix: context-aware default -- non-kafka inbound ignores kafka lag ──

#[test]
fn context_aware_default_ignores_lag_for_non_kafka_inbound() {
    let signals = TransportSignals {
        kafka_assigned_lag: Some(10_000_000.0), // huge -- but inbound is HTTP
        ..Default::default()
    };
    let (engine, _) = ScalingEngine::new(
        "t",
        &cfg(vec![], &[("cpu_target", 0.70), ("lag_target", 1.0)]),
        ScalingTransport::Http, // inbound is HTTP, so kafka lag is irrelevant
        ScalingTransport::Kafka,
    );
    // No in-flight, low CPU -> pressure ~0 despite the giant kafka lag.
    let v = engine.evaluate(&signals, 0.0, 0.0);
    assert!(
        v[0].1.abs() < f64::EPSILON,
        "HTTP inbound must not read kafka lag"
    );

    // Same signals, kafka inbound -> lag dominates -> clamped to 100.
    let (engine_k, _) = ScalingEngine::new(
        "t",
        &cfg(vec![], &[("cpu_target", 0.70), ("lag_target", 1.0)]),
        ScalingTransport::Kafka,
        ScalingTransport::Kafka,
    );
    assert!((engine_k.evaluate(&signals, 0.0, 0.0)[0].1 - 100.0).abs() < 1e-9);
}

// ── Matrix: a missing MAP key (metrics.*/custom.*) is warn-and-kept at load ──
//
// As of 2.8.11, custom domain signals are pushed at RUNTIME and so cannot be
// pre-populated in the load-time dry-run; the cel error for a missing map key
// (`NoSuchKey`) is identical whether it's `custom.<name>` or `metrics.<typo>`.
// The contract is: DOWNGRADE a missing-map-key to a load warning + KEEP the
// program; the runtime guard falls back to the smart default if it really
// errors. Syntax errors and unknown TOP-LEVEL identifiers stay hard rejects.

#[test]
fn missing_map_key_is_kept_then_falls_back_at_runtime() {
    // metrics.does_not_exist is a NoSuchKey at the dry-run -> warn-and-keep,
    // NOT a hard load error.
    let (engine, errs) = ScalingEngine::new(
        "t",
        &cfg(
            vec![expr("bad", "metrics.does_not_exist * 2.0")],
            &[("cpu_target", 0.70)],
        ),
        ScalingTransport::File, // CPU-only smart default for the fallback
        ScalingTransport::Kafka,
    );
    assert!(
        errs.is_empty(),
        "missing map key should be warn-and-kept, not a hard load error: {errs:?}"
    );
    // At runtime the key is still absent -> eval errors -> smart default (100 at
    // CPU 0.70/0.70). The pressure is still emitted (no panic, finite).
    let v = engine.evaluate(&TransportSignals::default(), 0.70, 0.0);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].0, "bad");
    assert!(
        (v[0].1 - 100.0).abs() < 1e-6,
        "missing metric must fall back to the smart default, got {}",
        v[0].1
    );
}

// ── Matrix: an unknown TOP-LEVEL identifier is STILL caught hard at LOAD ──

#[test]
fn unknown_top_level_identifier_caught_at_load() {
    let (_engine, errs) = ScalingEngine::new(
        "t",
        &cfg(vec![expr("bad", "cpu_utilisation_ratoi * 2.0")], &[]),
        ScalingTransport::Kafka,
        ScalingTransport::Kafka,
    );
    assert_eq!(
        errs.len(),
        1,
        "unknown top-level identifier should hard-fail at load"
    );
    assert!(errs[0].contains("bad"));
}

// ── Matrix: a custom domain signal flows end-to-end via set_custom path ──

#[test]
fn custom_signal_scales_end_to_end() {
    let (engine, errs) = ScalingEngine::new(
        "t",
        &cfg(
            vec![expr("ch", "custom.clickhouse_backlog / params.ch_target")],
            &[("ch_target", 2000.0)],
        ),
        ScalingTransport::File,
        ScalingTransport::Kafka,
    );
    assert!(errs.is_empty(), "custom.* must not hard-reject: {errs:?}");
    let mut signals = TransportSignals::default();
    signals.custom.insert("clickhouse_backlog".into(), 5000.0);
    let v = engine.evaluate(&signals, 0.0, 0.0);
    assert!(
        (v[0].1 - 2.5).abs() < 1e-9,
        "custom 5000/2000 = 2.5, got {}",
        v[0].1
    );
}

// ── Matrix: disabled pressure is not evaluated/emitted ──

#[test]
fn disabled_pressure_is_skipped() {
    let mut p = expr("off", "cpu_utilisation_ratio * 100.0");
    p.enabled = false;
    let (engine, errs) = ScalingEngine::new(
        "t",
        &cfg(vec![p], &[]),
        ScalingTransport::Kafka,
        ScalingTransport::Kafka,
    );
    assert!(errs.is_empty());
    assert!(
        engine
            .evaluate(&TransportSignals::default(), 0.9, 0.0)
            .is_empty()
    );
}
